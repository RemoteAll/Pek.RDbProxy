//! 协议回归：用最小 HTTP 桩验证「桌面工具发的请求 = 代理协议要求的样子」。
//!
//! 重点两条：
//! 1. 走的是 `POST /Db/Login`、`POST /Db/Query`（与项目侧 `provider=network` 同一协议）；
//! 2. **每个请求都带 `User-Agent: pek-rdbproxy-tool/<版本>`**（服务端 WAF 打开
//!    `block_empty_ua` 时不会被误拦）。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pek_rdbproxy_tool_core::{check, login, query, SelfCheckOptions, Target};

/// 起一个最小 HTTP 桩：依次接受 `replies.len()` 个连接，记录请求原文后回固定应答。
fn stub_server(replies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定回环端口");
    let addr = listener.local_addr().expect("本地地址");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let request = read_request(&mut stream);
            recorder.lock().expect("记录锁").push(request);
            let _ = stream.write_all(reply.as_bytes());
            let _ = stream.flush();
        }
    });
    (format!("http://{addr}"), seen)
}

/// 读到请求头 + 按 `Content-Length` 读完 body。
fn read_request(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buffer).to_string();
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = content_length(&text[..head_end]);
                    if buffer.len() >= head_end + 4 + length {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

fn content_length(head: &str) -> usize {
    for line in head.lines() {
        if let Some((key, value)) = line.split_once(':')
            && key.trim().eq_ignore_ascii_case("content-length")
        {
            return value.trim().parse().unwrap_or(0);
        }
    }
    0
}

fn ok_json(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn target_of(base: &str) -> Target {
    Target::parse(&format!(
        "Server={base};Database=demo;Password=tk-e2e-123456;provider=network"
    ))
    .expect("连接串应可解析")
}

#[test]
fn login_and_query_follow_proxy_protocol_with_user_agent() {
    let (base, seen) = stub_server(vec![
        ok_json(r#"{"code":0,"data":{"DbType":"SQLite","Version":"3.43"}}"#),
        ok_json(r#"{"columns":["Id","Name"],"rows":[[1,"hello"]]}"#),
    ]);
    let target = target_of(&base);

    let info = login(&target).expect("Login 应成功");
    assert_eq!(info.db_type, "SQLite");
    assert_eq!(info.version, "3.43");

    let view = query(&target, "SELECT Id, Name FROM t").expect("Query 应成功");
    assert_eq!(view.columns, vec!["Id".to_string(), "Name".to_string()]);
    assert_eq!(view.row_count, 1);

    let seen = seen.lock().expect("记录锁");
    assert_eq!(seen.len(), 2, "应各发一次 Login / Query");
    assert!(seen[0].starts_with("POST /Db/Login "), "实际：{}", seen[0]);
    assert!(seen[1].starts_with("POST /Db/Query "), "实际：{}", seen[1]);
    for request in seen.iter() {
        let lower = request.to_ascii_lowercase();
        assert!(
            lower.contains("user-agent: pek-rdbproxy-tool/"),
            "每个请求都必须带工具 UA（WAF block_empty_ua 场景）：{request}"
        );
    }
}

#[test]
fn selfcheck_reports_each_step() {
    // Login → GetTables → Query → QueryCount
    let (base, _seen) = stub_server(vec![
        ok_json(r#"{"code":0,"data":{"DbType":"SQLite","Version":"3.43"}}"#),
        ok_json(r#"{"code":0,"data":[{"Name":"t"}]}"#),
        ok_json(r#"{"columns":["One"],"rows":[[1]]}"#),
        ok_json(r#"{"code":0,"data":7}"#),
    ]);
    let target = target_of(&base);
    let options = SelfCheckOptions {
        sql: "SELECT 1".to_string(),
        table: "t".to_string(),
        write_sql: String::new(),
        check_wrong_token: false,
    };

    let report = check::selfcheck(&target, &options);
    assert_eq!(report.passed, 4, "明细：{:#?}", report.items);
    assert_eq!(report.failed, 0);
    assert_eq!(report.skipped, 2, "写操作与错令牌默认跳过");
    assert!(report.ok());
}

#[test]
fn login_failure_is_reported_with_message() {
    // 服务端返回业务错误信封（错令牌的典型形态）
    let (base, _seen) = stub_server(vec![ok_json(r#"{"code":401,"msg":"无效令牌"}"#)]);
    let target = target_of(&base);
    let error = login(&target).expect_err("应返回错误");
    assert!(error.contains("无效令牌"), "错误信息应透传服务端消息：{error}");
}
