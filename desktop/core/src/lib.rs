//! Pek.RDbProxy 桌面工具核心（无 UI 依赖，可直接 `cargo test`）。
//!
//! 定位：**一个真实的消费方**——走的是和项目侧
//! `Server=http://<代理>:5599;Database=<数据源名>;Password=<令牌>;provider=network`
//! 完全相同的协议路径（`pek_rcode::db_service::DbClient`），
//! 因此它跑通即说明代理对真实客户端可用。
//!
//! - [`Target`]：连接字符串解析（复用 `pek_rcode::dal::ConnectionString`）
//! - [`login`] / [`query`] / [`execute`] / [`tables`] / [`count`]：单次操作
//! - [`selfcheck`]：一键自检（Login → 读链路 → 查询 → 计数/写入 → 错误信封）
//!
//! 出站请求统一带 [`USER_AGENT`]：服务端 WAF 打开 `block_empty_ua` 时机器客户端不会被拦。

pub mod check;
pub mod client;

pub use check::{selfcheck, CheckItem, CheckStatus, SelfCheckOptions, SelfCheckReport};
pub use client::{
    count, execute, login, query, tables, CountView, ExecuteView, LoginView, QueryView, TablesView,
    Target, USER_AGENT,
};
