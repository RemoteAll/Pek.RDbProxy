//! Pek.RDbProxy CLI（对齐 Pek 系规范：服务化 + 面板账号 + 自升级目录 + 星尘托管注册）。
//!
//! **解决的问题**：项目/代码里不再出现真实数据库连接串（避免被 AI 或协作方读走），
//! 只写代理地址 + 令牌；真实连接串只留在本机 `Config/Proxy.toml`。
//!
//! 协议完全复用 Pek.RCode 的 `/Db/*`（与 C# `DbController` 一致），Rust / C# 客户端都能直连。

use std::sync::Arc;

use dhrust::logs::LogLevel;
use dhrust::net::http::{HttpServer, HttpServerOptions};

use pek_rdbproxy::proxy::{Proxy, ServerPatch};
use pek_rdbproxy::{VERSION, base_dir, config, panel, service};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let base = base_dir();
    let _ = std::fs::create_dir_all(&base);

    let text_only = matches!(
        args.first().map(String::as_str),
        Some(
            "-init"
                | "--init"
                | "-status"
                | "--status"
                | "-token"
                | "--token"
                | "-encrypt"
                | "--encrypt"
                | "-install"
                | "--install"
                | "-uninstall"
                | "--uninstall"
                | "-start"
                | "--start"
                | "-stop"
                | "--stop"
                | "-restart"
                | "--restart"
                | "-RegisterAgent"
                | "--RegisterAgent"
                | "-help"
                | "--help"
                | "-h"
        )
    );
    dhrust::logs::init_console_and_file(base.join("Log"), !text_only, LogLevel::Info);

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
        Some("-resetPassword" | "--resetPassword") => {
            reset_password_command(&base, args.get(1).map(String::as_str))
        }
        Some("-install" | "--install") => service_command(&base, "install"),
        Some("-uninstall" | "--uninstall") => service_command(&base, "uninstall"),
        Some("-start" | "--start") => service_command(&base, "start"),
        Some("-stop" | "--stop") => service_command(&base, "stop"),
        Some("-restart" | "--restart") => service_command(&base, "restart"),
        Some("-RegisterAgent" | "--RegisterAgent") => register_agent_command(&base),
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
         \x20 pek-rdbproxy -status        查看配置、服务状态与连通性（连接串脱敏）\n\
         \x20 pek-rdbproxy -token         生成一个随机令牌（项目侧 Password= 用）\n\
         \x20 pek-rdbproxy -encrypt <连接串>  加密连接串（密钥存 Config/.key）\n\
         \n\
         服务化（星尘托管 / systemd / Windows 服务）：\n\
         \x20 -install / -uninstall / -start / -stop / -restart\n\
         \x20 -RegisterAgent   注册到星尘（Pek.RAgent）托管（Mode=11 影子模式，覆盖即升级）\n\
         \n\
         项目侧连接串：\n\
         \x20 Server=http://127.0.0.1:<Port>;Database=<Name>;Password=<Token>;provider=network\n\
         \n\
         自升级：把新版本程序丢进 Update/ 目录，运行中的实例检测到后自动替换并重启。"
    );
}

/// `-init`：生成配置模板。
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

/// `-status`：配置概览 + 服务状态 + 连通性（**不打印真实连接串**）。
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
    let state = service::manager(base).query();
    println!("服务：{}（{}）", service::DISPLAY_NAME, state.text());
    println!(
        "监听：{}:{}（AllowRemote={}）",
        cfg.server.host, cfg.server.port, cfg.server.allow_remote
    );
    println!(
        "面板账号：{}（鉴权级别 {}，密码{}）",
        cfg.server.web_user_name,
        cfg.server.web_auth_level,
        if cfg.server.web_password_hash.is_empty() {
            "未初始化（启动时自动设为 admin/admin）"
        } else {
            "已设置"
        }
    );
    let proxy = match Proxy::open(base, &cfg) {
        Ok(proxy) => proxy,
        Err(e) => {
            eprintln!("构建失败：{e}");
            return 1;
        }
    };
    println!("数据源 {} 个：", proxy.source_count());
    for source in proxy.source_list() {
        println!(
            "  {:<16} {:<12} 只读={:<2} 令牌={:<4} {}",
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
    let update_dir = base.join("Update");
    if update_dir.is_dir() {
        let pending = std::fs::read_dir(&update_dir)
            .map(|dir| dir.filter_map(Result::ok).count())
            .unwrap_or(0);
        if pending > 0 {
            println!("Update/ 有 {pending} 个待升级文件（运行中的实例会自动替换并重启）");
        }
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
            println!(
                "把上面整行填到 Config/Proxy.toml 的 Connection 即可；密钥在 Config/.key（勿入库）。"
            );
            0
        }
        Err(e) => {
            eprintln!("加密失败：{e}");
            1
        }
    }
}

