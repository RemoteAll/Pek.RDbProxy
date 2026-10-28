//! 开发用：给工作区塞几条**示例数据**（连接 / 片段 / 历史），便于验证界面渲染。
//!
//! ```text
//! cargo run --example ws_seed -- "%APPDATA%\com.peikesmart.rdbproxytool"
//! ```
//!
//! 只写示例条目（名称带「示例」），不会覆盖已有连接（同名同库才会更新）。

use std::path::PathBuf;

use pek_rdbproxy_tool_core::store::{ConnectionInput, SnippetInput, Store};

fn main() -> Result<(), String> {
    let dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| "用法：ws_seed <数据目录>".to_string())?;
    let mut store = Store::open(dir)?;

    store.save_connection(ConnectionInput {
        id: None,
        name: "示例（本地 demo）".to_string(),
        project: "示例项目".to_string(),
        server: "http://127.0.0.1:5599".to_string(),
        db: "demo".to_string(),
        token: "tk-demo-change-me".to_string(),
        note: "面板首启自动生成的示例数据源（只读）".to_string(),
    })?;
    let id = store
        .snapshot()
        .connections
        .iter()
        .find(|c| c.name == "示例（本地 demo）")
        .map(|c| c.id.clone())
        .ok_or_else(|| "示例连接写入失败".to_string())?;

    store.save_snippet(SnippetInput {
        id: None,
        name: "看看都有哪些表".into(),
        sql: "SELECT name FROM sqlite_master WHERE type='table'".into(),
        note: "SQLite 专用；其他库换成对应的系统表查询".into(),
        project: "示例项目".into(),
        connection_id: id.clone(),
    })?;
    store.save_snippet(SnippetInput {
        id: None,
        name: "连通性自检（SELECT 1）".into(),
        sql: "SELECT 1 AS One".into(),
        note: "任何库都能跑，确认令牌/数据源名是否正确".into(),
        project: String::new(),
        connection_id: String::new(),
    })?;

    store.push_history("SELECT 1 AS One", &id, true, 1, 2)?;
    store.push_history(
        "SELECT name FROM sqlite_master WHERE type='table'",
        &id,
        true,
        0,
        3,
    )?;

    println!("已写入示例工作区：{}", store.dir().display());
    Ok(())
}
