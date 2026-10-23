//! Pek.RDbProxy —— 本地数据库代理（库形态，供 bin 与集成测试共用）。
//!
//! **解决的问题**：项目/代码里不再出现真实数据库连接串（避免被 AI 或协作方读走），
//! 只写代理地址 + 令牌；真实连接串只留在本机 `Config/Proxy.toml`。
//!
//! 协议完全复用 Pek.RCode 的 `/Db/*`（与 C# `DbController` 一致），
//! Rust / C# 客户端都能直连。

pub mod config;
pub mod panel;
pub mod proxy;

/// 程序基目录（`PEK_RDBPROXY_BASE` 环境变量可覆盖）。
pub fn base_dir() -> std::path::PathBuf {
    dhrust::io::base_dir(&["PEK_RDBPROXY_BASE"])
}

/// 版本号。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
