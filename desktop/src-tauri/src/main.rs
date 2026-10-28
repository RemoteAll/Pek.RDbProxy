//! Tauri 外壳：命令绑定，业务全在 `pek-rdbproxy-tool-core`。
//!
//! 两层职责：
//! 1. **执行**：所有命令都是 `async` + `spawn_blocking`（`DbClient` 是同步阻塞实现），
//!    执行完把「成功与否 / 行数 / 耗时」写回工作区（连接健康状态 + 执行历史）；
//! 2. **工作区**：连接 / SQL 片段 / 历史由 `core::store` 管理，
//!    令牌只在本进程内解密，**不下发到前端**——前端只记连接 id。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

use pek_rdbproxy_tool_core as tool;
use serde_json::{Value, json};
use tauri::Manager;

/// 应用状态：工作区存储（单进程单实例，用互斥锁串行化）。
struct AppState {
    store: Arc<Mutex<tool::Store>>,
}

/// 一次执行的回填信息（进历史）。
struct RunMeta {
    rows: i64,
    micros: u64,
}

/// 把同步逻辑放到阻塞线程池跑，返回 JSON + 回填信息。
async fn run_meta<F, T>(f: F) -> Result<(Value, RunMeta), String>
where
    F: FnOnce() -> Result<(T, RunMeta), String> + Send + 'static,
    T: serde::Serialize,
{
    tauri::async_runtime::spawn_blocking(move || -> Result<(Value, RunMeta), String> {
        let (value, meta) = f()?;
        let json = serde_json::to_value(value).map_err(|e| format!("结果序列化失败：{e}"))?;
        Ok((json, meta))
    })
    .await
    .map_err(|e| format!("后台任务失败（线程异常）：{e}"))?
}

/// 把同步逻辑放到阻塞线程池跑，返回 JSON。
async fn run_blocking<F, T>(f: F) -> Result<Value, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: serde::Serialize,
{
    let (value, _) = run_meta(move || f().map(|v| (v, RunMeta { rows: 0, micros: 0 }))).await?;
    Ok(value)
}

/// 取要用的连接：**优先工作区 id**（令牌只在 Rust 侧解密），否则用临时连接串。
fn resolve_target(
    store: &Arc<Mutex<tool::Store>>,
    conn_id: &Option<String>,
    conn: &Option<String>,
) -> Result<tool::Target, String> {
    if let Some(id) = conn_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let guard = store
            .lock()
            .map_err(|_| "工作区锁异常（上一个操作可能崩溃）".to_string())?;
        return guard.connection_target(id);
    }
    let raw = conn.as_deref().unwrap_or_default();
    tool::Target::parse(raw)
}

/// 记录一次执行：更新连接健康状态 + 追加历史（失败不影响主流程，只记日志）。
fn record_run(
    store: &Arc<Mutex<tool::Store>>,
    conn_id: &Option<String>,
    sql: &str,
    ok: bool,
    rows: i64,
    micros: u64,
) {
    let Some(id) = conn_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return; // 临时连接不记历史
    };
    let Ok(mut guard) = store.lock() else {
        return;
    };
    if let Err(e) = guard.touch_connection(id, ok) {
        eprintln!("记录连接状态失败（{id}）：{e}");
    }
    if !sql.trim().is_empty()
        && let Err(e) = guard.push_history(sql, id, ok, rows, micros)
    {
        eprintln!("记录执行历史失败（{id}）：{e}");
    }
}

/// 执行结果 → 回填历史 + 返回前端。
fn finish(
    store: &Arc<Mutex<tool::Store>>,
    conn_id: &Option<String>,
    sql: &str,
    call: Result<(Value, RunMeta), String>,
) -> Result<Value, String> {
    match call {
        Ok((value, meta)) => {
            record_run(store, conn_id, sql, true, meta.rows, meta.micros);
            Ok(value)
        }
        Err(e) => {
            record_run(store, conn_id, sql, false, 0, 0);
            Err(e)
        }
    }
}

/* ── 工作区 ─────────────────────────────────────── */

/// 读取工作区（连接 / 片段 / 历史）。
#[tauri::command]
async fn ws_load(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        Ok(guard.snapshot())
    })
    .await
}

/// 新建/更新连接（令牌留空 = 保留原值）。
#[tauri::command]
async fn ws_save_connection(
    state: tauri::State<'_, AppState>,
    input: tool::ConnectionInput,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.save_connection(input)?;
        Ok(guard.snapshot())
    })
    .await
}

/// 删除连接。
#[tauri::command]
async fn ws_delete_connection(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.delete_connection(&id)?;
        Ok(guard.snapshot())
    })
    .await
}

/// 新建/更新 SQL 片段。
#[tauri::command]
async fn ws_save_snippet(
    state: tauri::State<'_, AppState>,
    input: tool::SnippetInput,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.save_snippet(input)?;
        Ok(guard.snapshot())
    })
    .await
}

/// 删除 SQL 片段。
#[tauri::command]
async fn ws_delete_snippet(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.delete_snippet(&id)?;
        Ok(guard.snapshot())
    })
    .await
}

/// 标记片段被使用一次（用于「常用」排序）。
#[tauri::command]
async fn ws_touch_snippet(state: tauri::State<'_, AppState>, id: String) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.touch_snippet(&id)?;
        Ok(guard.snapshot())
    })
    .await
}

/// 删除单条历史。
#[tauri::command]
async fn ws_delete_history(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.delete_history(&id)?;
        Ok(guard.snapshot())
    })
    .await
}

/// 清空历史。
#[tauri::command]
async fn ws_clear_history(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.clear_history()?;
        Ok(guard.snapshot())
    })
    .await
}