/// 读取或生成 `Config/.key`。
fn ensure_key(base: &std::path::Path) -> Result<[u8; 32], String> {
    let path = base.join("Config/.key");
    if path.is_file() {
        return pek_rdbproxy::proxy::load_key();
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let seed = dhrust::random::hex(32);
    std::fs::write(&path, &seed).map_err(|e| format!("写入密钥失败：{e}"))?;
    println!("已生成密钥：{}", path.display());
    Ok(dhrust::secret::key_from_seed(&seed))
}

/// `-resetPassword <新密码>`：重置面板密码（忘记密码时的兜底，需重启服务）。
fn reset_password_command(base: &std::path::Path, password: Option<&str>) -> i32 {
    let Some(password) = password.filter(|p| !p.trim().is_empty()) else {
        eprintln!("用法：pek-rdbproxy -resetPassword \"<新密码>\"");
        eprintln!("规则：至少 10 位，且包含大写/小写/数字/符号中的至少三类");
        return 2;
    };
    let mut cfg = match pek_rdbproxy::config::load(base) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("读取配置失败：{e}");
            return 1;
        }
    };
    if let Err(e) = pek_rdbproxy::proxy::password_strength(&cfg.server.web_user_name, password) {
        eprintln!("密码不符合规则：{e}");
        return 1;
    }
    cfg.server.web_password_hash = pek_rdbproxy::proxy::hash_password(password);
    match pek_rdbproxy::config::save(base, &cfg) {
        Ok(path) => {
            println!("面板密码已重置（用户名 {}）→ {}", cfg.server.web_user_name, path.display());
            println!("重启服务后生效：pek-rdbproxy -restart");
            0
        }
        Err(e) => {
            eprintln!("保存失败：{e}");
            1
        }
    }
}

/// 服务化命令（install/uninstall/start/stop/restart）。
fn service_command(base: &std::path::Path, action: &str) -> i32 {
    let manager = service::manager(base);
    let result = match action {
        "install" => manager.install(true),
        "uninstall" => manager.uninstall(true),
        "start" => manager.start(),
        "stop" => manager.stop(),
        "restart" => manager.restart(),
        _ => Err(format!("未知服务动作：{action}")),
    };
    match result {
        Ok(()) => {
            println!(
                "服务 {action} 成功：{}（{}）",
                service::SERVICE_NAME,
                manager.query().text()
            );
            0
        }
        Err(e) => {
            eprintln!("服务 {action} 失败：{e}");
            1
        }
    }
}

/// `-RegisterAgent`：注册到星尘托管（Mode=11 影子模式，覆盖程序即自动重启）。
fn register_agent_command(_base: &std::path::Path) -> i32 {
    let Some(agent_config) = find_agent_config() else {
        eprintln!(
            "未找到星尘配置文件（StarAgent.config）。可用环境变量 PEK_RAGENT_CONFIG 指定，\
             或先安装 Pek.RAgent。"
        );
        return 1;
    };
    // 程序路径与工作目录由 `staragent::register` 取 `current_exe()` 及其父目录
    let entry = dhrust::staragent::Entry {
        name: service::SERVICE_NAME,
        base_env: Some("PEK_RDBPROXY_BASE"),
        health_check: "",
    };
    match dhrust::staragent::register(&agent_config, &entry) {
        Ok(message) => {
            println!("{message}");
            println!("配置：{}", agent_config.display());
            println!("星尘以 Mode=11 影子模式托管：覆盖程序文件即自动重启（无需停服）");
            0
        }
        Err(e) => {
            eprintln!("注册失败：{e}");
            1
        }
    }
}

