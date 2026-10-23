//! 端到端：真实 HTTP 宿主 + `DbClient`（network 协议）+ 只读护栏 + 面板接口。

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use dhrust::net::http::{HttpOutcome, HttpServer, HttpServerOptions, handler};
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
fn build_proxy(base: &Path, read_only: bool, admin_token: &str) -> Arc<Proxy> {
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
            admin_token: admin_token.into(),
            allow_remote: false,
        },
        databases: vec![DatabaseSection {
            name: "demo".into(),
            connection: format!("Data Source={};Provider=SQLite", db.display()),
            token: "tk-test-123456".into(),
            read_only,
            description: "e2e".into(),
        }],
    };
    Arc::new(Proxy::open(&config).expect("代理应可构建"))
}

/// 后台线程起 HTTP 服务，返回监听地址。
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
            let handler = handler(move |req| {
                let proxy = Arc::clone(&proxy);
                async move { HttpOutcome::Response(proxy.handle(req)) }
            });
            let options = HttpServerOptions {
                thread_per_connection: true,
                ..Default::default()
            };
            let _ = server.serve_with(handler, options).await;
        });
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("服务应在 10 秒内就绪")
}

/// 极简 HTTP GET（取完整响应文本）。
fn http_get(addr: &str, path: &str, bearer: Option<&str>) -> String {
    let mut stream = std::net::TcpStream::connect(addr).expect("连接");
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Connection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).expect("发送");
    let mut text = String::new();
    stream.read_to_string(&mut text).expect("接收");
    text
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
    assert_eq!(
        set.rows[0].get_by_name("Name").and_then(|v| v.as_str().map(str::to_string)),
        Some("hello".to_string())
    );

    // 只读护栏：写操作必须被拒
    let err = client
        .execute("INSERT INTO t (Id, Name) VALUES (2, 'x')", None)
        .expect_err("只读模式下应拒绝写入");
    assert!(err.to_string().contains("只读"), "错误信息应说明只读：{err}");

    // 令牌错误 → 登录失败
    let bad = DbClient::new(format!("http://{addr}"), "demo", Some("wrong-token"));
    assert!(bad.login().is_err(), "错误令牌应登录失败");

    // 数据源不存在 → 404
    let missing = DbClient::new(format!("http://{addr}"), "nope", Some("tk-test-123456"));
    assert!(missing.login().is_err(), "不存在的数据源应失败");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn writes_pass_when_not_read_only_and_panel_reports() {
    let base = temp_dir("rw");
    let proxy = build_proxy(&base, false, "admin-tk");
    let addr = serve(Arc::clone(&proxy));

    let client = DbClient::new(format!("http://{addr}"), "demo", Some("tk-test-123456"));
    client.login().expect("登录");
    let affected = client
        .execute("INSERT INTO t (Id, Name) VALUES (2, 'y')", None)
        .expect("非只读应允许写入");
    assert_eq!(affected, 1);
    let total = client.query_count("t").expect("计数");
    assert_eq!(total, 2);

    // 面板：无令牌 401
    let unauthorized = http_get(&addr, "/api/status", None);
    assert!(unauthorized.contains("401"), "无令牌应 401：{unauthorized}");

    // 面板：带令牌可读，且包含数据源与统计（连接串已脱敏）
    let authorized = http_get(&addr, "/api/status", Some("admin-tk"));
    assert!(authorized.contains("\"sources\""), "应返回数据源列表");
    assert!(authorized.contains("\"name\":\"demo\""));
    assert!(
        !authorized.contains("tk-test-123456"),
        "面板不应回显数据源令牌"
    );

    // 审计：应有查询/写入记录
    let audit = http_get(&addr, "/api/audit?n=20", Some("admin-tk"));
    assert!(audit.contains("\"op\":\"Execute\""), "审计应记录 Execute：{audit}");
    assert!(audit.contains("****"), "审计里令牌应是指纹");

    // 首页（面板 HTML）
    let page = http_get(&addr, "/", None);
    assert!(page.contains("Pek.RDbProxy"), "首页应返回面板 HTML");

    let _ = std::fs::remove_dir_all(&base);
}
