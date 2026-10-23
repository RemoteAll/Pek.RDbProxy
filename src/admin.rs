//! 面板用户与权限（多用户 + 页签权限）：数据层接入 `pek-radmin`，存储 `Data/admin.db`。
//!
//! 约定（与 Pek.RAgent / HlkProductTool 同一套组装线）：
//! - 实体 `PanelUser` / `OperationLog`，物理表 `RDbProxy_PanelUser` / `RDbProxy_OperationLog`
//!   （见 `Entity/Model.xml`，与 C# 生态共用的 XCode 模型）；
//! - **内置管理员**（配置文件 `WebUserName` / `WebPasswordHash`）不落表、拥有全部权限；
//! - 表内用户按 `Permissions`（逗号分隔页签 key）授权；**用户管理**只允许内置管理员；
//! - 密码：`SHA-256(salt:password)`，只要求非空（不强制复杂度，对齐 `pek-radmin` 口径）。

use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use pek_radmin::panel::{AuditEntry, PanelAuth, PanelUser};
use pek_rcode::store::{self, SharedStore};
use pek_rcode::{Dal, EntityModel};

/// 内嵌 XCode 模型（唯一事实来源）。
const MODEL_XML: &str = include_str!("../Entity/Model.xml");
/// 管理库文件名（位于 `Data/` 下）。
const DB_FILE: &str = "admin.db";

/// 可授予的页签权限（顺序即展示顺序）。
pub const ALL_PERMISSIONS: &[(&str, &str)] = &[
    ("overview", "数据源与概览（只读）"),
    ("sources", "数据源管理（增删改 / 令牌 / 真实连接串）"),
    ("fields", "字段安全（表管理）"),
    ("audit", "SQL 审计"),
    ("settings", "服务设置与升级"),
];

/// 用户管理权限（**不在可授予表内**：只有内置管理员/本机免登录有）。
pub const PERM_USERS: &str = "users";

/// 面板权限表（key → 中文名）。
pub fn auth() -> &'static PanelAuth {
    static AUTH: OnceLock<PanelAuth> = OnceLock::new();
    AUTH.get_or_init(|| PanelAuth::new(ALL_PERMISSIONS))
}

/// 权限 key 列表（内置管理员全给）。
pub fn all_keys() -> Vec<String> {
    ALL_PERMISSIONS.iter().map(|(k, _)| (*k).to_string()).collect()
}

/// 权限中文名。
pub fn permission_name(key: &str) -> String {
    ALL_PERMISSIONS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, n)| (*n).to_string())
        .unwrap_or_else(|| key.to_string())
}

/// 打开失败的日志节流（同一错误只记一次）。
static OPEN_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// 获取（或首次打开）管理库共享存储（`store::get_or_open` 单飞打开 + 建表）。
pub fn storage(base: &Path) -> Result<Arc<SharedStore>, String> {
    match store::get_or_open(base, || open_dal(base)) {
        Ok((opened, created)) => {
            if created && OPEN_ERROR.lock().unwrap().take().is_some() {
                dhrust::info!("面板用户数据库已恢复可用");
            }
            Ok(opened)
        }
        Err(e) => {
            let mut last = OPEN_ERROR.lock().unwrap();
            if last.as_deref() != Some(e.as_str()) {
                dhrust::error!("面板用户数据库打开失败：{e}");
                *last = Some(e.clone());
            }
            Err(e)
        }
    }
}

fn open_dal(base: &Path) -> Result<Dal, String> {
    let model = EntityModel::parse(MODEL_XML).map_err(|e| format!("Model.xml 解析失败：{e}"))?;
    let db_path = base.join("Data").join(DB_FILE);
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建数据目录失败：{e}"))?;
    }
    let conn = format!("Data Source={};Provider=SQLite;ShowSql=false", db_path.display());
    let dal = Dal::open_with_model(&conn, model).map_err(|e| format!("打开数据库失败：{e}"))?;
    dal.sync_schema().map_err(|e| format!("同步表结构失败：{e}"))?;
    dhrust::info!("面板用户数据库就绪（{}）", db_path.display());
    Ok(dal)
}

// ————— 用户 —————

/// 按用户名查找（大小写不敏感）。
pub fn find_user(base: &Path, user_name: &str) -> Result<Option<PanelUser>, String> {
    let store = storage(base)?;
    auth().find_user(&store, user_name)
}

/// 用户列表（JSON 视图，含权限中文名）。
pub fn list_users_json(base: &Path) -> Result<serde_json::Value, String> {
    let store = storage(base)?;
    auth().list_users_json(&store)
}

/// 登录校验（表内用户，启用且密码正确）。
pub fn verify_login(base: &Path, user_name: &str, password: &str) -> Result<Option<PanelUser>, String> {
    let store = storage(base)?;
    auth().verify_login(&store, user_name, password)
}

/// 保存用户（不存在则创建，`password` 为空表示不改）。
pub fn save_user(
    base: &Path,
    user_name: &str,
    password: Option<&str>,
    permissions: &[String],
    enabled: bool,
    remark: &str,
) -> Result<(), String> {
    let store = storage(base)?;
    auth().save_user(&store, user_name, password, permissions, enabled, remark)
}

/// 删除用户。
pub fn delete_user(base: &Path, user_name: &str) -> Result<(), String> {
    let store = storage(base)?;
    auth().delete_user(&store, user_name)
}

// ————— 审计 —————

/// 写一条面板操作审计（失败只记程序日志）。
pub fn record(base: &Path, entry: &AuditEntry) {
    match storage(base) {
        Ok(store) => pek_radmin::panel::record(&store, entry),
        Err(e) => dhrust::warn!("操作审计未写入（用户库不可用）：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "rdbproxy-admin-{}-{stamp}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn user_crud_and_login() {
        let base = temp_dir("crud");
        save_user(&base, "alice", Some("pw1"), &["overview".to_string()], true, "运维").unwrap();
        let user = find_user(&base, "ALICE").unwrap().expect("用户应存在");
        assert!(user.verify("pw1"));
        assert!(verify_login(&base, "alice", "pw1").unwrap().is_some());
        assert!(verify_login(&base, "alice", "bad").unwrap().is_none());

        save_user(&base, "alice", None, &["overview".to_string(), "fields".to_string()], false, "停用").unwrap();
        assert!(verify_login(&base, "alice", "pw1").unwrap().is_none(), "停用后不能登录");
        let list = list_users_json(&base).unwrap();
        assert!(list.to_string().contains("alice"));

        delete_user(&base, "alice").unwrap();
        assert!(find_user(&base, "alice").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn permissions_are_normalized() {
        let base = temp_dir("perms");
        save_user(
            &base,
            "bob",
            Some("x"),
            &["settings".to_string(), "unknown".to_string(), "settings".to_string()],
            true,
            "",
        )
        .unwrap();
        let user = find_user(&base, "bob").unwrap().unwrap();
        assert_eq!(user.permissions, vec!["settings".to_string()], "未知权限应被过滤、重复应去掉");
        let _ = std::fs::remove_dir_all(&base);
    }
}
