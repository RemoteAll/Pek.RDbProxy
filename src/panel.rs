//! Web 面板（单文件 HTML，`include_bytes!` 内嵌，无外部依赖）。
//!
//! **注意**：嵌入资源改动不会触发 cargo 重编 —— 改完 `res/index.html` 后请 touch 本文件
//! （或 `cargo clean -p pek-rdbproxy`）再构建。

/// 面板页面（内嵌）。
pub const INDEX_HTML: &str = include_str!("../res/index.html");