/// 导出工作区 JSON（**含明文令牌**，由界面提示风险）。
#[tauri::command]
async fn ws_export(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        Ok(json!({ "json": guard.export()? }))
    })
    .await
}

/// 导入工作区 JSON（按「名称 + 数据源」合并连接，片段追加）。
#[tauri::command]
async fn ws_import(state: tauri::State<'_, AppState>, json_text: String) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    run_blocking(move || {
        let mut guard = store.lock().map_err(|_| "工作区锁异常".to_string())?;
        let (connections, snippets) = guard.import(&json_text)?;
        Ok(json!({
            "connections": connections,
            "snippets": snippets,
            "workspace": guard.snapshot(),
        }))
    })
    .await
}

/* ── 执行 ───────────────────────────────────────── */

/// `Login`：探明库类型与版本（顺带验证令牌与数据源名）。
#[tauri::command]
async fn tool_login(
    state: tauri::State<'_, AppState>,
    conn_id: Option<String>,
    conn: Option<String>,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    let call = {
        let store = Arc::clone(&store);
        let conn_id = conn_id.clone();
        run_meta(move || {
            let view = tool::login(&resolve_target(&store, &conn_id, &conn)?)?;
            let meta = RunMeta { rows: 0, micros: view.elapsed_ms };
            Ok((view, meta))
        })
        .await
    };
    finish(&store, &conn_id, "", call)
}

/// 查询。
#[tauri::command]
async fn tool_query(
    state: tauri::State<'_, AppState>,
    conn_id: Option<String>,
    conn: Option<String>,
    sql: String,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    let call = {
        let store = Arc::clone(&store);
        let conn_id = conn_id.clone();
        let sql = sql.clone();
        run_meta(move || {
            let view = tool::query(&resolve_target(&store, &conn_id, &conn)?, &sql)?;
            let meta = RunMeta { rows: view.row_count as i64, micros: view.elapsed_ms };
            Ok((view, meta))
        })
        .await
    };
    finish(&store, &conn_id, &sql, call)
}

/// 执行写语句。
#[tauri::command]
async fn tool_execute(
    state: tauri::State<'_, AppState>,
    conn_id: Option<String>,
    conn: Option<String>,
    sql: String,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    let call = {
        let store = Arc::clone(&store);
        let conn_id = conn_id.clone();
        let sql = sql.clone();
        run_meta(move || {
            let view = tool::execute(&resolve_target(&store, &conn_id, &conn)?, &sql)?;
            let meta = RunMeta { rows: view.affected, micros: view.elapsed_ms };
            Ok((view, meta))
        })
        .await
    };
    finish(&store, &conn_id, &sql, call)
}

/// 表结构。
#[tauri::command]
async fn tool_tables(
    state: tauri::State<'_, AppState>,
    conn_id: Option<String>,
    conn: Option<String>,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    let call = {
        let store = Arc::clone(&store);
        let conn_id = conn_id.clone();
        run_meta(move || {
            let view = tool::tables(&resolve_target(&store, &conn_id, &conn)?)?;
            let meta = RunMeta { rows: view.count as i64, micros: view.elapsed_ms };
            Ok((view, meta))
        })
        .await
    };
    finish(&store, &conn_id, "", call)
}

/// 单表记录数。
#[tauri::command]
async fn tool_count(
    state: tauri::State<'_, AppState>,
    conn_id: Option<String>,
    conn: Option<String>,
    table: String,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let call = {
        let store = Arc::clone(&store);
        let conn_id = conn_id.clone();
        let table = table.clone();
        run_meta(move || {
            let view = tool::count(&resolve_target(&store, &conn_id, &conn)?, &table)?;
            let meta = RunMeta { rows: view.count, micros: view.elapsed_ms };
            Ok((view, meta))
        })
        .await
    };
    finish(&store, &conn_id, &sql, call)
}

/// 一键自检。
#[tauri::command]
async fn tool_selfcheck(
    state: tauri::State<'_, AppState>,
    conn_id: Option<String>,
    conn: Option<String>,
    options: tool::SelfCheckOptions,
) -> Result<Value, String> {
    let store = Arc::clone(&state.store);
    let call = {
        let store = Arc::clone(&store);
        let conn_id = conn_id.clone();
        run_meta(move || {
            let target = resolve_target(&store, &conn_id, &conn)?;
            let report = tool::selfcheck(&target, &options);
            let meta = RunMeta { rows: 0, micros: report.total_ms };
            Ok((report, meta))
        })
        .await
    };
    finish(&store, &conn_id, "", call)
}

/// 程序信息（界面「关于」用）。
#[tauri::command]
async fn tool_info(state: tauri::State<'_, AppState>) -> Result<Value, String> {
    let data_dir = {
        let guard = state.store.lock().map_err(|_| "工作区锁异常".to_string())?;
        guard.dir().display().to_string()
    };
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "userAgent": tool::USER_AGENT,
        "protocol": "复用 Pek.RCode /Db/*（与项目侧 provider=network 同一条代码路径）",
        "dataDir": data_dir,
        "historyLimit": tool::HISTORY_LIMIT,
    }))
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("取应用数据目录失败：{e}"))?;
            let store = tool::Store::open(dir).map_err(|e| format!("打开工作区失败：{e}"))?;
            app.manage(AppState {
                store: Arc::new(Mutex::new(store)),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ws_load,
            ws_save_connection,
            ws_delete_connection,
            ws_save_snippet,
            ws_delete_snippet,
            ws_touch_snippet,
            ws_delete_history,
            ws_clear_history,
            ws_export,
            ws_import,
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
