//! Pek.RDbProxy —— 本地数据库代理。
//!
//! **解决的问题**：项目/代码里不再出现真实数据库连接串（避免被 AI 或协作方读走），
//! 只写代理地址 + 令牌；真实连接串只留在本机 `Config/Proxy.toml`。
//!
//! ```text
//! # 项目里的连接串（不含任何真实凭据）
//! Server=http://127.0.0.1:5599;Database=demo;Password=tk-xxxx;provider=network
//! ```
//!
//! 协议完全复用 Pek.RCode 的 `/Db/*`（与 C# `DbController` 一致），
//! 所以 **Rust / C# 客户端都能直连**，本项目只做「多数据源 + 鉴权 + 只读护栏 + 审计 + 面板」。

use std::sync::Arc;

use dhrust::logs::LogLevel;
use dhrust::net::http::{HttpOutcome, HttpServer, HttpServerOptions, handler};

use pek_rdbproxy::{VERSION, base_dir, config, proxy};
use proxy::Proxy;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = base_dir();
    let _ = std::fs::create_dir_all(&base);

    // 启动类命令需要日志文件；纯查询类命令只打印到控制台
    let text_command = matches!(
        args.first().map(String::as_str),
        Some("-init" | "--init" | "-status" | "--status" | "-token" | "--token" | "-encrypt" | "--encrypt" | "-help" | "--help" | "-h")
    );
    dhrust::logs::init_console_and_file(base.join("Log"), !text_command, LogLevel::Info);

    let code = match args.first().map(String::as_str) {
        Some("-help" | "--help" | "-h") => {
            print_help();
            0
        }
        Some("-init" | "--init") => init_command(&base),
        Some("-status" | "--status") => status_command(&base),
        Some("-token" | "--token") => {
            println!("{}", dhrust::random::token());
            0
        }
        Some("-encrypt" | "--encrypt") => encrypt_command(&base, args.get(1).map(String::as_str)),
        Some("-run" | "--run") | None => run_command(base),
        Some(other) => {
            eprintln!("未知参数：{other}（用 -help 查看用法）");
            2
        }
    };
    std::process::exit(code);
}

/// 打印用法。
fn print_help() {
    println!(
        "Pek.RDbProxy v{VERSION} —— 本地数据库代理（真实连接串留在代理端）\n\
         \n\
         用法：\n\
         \x20 pek-rdbproxy                启动代理（读 Config/Proxy.toml）\n\
         \x20 pek-rdbproxy -init          生成配置模板（不覆盖已有配置）\n\
         \x20 pek-rdbproxy -status        查看配置概览与连通性（连接串脱敏）\n\
         \x20 pek-rdbproxy -token         生成一个随机令牌（给项目侧 Password= 用）\n\
         \x20 pek-rdbproxy -encrypt <连接串>  加密连接串（密钥存 Config/.key）\n\
         \n\
         项目侧连接串：\n\
         \x20 Server=http://127.0.0.1:<Port>;Database=<Name>;Password=<Token>;provider=network"
    );
}

/// `-init`：生成模板。
fn init_command(base: &std::path::Path) -> i32 {
    match config::write_template(base) {
        Ok(path) => {
            println!("已生成配置模板：{}", path.display());
            println!("请填入真实连接串与令牌后再启动：pek-rdbproxy");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

/// `-status`：配置概览 + 连通性（**不打印真实连接串**）。
fn status_command(base: &std::path::Path) -> i32 {
    let cfg = match config::load(base) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("Pek.RDbProxy v{VERSION}");
    println!("基目录：{}", base.display());
    println!(
        "监听：{}:{}（AllowRemote={}）",
        cfg.server.host, cfg.server.port, cfg.server.allow_remote
    );
    println!(
        "面板令牌：{}",
        if cfg.server.admin_token.is_empty() {
            "未设置（面板仅本机可访问）".to_string()
        } else {
            "已设置".to_string()
        }
    );
    println!("数据源 {} 个：", cfg.databases.len());
    let proxy = match Proxy::open(&cfg) {
        Ok(proxy) => proxy,
        Err(e) => {
            eprintln!("构建失败：{e}");
            return 1;
        }
    };
    for source in proxy.source_list() {
        println!(
            "  {:<16} {:<12} 只读={:<5} 令牌={:<5} {}",
            source.name,
            if source.kind.is_empty() { "-" } else { source.kind.as_str() },
            if source.read_only { "是" } else { "否" },
            if source.token_set { "已设" } else { "无" },
            if source.available {
                "可用".to_string()
            } else {
                format!("不可用：{}", source.error)
            }
        );
        println!("       {}", source.connection);
    }
    0
}

/// `-encrypt`：加密连接串。
fn encrypt_command(base: &std::path::Path, plain: Option<&str>) -> i32 {
    let Some(plain) = plain.filter(|text| !text.trim().is_empty()) else {
        eprintln!("用法：pek-rdbproxy -encrypt \"Server=...;Provider=MySQL;...\"");
        return 2;
    };
    let key = match ensure_key(base) {
        Ok(key) => key,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match dhrust::secret::encrypt(plain, &key) {
        Ok(encrypted) => {
            println!("{encrypted}");
            println!();
            println!("把上面整行填到 Config/Proxy.toml 的 Connection 即可；密钥在 Config/.key（勿入库）。");
            0
        }
        Err(e) => {
            eprintln!("加密失败：{e}");
            1
        }
    }
}

/// 读取或生成 `Config/.key`（仅存随机种子文本）。
fn ensure_key(base: &std::path::Path) -> Result<[u8; 32], String> {
    let path = base.join("Config/.key");
    if path.is_file() {
        return proxy::load_key();
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let seed = dhrust::random::hex(32);
    std::fs::write(&path, &seed).map_err(|e| format!("写入密钥失败：{e}"))?;
    println!("已生成密钥：{}", path.display());
    Ok(dhrust::secret::key_from_seed(&seed))
}

/// 启动代理服务。
fn run_command(base: std::path::PathBuf) -> i32 {
    let cfg = match config::load(&base) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let proxy = match Proxy::open(&cfg) {
        Ok(proxy) => Arc::new(proxy),
        Err(e) => {
            eprintln!("启动失败：{e}");
            return 1;
        }
    };
    let host = cfg.server.host.clone();
    let port = cfg.server.port;
    let address = format!("{host}:{port}");

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("创建运行时失败：{e}");
            return 1;
        }
    };

    runtime.block_on(async move {
        let server = match HttpServer::bind(address.as_str()).await {
            Ok(server) => server,
            Err(e) => {
                eprintln!("监听失败（{address}）：{e}");
                return 1;
            }
        };
        let local = server
            .local_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_else(|_| address.clone());
        println!("Pek.RDbProxy v{VERSION} 已启动：http://{local}");
        println!("面板：http://{local}/");
        println!(
            "数据源：{}（{}）",
            proxy.source_count(),
            proxy.source_names().join(", ")
        );
        println!(
            "项目侧连接串：Server=http://{local};Database=<Name>;Password=<Token>;provider=network"
        );

        let handler = handler(move |req| {
            let proxy = Arc::clone(&proxy);
            async move { HttpOutcome::Response(proxy.handle(req)) }
        });
        // 每连接一线程：SQL 是同步阻塞调用，独立线程不会互相拖慢
        let options = HttpServerOptions {
            thread_per_connection: true,
            ..Default::default()
        };
        if let Err(e) = server.serve_with(handler, options).await {
            eprintln!("服务异常退出：{e}");
            return 1;
        }
        0
    })
}
