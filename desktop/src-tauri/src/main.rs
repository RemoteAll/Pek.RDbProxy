//! Tauri 外壳：只做命令绑定，业务全在 `pek-rdbproxy-tool-core`。
//!
//! 所有命令都是 `async` + `spawn_blocking`：`DbClient` 是同步阻塞实现，
//! 直接在 UI 线程上跑会卡界面。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use pek_rdbproxy_tool_core as tool;
use serde_json::{Value, json};

/// 把同步逻辑放到阻塞线程池跑，再序列化成 JSON 返回给前端。
async fn run_blocking<F, T>(f: F) -> Result<Value, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: serde::Serialize,
{
    tauri::async_runtime::spawn_blocking(move || {
        f().and_then(|value| serde_json::to_value(value).map_err(|e| format!("结果序列化失败：{e}")))
    })
    .await
    .map_err(|e| format!("后台任务失败（线程异常）：{e}"))?
}

/// 解析连接串（界面「预览」用；不联网）。
#[tauri::command]
async fn tool_parse(conn: String) -> Result<Value, String> {
    run_blocking(move || {
        let target = tool::Target::parse(&conn)?;
        Ok(json!({
            "server": target.server,
            "db": target.db,
            "tokenSet": !target.token.is_empty(),
            "connection": target.connection_string(),
            "masked": target.masked(),
        }))
    })
    .await
}

/// `Login`：探明库类型与版本（同时验证令牌与数据源名）。
#[tauri::command]
async fn tool_login(conn: String) -> Result<Value, String> {
    run_blocking(move || tool::login(&tool::Target::parse(&conn)?)).await
}

/// 查询。
#[tauri::command]
async fn tool_query(conn: String, sql: String) -> Result<Value, String> {
    run_blocking(move || tool::query(&tool::Target::parse(&conn)?, &sql)).await
}

/// 执行写语句。
#[tauri::command]
async fn tool_execute(conn: String, sql: String) -> Result<Value, String> {
    run_blocking(move || tool::execute(&tool::Target::parse(&conn)?, &sql)).await
}

/// 表结构。
#[tauri::command]
async fn tool_tables(conn: String) -> Result<Value, String> {
    run_blocking(move || tool::tables(&tool::Target::parse(&conn)?)).await
}

/// 单表记录数。
#[tauri::command]
async fn tool_count(conn: String, table: String) -> Result<Value, String> {
    run_blocking(move || tool::count(&tool::Target::parse(&conn)?, &table)).await
}

/// 一键自检。
#[tauri::command]
async fn tool_selfcheck(conn: String, options: tool::SelfCheckOptions) -> Result<Value, String> {
    run_blocking(move || {
        let target = tool::Target::parse(&conn)?;
        Ok(tool::selfcheck(&target, &options))
    })
    .await
}

/// 程序信息（界面「关于」用）。
#[tauri::command]
async fn tool_info() -> Result<Value, String> {
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "userAgent": tool::USER_AGENT,
        "protocol": "复用 Pek.RCode /Db/*（与项目侧 provider=network 同一条代码路径）",
    }))
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            tool_parse,
            tool_login,
            tool_query,
            tool_execute,
            tool_tables,
            tool_count,
            tool_selfcheck,
            tool_info,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Pek.RDbProxy 桌面工具失败");
}
