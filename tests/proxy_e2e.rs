//! 端到端：真实 HTTP（Router + WAF + JWT 鉴权）+ `DbClient`（network 协议）+ 只读护栏 + 面板 API。

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use dhrust::net::http::HttpServer;
use pek_rcode::dal::Dal;
use pek_rcode::db_service::DbClient;
use pek_rdbproxy::config::{DatabaseSection, ProxyConfig, ServerSection};
use pek_rdbproxy::proxy::Proxy;

/// 临时目录（pid + 纳秒唯一）。
fn temp_dir(name: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "rdbproxy-e2e-{}-{stamp}-{name}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 建一个 SQLite 库并放一行数据，返回代理实例。
fn build_proxy(base: &Path, read_only: bool, password_hash: &str) -> Arc<Proxy> {
    let db = base.join("demo.db");
    {
        let dal = Dal::open(&format!("Data Source={};Provider=SQLite", db.display())).unwrap();
        let mut session = dal.open_session().unwrap();
        session
            .execute("CREATE TABLE t (Id INTEGER PRIMARY KEY, Name TEXT)", &[])
            .unwrap();
        session
            .execute("INSERT INTO t (Id, Name) VALUES (1, 'hello')", &[])
            .unwrap();
    }
    let config = ProxyConfig {
        server: ServerSection {
            host: "127.0.0.1".into(),
            port: 0,
            allow_remote: false,
            web_user_name: "admin".into(),
            web_password_hash: password_hash.into(),
            web_auth_level: "LocalOnly".into(),
            jwt_secret: String::new(),
        },
        databases: vec![DatabaseSection {
            name: "demo".into(),
            connection: format!("Data Source={};Provider=SQLite", db.display()),
            token: "tk-test-123456".into(),
            read_only,
            description: "e2e".into(),
        }],
    };
    Arc::new(Proxy::open(base, &config).expect("代理应可构建"))
}

/// 后台线程起 HTTP 服务（走真实路由 + WAF），返回监听地址。
fn serve(proxy: Arc<Proxy>) -> String {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("运行时");
        runtime.block_on(async move {
            let server = HttpServer::bind("127.0.0.1:0").await.expect("绑定");
            let addr = server.local_addr().expect("本地地址").to_string();
            let _ = tx.send(addr);
            let handler = pek_rdbproxy::panel::build_router(proxy).into_handler();
            let _ = server.serve_with(handler, Default::default()).await;
        });
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("服务应在 10 秒内就绪")
}

/// 极简 HTTP 请求（返回状态码与正文）。
fn http(
    addr: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = std::net::TcpStream::connect(addr).expect("连接");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok();
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let payload = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(request.as_bytes()).expect("发送");
    let mut text = String::new();
    stream.read_to_string(&mut text).expect("接收");
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

#[test]
fn forwards_queries_and_blocks_writes_in_read_only() {
    let base = temp_dir("ro");
    let proxy = build_proxy(&base, true, "");
    let addr = serve(proxy);

    let client = DbClient::new(format!("http://{addr}"), "demo", Some("tk-test-123456"));
    let info = client.login().expect("登录应成功");
    assert!(
        info.db_type.eq_ignore_ascii_case("sqlite"),
        "登录应返回 C# 规范类型名，实际 {}",
        info.db_type
    );

    let set = client
        .query_rowset("SELECT Id, Name FROM t", None)
        .expect("查询应成功");
    assert_eq!(set.rows.len(), 1, "应查到 1 行");

    // 只读护栏
    let err = client
        .execute("INSERT INTO t (Id, Name) VALUES (2, 'x')", None)
        .expect_err("只读模式下应拒绝写入");
    assert!(err.to_string().contains("只读"), "错误信息应说明只读：{err}");

    // 令牌错误 / 数据源不存在
    assert!(
        DbClient::new(format!("http://{addr}"), "demo", Some("wrong"))
            .login()
            .is_err(),
        "错误令牌应登录失败"
    );
    assert!(
        DbClient::new(format!("http://{addr}"), "nope", Some("tk-test-123456"))
            .login()
            .is_err(),
        "不存在的数据源应失败"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn panel_login_uses_jwt_and_crud_works() {
    let base = temp_dir("panel");
    let hash = pek_rdbproxy::proxy::hash_password("pass-123");
    let proxy = build_proxy(&base, false, &hash);
    let addr = serve(Arc::clone(&proxy));

    // 未登录：LocalOnly + 本机回环 → 面板 API 免登录
    let (status, body) = http(&addr, "GET", "/api/status", None, None);
    assert_eq!(status, 200, "回环应免登录：{body}");
    assert!(body.contains("\"sources\""), "应返回数据源列表：{body}");
    assert!(!body.contains("tk-test-123456"), "不应回显数据源令牌");

    // 登录：错误密码 → 401
    let (status, _) = http(
        &addr,
        "POST",
        "/api/login",
        None,
        Some(r#"{"user":"admin","password":"wrong"}"#),
    );
    assert_eq!(status, 401, "错误密码应 401");

    // 登录：正确密码 → JWT
    let (status, body) = http(
        &addr,
        "POST",
        "/api/login",
        None,
        Some(r#"{"user":"admin","password":"pass-123"}"#),
    );
    assert_eq!(status, 200, "应登录成功：{body}");
    let token = json_field(&body, "token");
    assert_eq!(token.split('.').count(), 3, "令牌应是 JWT（三段）：{token}");

    // 带 JWT 访问面板
    let (status, body) = http(&addr, "GET", "/api/status", Some(&token), None);
    assert_eq!(status, 200);
    assert!(body.contains("\"name\":\"demo\""), "{body}");

    // 新增数据源（写回配置）
    let new_db = base.join("second.db");
    // Windows 路径含反斜杠：JSON 字符串里必须用正斜杠（否则 `\U` 是非法转义）
    let new_db_text = new_db.display().to_string().replace('\\', "/");
    let payload = format!(
        r#"{{"name":"second","connection":"Data Source={new_db_text};Provider=SQLite","token":"tk-second-9999","readOnly":true,"description":"第二个"}}"#
    );
    let (status, body) = http(&addr, "POST", "/api/datasource", Some(&token), Some(&payload));
    assert_eq!(status, 200, "新增数据源应成功：{body}");
    assert!(body.contains("\"name\":\"second\""), "{body}");
    let config_text =
        std::fs::read_to_string(base.join("Config/Proxy.toml")).expect("配置应被写回");
    assert!(config_text.contains("second"), "配置里应有新数据源：{config_text}");
    assert!(
        config_text.contains("# ──"), 
        "写回应保留模板注释：{config_text}"
    );

    // 切换只读
    let (status, body) = http(
        &addr,
        "POST",
        "/api/readonly",
        Some(&token),
        Some(r#"{"db":"second","readOnly":false}"#),
    );
    assert_eq!(status, 200, "{body}");

    // 删除数据源
    let (status, body) = http(
        &addr,
        "POST",
        "/api/datasource/delete",
        Some(&token),
        Some(r#"{"name":"second"}"#),
    );
    assert_eq!(status, 200, "{body}");
    let (_, body) = http(&addr, "GET", "/api/status", Some(&token), None);
    assert!(!body.contains("\"second\""), "删除后不应再出现：{body}");

    // 审计：先通过代理协议产生记录（面板 API 不写审计）
    let client = DbClient::new(format!("http://{addr}"), "demo", Some("tk-test-123456"));
    client.login().expect("协议登录");
    let _ = client.query_count("t");
    let (status, body) = http(&addr, "GET", "/api/audit?n=20", Some(&token), None);
    assert_eq!(status, 200);
    assert!(body.contains("\"op\":\"Login\""), "审计应含协议调用：{body}");
    assert!(body.contains("****"), "审计里令牌应是指纹：{body}");

    let _ = std::fs::remove_dir_all(&base);
}

/// 从 JSON 文本里取字符串字段（避免为测试引入 serde_json 依赖链以外的解析）。
fn json_field(body: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    body.split_once(&needle)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value.to_string())
        .unwrap_or_default()
}