/// 查找星尘配置（环境变量优先，其次常见安装目录）。
fn find_agent_config() -> Option<std::path::PathBuf> {
    if let Ok(path) = std::env::var("PEK_RAGENT_CONFIG") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let candidates = [
        r"C:\StarAgent\Config\StarAgent.config",
        "/www/Agent/Config/StarAgent.config",
        "/www/Stardust/Agent/Config/StarAgent.config",
        "/opt/StarAgent/Config/StarAgent.config",
    ];
    candidates
        .iter()
        .map(std::path::PathBuf::from)
        .find(|path| path.is_file())
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
    let proxy = match Proxy::open(&base, &cfg) {
        Ok(proxy) => Arc::new(proxy),
        Err(e) => {
            eprintln!("启动失败：{e}");
            return 1;
        }
    };

    // 面板账号首次初始化（对齐 Pek.RAgent：默认 admin/admin，面板横幅提示改密）
    if proxy.server_config().web_password_hash.is_empty() {
        let patch = ServerPatch {
            password: Some("admin".to_string()),
            user_name: Some("admin".to_string()),
            ..Default::default()
        };
        match proxy.update_server(patch) {
            Ok(_) => dhrust::warn!("面板账号已初始化为 admin/admin —— 请尽快在「设置」里改密"),
            Err(e) => dhrust::warn!("面板账号初始化失败：{e}"),
        }
    }

    // 自升级：Update/ 目录热检测（覆盖即升级）
    spawn_upgrade_watcher(base.clone());

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
        let router = panel::build_router(Arc::clone(&proxy));
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

        // 每连接一线程：SQL 是同步阻塞调用，独立线程不会互相拖慢
        let options = HttpServerOptions {
            thread_per_connection: true,
            ..Default::default()
        };
        if let Err(e) = server.serve_with(router.into_handler(), options).await {
            eprintln!("服务异常退出：{e}");
            return 1;
        }
        0
    })
}

/// `Update/` 目录监视：静置 10 秒后原子替换自身并退出（由服务管理器 / 星尘拉起新版本）。
fn spawn_upgrade_watcher(base: std::path::PathBuf) {
    std::thread::Builder::new()
        .name("rdbproxy-upgrade".into())
        .spawn(move || {
            let update_dir = base.join("Update");
            let _ = std::fs::create_dir_all(&update_dir);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(10));
                let Ok(entries) = std::fs::read_dir(&update_dir) else {
                    continue;
                };
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if !path.is_file() {
                        continue;
                    }
                    // 静置 10 秒（防正在上传）
                    let stable = entry
                        .metadata()
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|elapsed| elapsed >= std::time::Duration::from_secs(10));
                    if !stable {
                        continue;
                    }
                    if let Err(e) = install_update(&path) {
                        dhrust::error!("自升级失败：{e}");
                        let failed = path.with_extension("failed");
                        let _ = std::fs::rename(&path, failed);
                    }
                }
            }
        })
        .ok();
}

/// 用 `Update/` 里的文件替换当前程序（原子改名），随后退出让服务管理器拉起。
fn install_update(new_file: &std::path::Path) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| format!("取当前程序路径失败：{e}"))?;
    let backup = current.with_extension("old");
    let _ = std::fs::remove_file(&backup);
    // Unix：rename 原子替换（运行中的文件允许被替换）；Windows：先改名让位
    #[cfg(windows)]
    {
        std::fs::rename(&current, &backup).map_err(|e| format!("改名让位失败：{e}"))?;
    }
    std::fs::rename(new_file, &current).map_err(|e| format!("替换程序失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&current) {
            let mut perm = meta.permissions();
            perm.set_mode(0o755);
            let _ = std::fs::set_permissions(&current, perm);
        }
    }
    dhrust::info!(
        "自升级完成：{} → {}，准备重启",
        new_file.display(),
        current.display()
    );
    // 日志落盘后退出（服务管理器 / 星尘影子模式会拉起新版本）
    dhrust::logs::flush();
    std::process::exit(0);
}
