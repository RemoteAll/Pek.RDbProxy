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
            panel_local_only: false, // 测试走回环；显式关闭避免与断言耦合
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
        // 测试只用 SQLite（内嵌驱动，永不走组件源）
        driver_store: Default::default(),
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
    // 触发字段策略备注（默认 enforce 白名单：未登记列会被拦截）
    let _ = client.query_rowset("SELECT Id, Name FROM t", None);
    let (status, body) = http(&addr, "GET", "/api/audit?n=20", Some(&token), None);
    assert_eq!(status, 200);
    assert!(body.contains("\"op\":\"Login\""), "审计应含协议调用：{body}");
    assert!(body.contains("****"), "审计里令牌应是指纹：{body}");
    assert!(
        body.contains("\"sql\":\"SELECT Id, Name FROM t\""),
        "审计必须记录**实际 SQL**：{body}"
    );
    assert!(
        body.contains("\"note\":\"字段已拦截"),
        "字段策略备注要单独进 note（不能顶掉 SQL）：{body}"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// 字段级安全：`mask` 保留列但值变 `***`；`deny` 让列从结果集消失（`SELECT *` 也一样）。
#[test]
fn field_policy_masks_and_blocks_columns() {
    use pek_rdbproxy::policy::{Action, Mode, Rule};

    let base = temp_dir("field-policy");
    let proxy = build_proxy(&base, true, "");
    let addr = serve(Arc::clone(&proxy));
    let client = DbClient::new(format!("http://{addr}"), "demo", Some("tk-test-123456"));
    client.login().expect("登录");

    // 默认 enforce = 白名单：策略为空时所有列都算"未登记" → 一律不返回
    let set = client
        .query_rowset("SELECT Id, Name FROM t", None)
        .expect("查询应成功");
    assert!(
        set.columns.is_empty(),
        "未登记列默认不返回：{:?}",
        set.columns
    );

    let mut policy = proxy.policy_snapshot();
    policy.mode = Mode::Enforce;
    // enforce 下未登记列默认不返回，所以常规列要显式 allow
    policy.upsert(Rule {
        db: "demo".into(),
        column: "Id".into(),
        action: Action::Allow,
        note: String::new(),
    });
    policy.upsert(Rule {
        db: "demo".into(),
        column: "Name".into(),
        action: Action::Mask,
        note: "敏感列".into(),
    });
    proxy.save_policy(policy).expect("保存策略");

    let set = client
        .query_rowset("SELECT Id, Name FROM t", None)
        .expect("查询应成功");
    assert_eq!(set.columns.len(), 2, "掩码列应保留：{:?}", set.columns);
    assert_eq!(
        set.rows[0].get_by_name("Name").and_then(|v| v.as_str()),
        Some("***"),
        "掩码值应为 ***"
    );
    assert!(
        set.rows[0].get_by_name("Id").is_some(),
        "非掩码列应原样保留"
    );

    // 改为拒绝：列从结果集消失（含 SELECT *）
    let mut policy = proxy.policy_snapshot();
    policy.upsert(Rule {
        db: "demo".into(),
        column: "Name".into(),
        action: Action::Deny,
        note: String::new(),
    });
    proxy.save_policy(policy).expect("保存策略");
    let set = client
        .query_rowset("SELECT * FROM t", None)
        .expect("查询应成功");
    assert_eq!(set.columns.len(), 1, "拒绝列应消失：{:?}", set.columns);
    assert!(
        set.rows[0].get_by_name("Name").is_none(),
        "SELECT * 也拿不到被拒绝的列"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// `provider=network` 真实可用：`Dal::open` + 会话查询经代理转发到 SQLite。
#[test]
fn dal_open_with_network_provider_executes() {
    use pek_rdbproxy::policy::{Action, Rule};

    let base = temp_dir("network-dal");
    let proxy = build_proxy(&base, true, "");
    // 默认严格白名单：先允许本测试要读的两列
    let mut policy = proxy.policy_snapshot();
    for column in ["Id", "Name"] {
        policy.upsert(Rule {
            db: "demo".into(),
            column: column.into(),
            action: Action::Allow,
            note: String::new(),
        });
    }
    proxy.save_policy(policy).expect("保存策略");
    let addr = serve(Arc::clone(&proxy));

    let conn = format!("Server=http://{addr};Database=demo;Password=tk-test-123456;provider=network");
    let dal = Dal::open(&conn).expect("network 连接串应可打开");
    let mut session = dal.open_session().expect("应能建会话");
    let set = session
        .query("SELECT Id, Name FROM t", &[])
        .expect("经代理查询应成功");
    assert_eq!(set.rows.len(), 1, "应查到 1 行");
    assert_eq!(
        set.rows[0].get_by_name("Name").and_then(|v| v.as_str()),
        Some("hello"),
        "值应原样返回"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// 首启默认账号：`admin/admin` 本身不满足强度规则，但必须能建（登录后立刻强制升级）。
#[test]
fn default_account_is_created_even_though_admin_is_weak() {
    let base = temp_dir("default-account");
    let proxy = build_proxy(&base, true, "");
    assert!(proxy.ensure_default_account().expect("首启应可创建"));
    assert!(proxy.authenticate("admin", "admin"), "默认账号应可登录");
    assert!(proxy.uses_default_password());
    assert!(
        !proxy.ensure_default_account().expect("第二次应为未创建"),
        "已有密码时不应覆盖"
    );

    let upgraded = proxy
        .upgrade_weak_password()
        .expect("应自动升级")
        .expect("应返回新密码");
    assert!(upgraded.len() >= 10, "新密码应满足长度：{upgraded}");
    assert!(!proxy.uses_default_password(), "升级后不应再是默认密码");
    assert!(!proxy.authenticate("admin", "admin"), "旧默认密码应失效");
    assert!(proxy.authenticate("admin", &upgraded));

    let _ = std::fs::remove_dir_all(&base);
}

/// 从 JSON 文本里取字符串字段（避免为测试引入 serde_json 依赖链以外的解析）。
/// 极简 HTTP 请求（二进制请求体，返回状态码与正文字节）。
fn http_bytes(
    addr: &str,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    content_type: &str,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let mut stream = std::net::TcpStream::connect(addr).expect("连接");
    stream.set_read_timeout(Some(Duration::from_secs(20))).ok();
    let auth = bearer
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{auth}Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).expect("发送头");
    stream.write_all(body).expect("发送体");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("接收");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(0);
    let head_text = String::from_utf8_lossy(&raw[..split]).to_string();
    let status = head_text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(0);
    (status, raw[split..].to_vec())
}

fn json_field(body: &str, key: &str) -> String {
    let needle = format!("\"{key}\":\"");
    body.split_once(&needle)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value.to_string())
        .unwrap_or_default()
}

/// 用面板账号登录，取 JWT。
fn login_token(addr: &str, user: &str, password: &str) -> String {
    let body = format!(r#"{{"user":"{user}","password":"{password}"}}"#);
    let (status, text) = http(addr, "POST", "/api/login", None, Some(&body));
    assert_eq!(status, 200, "{user} 应能登录：{text}");
    let token = json_field(&text, "token");
    assert_eq!(token.split('.').count(), 3, "应是 JWT：{token}");
    token
}

/// 工具包托管：上传（需 `settings` 权限）→ 列版本（免登录只读）→ 下载（免登录）→ 内容与 SHA-256 一致。
///
/// 这条链就是「代理上升级工具包 → 工具自己检测并升级」的底座：格式、校验、路由一次盖完。
#[test]
fn tool_package_upload_and_download() {
    let base = temp_dir("toolhub");
    let hash = pek_rdbproxy::proxy::hash_password("pass-123");
    let proxy = build_proxy(&base, false, &hash);
    let addr = serve(Arc::clone(&proxy));
    let admin = login_token(&addr, "admin", "pass-123");

    // 假 PE：`MZ` 魔数 + 填充（`is_executable` 只看魔数）
    let mut exe = vec![b'M', b'Z'];
    exe.extend(std::iter::repeat_n(0u8, 4096));
    let digest = dhrust::sign::sha256_hex(&exe);

    // 注意：本用例的鉴权级别是 `LocalOnly`，回环请求按「内置管理员」放行（本机能改配置就能管理），
    // 所以“未登录上传被拒”要在 `Full` 级别下才成立；这里用**普通账号**来验权限闸门。
    let payload =
        r#"{"name":"ops","password":"p-ops","enabled":true,"permissions":["overview"],"remark":"e2e"}"#;
    let (status, body) = http(&addr, "POST", "/api/user", Some(&admin), Some(payload));
    assert_eq!(status, 200, "建号应成功：{body}");
    let ops = login_token(&addr, "ops", "p-ops");
    let (status, _) = http_bytes(
        &addr,
        "POST",
        "/api/tool/upload/0.9.9",
        Some(&ops),
        "application/octet-stream",
        &exe,
    );
    assert_eq!(status, 403, "无 settings 权限应 403，实际 {status}");

    // 非可执行内容应 400
    let (status, body) = http_bytes(
        &addr,
        "POST",
        "/api/tool/upload/0.9.8",
        Some(&admin),
        "application/octet-stream",
        b"not-a-program",
    );
    assert_eq!(
        status,
        400,
        "非可执行应 400：{}",
        String::from_utf8_lossy(&body)
    );

    // 正常上传
    let (status, body) = http_bytes(
        &addr,
        "POST",
        "/api/tool/upload/0.9.9?notes=e2e%20%E5%B7%A5%E5%85%B7",
        Some(&admin),
        "application/octet-stream",
        &exe,
    );
    assert_eq!(
        status,
        200,
        "上传应成功：{}",
        String::from_utf8_lossy(&body)
    );
    let text = String::from_utf8_lossy(&body).to_string();
    assert!(text.contains("\"version\":\"0.9.9\""), "{text}");
    assert!(text.contains(&digest), "应回传 SHA-256：{text}");

    // 免登录查版本（工具端检查更新用）
    let (status, body) = http(&addr, "GET", "/api/tool", None, None);
    assert_eq!(status, 200, "免登录应可查版本：{body}");
    assert!(body.contains("\"present\":true"), "{body}");
    assert!(body.contains("\"version\":\"0.9.9\""), "{body}");

    // 免登录下载（latest + 指定文件名两种）
    for file in ["latest", "pek-rdbproxy-tool-0.9.9"] {
        let (status, bytes) = http_bytes(
            &addr,
            "GET",
            &format!("/api/tool/download/{file}"),
            None,
            "application/octet-stream",
            b"",
        );
        assert_eq!(status, 200, "下载 {file} 应成功（{} 字节）", bytes.len());
        assert_eq!(bytes, exe, "下载内容应与上传一致（{file}）");
        assert_eq!(dhrust::sign::sha256_hex(&bytes), digest, "SHA-256 应一致");
    }

    // 不存在的包 → 404
    let (status, _) = http_bytes(
        &addr,
        "GET",
        "/api/tool/download/pek-rdbproxy-tool-9.9.9",
        None,
        "application/octet-stream",
        b"",
    );
    assert_eq!(status, 404, "不存在的版本应 404");

    let _ = std::fs::remove_dir_all(&base);
}

/// 按用户授权：`db:read:<源>` 只能查、`db:write:<源>` 才能写，没有 `db:` 权限一律 403；
/// 审计要能看出「是谁在查」；带了无效令牌**不能**静默降级成旧式调用。
#[test]
fn user_permissions_gate_datasource_access() {
    let base = temp_dir("user-perm");
    let hash = pek_rdbproxy::proxy::hash_password("pass-123");
    let proxy = build_proxy(&base, false, &hash);
    let addr = serve(Arc::clone(&proxy));
    let admin = login_token(&addr, "admin", "pass-123");

    // 三个账号：只读 / 读写 / 无数据源权限
    for (user, perms) in [
        ("reader", r#"["overview","db:read:demo"]"#),
        ("writer", r#"["overview","db:write:demo"]"#),
        ("nobody", r#"["overview"]"#),
    ] {
        let payload = format!(
            r#"{{"name":"{user}","password":"p-{user}","enabled":true,"permissions":{perms},"remark":"e2e"}}"#
        );
        let (status, body) = http(&addr, "POST", "/api/user", Some(&admin), Some(&payload));
        assert_eq!(status, 200, "创建用户 {user} 应成功：{body}");
    }

    let reader = login_token(&addr, "reader", "p-reader");
    let writer = login_token(&addr, "writer", "p-writer");
    let nobody = login_token(&addr, "nobody", "p-nobody");

    let target = format!("http://{addr}");

    // 读权限：查询通过
    DbClient::new(&target, "demo", Some("tk-test-123456"))
        .with_bearer(&reader)
        .query_rowset("SELECT Id FROM t", None)
        .expect("db:read:demo 应能查询");

    // 读权限：写被拒，且文案说清缺什么
    let err = DbClient::new(&target, "demo", Some("tk-test-123456"))
        .with_bearer(&reader)
        .execute("INSERT INTO t (Id, Name) VALUES (9, 'x')", None)
        .expect_err("只有读权限应拒绝写入");
    assert!(
        err.to_string().contains("只有读权限"),
        "错误信息应说明只有读权限：{err}"
    );

    // 读写权限：写成功
    DbClient::new(&target, "demo", Some("tk-test-123456"))
        .with_bearer(&writer)
        .execute("INSERT INTO t (Id, Name) VALUES (3, 'w')", None)
        .expect("db:write:demo 应能写入");

    // 无数据源权限：查询也拒绝
    let err = DbClient::new(&target, "demo", Some("tk-test-123456"))
        .with_bearer(&nobody)
        .query_rowset("SELECT Id FROM t", None)
        .expect_err("没有数据源权限应被拒绝");
    assert!(
        err.to_string().contains("没有数据源") && err.to_string().contains("db:read:demo"),
        "错误信息应指明数据源与所需权限：{err}"
    );

    // 无效令牌：不静默降级（否则「去掉令牌就绕过用户限制」）
    assert!(
        DbClient::new(&target, "demo", Some("tk-test-123456"))
            .with_bearer("not-a-jwt")
            .login()
            .is_err(),
        "无效用户令牌应 401"
    );

    // 旧式调用（不带用户令牌）保持原行为
    DbClient::new(&target, "demo", Some("tk-test-123456"))
        .login()
        .expect("不带用户令牌应仍按数据源令牌放行");

    // 审计：读成功 / 写被拒 都要记到人
    let (status, body) = http(&addr, "GET", "/api/audit?n=50", Some(&admin), None);
    assert_eq!(status, 200);
    assert!(
        body.contains("\"user\":\"reader\""),
        "审计应记录发起者：{body}"
    );
    assert!(
        body.contains("\"user\":\"nobody\""),
        "被拒的请求也要能看出是谁：{body}"
    );
    assert!(
        body.contains("\"user\":\"\""),
        "旧式调用 user 为空（面板显示「数据源令牌」）：{body}"
    );

    let _ = std::fs::remove_dir_all(&base);
}
