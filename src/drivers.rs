//! 驱动组件按需分发（Pek.RCode `driver-pack`）——**驱动升级不必重编/重发本程序**。
//!
//! # 为什么这样做
//!
//! `Cargo.toml` 里 `driver-*` 特性是**编译期**决定内置哪些驱动：新驱动类型（Oracle / 达梦 /
//! MongoDB / HANA / Firebird / ODBC …）要改 `Cargo.toml` 重编；驱动 crate 升级也要跟着重编重发。
//! 本模块把「运行时需要但不内置」的驱动交给 Pek.RCode 的组件源分发：
//!
//! ```text
//! 数据源连接串（Provider=Oracle）→ driver_pack_need → 组件 id（dbserver-oracle）
//!   → 组件源 catalog.json（Ed25519 验签）→ 下载/校验（SHA-256）→ 解压到 Data/Drivers
//!   → 拉起 dbserver 宿主（回环 + 一次性令牌）→ 返回 provider=network 连接串
//! ```
//!
//! # 取驱动顺序（[`prepare`]）
//!
//! 1. **组件缓存优先**：`Data/Drivers/<组件>/` 已存在（面板装过 / 上次下载）→ 用组件
//!    （组件可比内置新，驱动能单独远程升级，不必升级本程序）；
//! 2. **内置兜底**：本构建编入的驱动（见 [`BUILTIN_PROVIDERS`]）直接本地打开，零网络；
//! 3. 其余类型 → 联网按需下载组件包。
//!
//! [`prepare`] 用的是 `DriverManager::ensure`（**本地缓存优先**），所以重启/打开数据源
//! 不会每次都联网；要升级驱动在面板「设置 → 驱动组件」点「下载/升级」
//! （或把 `ComponentFirst` 设 true 让内置类型也强制走组件）。
//!
//! 参照实现：`HLZX.EIP/src/drivers.rs`、`HlktechIoT/Rust/crates/hlktech-iot-core/src/drivers.rs`。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use pek_rcode::driver_pack::{
    DriverManager, DriverPackNeed, HostStatus, StoreConfig, current_target, driver_pack_need,
};

use crate::config::DriverStoreSection;

/// 本构建**内置**的 `provider` 名单。
///
/// 与 `Cargo.toml` 里 `pek-rcode` 的 `driver-*` 特性保持一致（SQLite 由 `rusqlite` 恒内置，
/// 不在名单里也不必下载）。**增删内置驱动必须同步改这里**，否则面板会把「其实内置可用」
/// 的类型误标成需要下载组件。
pub const BUILTIN_PROVIDERS: &[&str] = &["mysql", "postgresql", "sqlserver"];

/// 驱动分发运行态（配置变更时整体替换）。
struct State {
    /// 驱动管理器（未初始化 = `None`，此时连接串原样返回）
    manager: Option<Arc<DriverManager>>,
    /// 驱动缓存目录
    cache_dir: PathBuf,
    /// 生效的组件源地址（空 = 仅本地缓存模式）
    store: String,
    /// 强制组件优先（true = 内置类型也走组件，便于驱动单独升级）
    component_first: bool,
}

fn state() -> &'static RwLock<State> {
    static STATE: OnceLock<RwLock<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        RwLock::new(State {
            manager: None,
            cache_dir: PathBuf::from("Data/Drivers"),
            store: String::new(),
            component_first: false,
        })
    })
}

/// 初始化（幂等，可重复调用：面板保存配置后按新配置重建，旧宿主先停）。
///
/// # 参数
/// - `base`：程序基目录（相对路径的 CA / 缓存目录按它解析）
/// - `section`：`[DriverStore]` 配置
pub fn init(base: &Path, section: &DriverStoreSection) -> Result<(), String> {
    let raw_cache = section.cache_dir.trim();
    let cache_dir = if raw_cache.is_empty() {
        base.join("Data").join("Drivers")
    } else {
        let p = Path::new(raw_cache);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        }
    };
    std::fs::create_dir_all(&cache_dir).map_err(|e| {
        format!(
            "创建驱动缓存目录失败（{}）：{e}",
            cache_dir.display()
        )
    })?;

    // 三件套装配复用 Pek.RCode（归一化地址 → 读自定义 CA → 生成管理器配置）
    let store = StoreConfig::load(&section.url, &section.pub_key, &section.ca_file, base)?;
    let manager = DriverManager::new(store.manager_config(cache_dir.clone()))
        .map_err(|e| format!("驱动组件源配置无效（{}）：{e}", section.url))?;

    let mut guard = state().write().unwrap_or_else(|e| e.into_inner());
    // 配置变更：旧宿主进程必须先停（否则它们继续跑旧版本驱动、并占着端口）
    if let Some(old) = guard.manager.take() {
        old.stop_all();
    }
    guard.manager = Some(Arc::new(manager));
    guard.cache_dir = cache_dir.clone();
    guard.store = store.url.clone();
    guard.component_first = section.component_first;
    drop(guard);

    if store.url.is_empty() {
        dhrust::info!(
            "驱动组件：未启用联网下载（仅本机缓存 {}）；内置驱动照常可用：{}",
            cache_dir.display(),
            BUILTIN_PROVIDERS.join(", ")
        );
    } else {
        dhrust::info!(
            "驱动组件：组件源 {}；缓存 {}；内置 {}；组件优先={}",
            store.url.trim_end_matches('/'),
            cache_dir.display(),
            BUILTIN_PROVIDERS.join(", "),
            section.component_first
        );
    }
    Ok(())
}

/// 当前驱动管理器（未初始化 = `None`）。
pub fn manager() -> Option<Arc<DriverManager>> {
    state()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .manager
        .clone()
}

/// 生效的驱动缓存目录（展示用；未初始化时为相对路径 `Data/Drivers`）。
pub fn cache_dir() -> String {
    state()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .cache_dir
        .display()
        .to_string()
}

/// 生效的组件源地址（空 = 仅本地缓存模式）。
pub fn store() -> String {
    state()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .store
        .clone()
}

/// 连接串准备（**打开数据库前必须经过这里**）。
///
/// - 内嵌驱动（SQLite）与已是 `provider=network` 的连接串 → 原样返回；
/// - 需要组件、且组件未配置/不可用但本构建内置该驱动 → 回落到内置驱动直连（记 warn）；
/// - 其余情况返回驱动宿主给的 `provider=network` 连接串。
pub fn prepare(conn: &str) -> Result<String, String> {
    let (manager, component_first, cache_dir) = {
        let guard = state().read().unwrap_or_else(|e| e.into_inner());
        (guard.manager.clone(), guard.component_first, guard.cache_dir.clone())
    };
    let builtin = is_builtin(conn);

    // 内置可用、不强制组件、且本机没有该组件缓存 → 直接本地打开（零网络、零进程）
    if builtin && !component_first && !component_cached(conn, &cache_dir) {
        return Ok(conn.trim().to_string());
    }

    let Some(manager) = manager else {
        return Ok(conn.trim().to_string());
    };
    match manager.prepare(conn) {
        Ok(conn) => Ok(conn),
        Err(e) if builtin && !component_first => {
            dhrust::warn!(
                "驱动组件不可用，改用内置驱动直连（连接串 provider 命中内置：{}）：{e}",
                conn_provider(conn)
            );
            Ok(conn.trim().to_string())
        }
        Err(e) => Err(friendly_error(&e.to_string())),
    }
}

/// 组件状态行（面板展示用）。
#[derive(Debug, Clone)]
pub struct ComponentRow {
    /// 组件 id（如 `dbserver-mysql`）
    pub id: String,
    /// 显示名
    pub name: String,
    /// 本构建是否内置该类型的驱动（内置 = 不下载也能用）
    pub builtin: bool,
    /// 本机缓存版本（未下载 = `None`）
    pub local: Option<String>,
    /// 组件源最新版本（无组件源/无当前平台包 = `None`）
    pub latest: Option<String>,
    /// 正在运行的宿主进程数
    pub running: usize,
}

impl ComponentRow {
    /// 是否需要安装/升级（本地缺失，或远端版本更高）。
    pub fn upgradable(&self) -> bool {
        match (&self.local, &self.latest) {
            (None, Some(_)) => true,
            (Some(local), Some(latest)) => version_cmp(latest, local).is_gt(),
            _ => false,
        }
    }
}

/// 驱动组件总览。
#[derive(Debug, Clone)]
pub struct DriverReport {
    /// 生效的组件源地址（空 = 仅本地缓存模式）
    pub store: String,
    /// 是否关闭了联网下载
    pub offline: bool,
    /// 强制组件优先
    pub component_first: bool,
    /// 驱动缓存目录
    pub cache_dir: String,
    /// 当前平台标识（组件包按平台区分）
    pub target: String,
    /// 组件源是否可达（`false` 时 `error` 说明原因，仍会列出本地已有组件）
    pub online: bool,
    /// 组件源访问失败原因
    pub error: String,
    /// 组件清单（远端目录 ∪ 本机缓存）
    pub items: Vec<ComponentRow>,
    /// 运行中的宿主
    pub hosts: Vec<HostStatus>,
}

/// 组件总览（一次网络请求读目录；组件源不可达时降级为「只看本地缓存」）。
pub fn report() -> DriverReport {
    let (manager, cache_dir, store, component_first) = {
        let guard = state().read().unwrap_or_else(|e| e.into_inner());
        (
            guard.manager.clone(),
            guard.cache_dir.clone(),
            guard.store.clone(),
            guard.component_first,
        )
    };

    // 远端目录：id → (显示名, 最新版本)
    let mut remote: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut online = false;
    let mut error = String::new();
    if let Some(manager) = &manager {
        match manager.catalog() {
            Ok(entries) => {
                online = !store.is_empty();
                for entry in entries {
                    let slot = remote
                        .entry(entry.id.clone())
                        .or_insert_with(|| (entry.name.clone(), entry.version.clone()));
                    if !entry.name.trim().is_empty() {
                        slot.0 = entry.name.clone();
                    }
                    if version_cmp(&entry.version, &slot.1).is_gt() {
                        slot.1 = entry.version.clone();
                    }
                }
            }
            Err(e) => error = e.to_string(),
        }
    }

    // 本机缓存：{cache}/{组件}/{版本}/
    let mut local: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(&cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            let version = manager
                .as_ref()
                .and_then(|m| m.local_version(&id))
                .or_else(|| max_version_dir(&path));
            if let Some(version) = version {
                local.insert(id, version);
            }
        }
    }

    let hosts = manager.as_ref().map(|m| m.hosts()).unwrap_or_default();
    let mut running: BTreeMap<String, usize> = BTreeMap::new();
    for host in &hosts {
        *running.entry(host.component.clone()).or_insert(0) += 1;
    }

    let ids: BTreeSet<String> = remote.keys().cloned().chain(local.keys().cloned()).collect();
    let items = ids
        .into_iter()
        .map(|id| {
            let name = remote
                .get(&id)
                .map(|(name, _)| name.clone())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| display_name(&id));
            ComponentRow {
                builtin: component_builtin(&id),
                local: local.get(&id).cloned(),
                latest: remote.get(&id).map(|(_, version)| version.clone()),
                running: running.get(&id).copied().unwrap_or(0),
                id,
                name,
            }
        })
        .collect();

    DriverReport {
        offline: store.is_empty(),
        store,
        component_first,
        cache_dir: cache_dir.display().to_string(),
        target: current_target().unwrap_or("unknown").to_string(),
        online,
        error,
        items,
        hosts,
    }
}

/// 下载/升级指定组件到本地缓存（**不拉起宿主**；返回安装的版本）。
pub fn install(component: &str) -> Result<String, String> {
    let manager = manager().ok_or_else(|| "驱动组件未初始化（检查 Config/Proxy.toml 的 [DriverStore]）".to_string())?;
    let (version, dir) = manager
        .install_component(component.trim())
        .map_err(|e| format!("组件 {component} 安装失败：{}", friendly_error(&e.to_string())))?;
    dhrust::info!(
        "驱动组件已就绪：{component} {version}（{}）",
        dir.display()
    );
    Ok(version)
}

/// 停止全部驱动宿主（升级/排查用；下次打开数据源会重新拉起新版本）。
///
/// 返回停止的宿主数量。
pub fn stop_hosts() -> usize {
    let Some(manager) = manager() else {
        return 0;
    };
    let count = manager.hosts().len();
    manager.stop_all();
    count
}

/// 该连接串对应的组件是否已在本机缓存（面板装过一次即为真）。
fn component_cached(conn: &str, cache_dir: &Path) -> bool {
    match driver_pack_need(conn) {
        Ok(DriverPackNeed::Component(id)) => cache_dir.join(&id).is_dir(),
        _ => false,
    }
}

/// 本构建是否内置了该连接串所需驱动的 `provider`。
///
/// 只认连接串的 `provider=` 字段（分号分段、大小写不敏感、去引号），不做子串匹配——
/// 避免密码里带 `provider=mysql` 字样被误判。
fn is_builtin(conn: &str) -> bool {
    let provider = conn_provider(conn);
    BUILTIN_PROVIDERS
        .iter()
        .any(|p| provider.eq_ignore_ascii_case(p))
}

/// 取连接串的 `provider` 值（小写原样返回，未写则空串）。
fn conn_provider(conn: &str) -> String {
    for segment in conn.split(';') {
        let Some((key, value)) = segment.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("provider") {
            return value
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_ascii_lowercase();
        }
    }
    String::new()
}

/// 组件 id 对应的驱动是否内置（面板据此标注「内置」）。
pub fn component_builtin(component_id: &str) -> bool {
    let id = component_id.trim().to_ascii_lowercase();
    let Some(kind) = id.strip_prefix("dbserver-") else {
        return false;
    };
    is_builtin(&format!("Provider={kind}"))
}

/// 组件 id → 显示名（未知 id 原样返回）。
fn display_name(component_id: &str) -> String {
    let kind = component_id
        .strip_prefix("dbserver-")
        .unwrap_or(component_id);
    match kind.to_ascii_lowercase().as_str() {
        "mysql" => "MySQL / MariaDB".to_string(),
        "postgresql" => "PostgreSQL / HighGo / KingBase".to_string(),
        "sqlserver" => "SQL Server".to_string(),
        "oracle" => "Oracle".to_string(),
        "dameng" => "达梦 DM".to_string(),
        "firebird" => "Firebird".to_string(),
        "clickhouse" => "ClickHouse".to_string(),
        "tdengine" => "TDengine".to_string(),
        "influxdb" => "InfluxDB".to_string(),
        "mongodb" => "MongoDB".to_string(),
        "hana" => "SAP HANA".to_string(),
        "odbc" => "ODBC（DB2 / IRIS 等）".to_string(),
        "vastbase" => "VastBase（海量）".to_string(),
        other => other.to_string(),
    }
}

/// 缓存目录下版本号最大的子目录（无有效版本目录 = `None`）。
fn max_version_dir(component_dir: &Path) -> Option<String> {
    let mut best: Option<String> = None;
    for entry in std::fs::read_dir(component_dir).ok()?.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let version = entry.file_name().to_string_lossy().to_string();
        if best.as_deref().is_none_or(|b| version_cmp(&version, b).is_gt()) {
            best = Some(version);
        }
    }
    best
}

/// 版本号比较（数字段按数值、非数字段按字典序；缺段视为 0）。
fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn key(segment: Option<&str>) -> (u8, u64, String) {
        match segment {
            None => (2, 0, String::new()),
            Some(s) => match s.parse::<u64>() {
                Ok(n) => (2, n, String::new()),
                Err(_) => (0, 0, s.to_ascii_lowercase()),
            },
        }
    }
    let sa: Vec<&str> = a.split(['.', '-', '_']).collect();
    let sb: Vec<&str> = b.split(['.', '-', '_']).collect();
    for i in 0..sa.len().max(sb.len()) {
        let ord = key(sa.get(i).copied())
            .cmp(&key(sb.get(i).copied()));
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

/// 给「缺驱动 / 组件源不可达」类错误补上面板操作指引（否则运维只看到一句技术错误）。
fn friendly_error(msg: &str) -> String {
    let actionable = msg.contains("未配置组件源")
        || msg.contains("本地缓存为空")
        || msg.contains("未包含")
        || msg.contains("地址不被允许");
    if !actionable {
        return msg.to_string();
    }
    format!(
        "{msg}\n（面板「设置 → 驱动组件」可检测/下载/升级驱动；离线环境请把该驱动包解压到 {} 后重试）",
        state()
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .cache_dir
            .display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_detection_is_field_based() {
        assert!(is_builtin("Server=1.2.3.4;Database=x;provider=mysql"));
        assert!(is_builtin("provider=\"MySQL\";Server=1"));
        assert!(is_builtin("Server=1;provider=sqlserver"));
        assert!(!is_builtin("Server=1;Password=provider=mysql"));
        assert!(!is_builtin("Server=1;provider=oracle"));
        assert!(!is_builtin("Data Source=x.db"));
    }

    #[test]
    fn component_builtin_matches_feature_set() {
        assert!(component_builtin("dbserver-mysql"));
        assert!(component_builtin("dbserver-postgresql"));
        assert!(!component_builtin("dbserver-oracle"));
        assert!(!component_builtin("mysql"));
    }

    #[test]
    fn version_order() {
        assert!(version_cmp("0.1.10", "0.1.3").is_gt());
        assert!(version_cmp("0.1.0", "0.1").is_eq());
        assert!(version_cmp("1.0.0", "1.0.0-beta").is_gt());
    }

    #[test]
    fn prepare_without_init_passes_through() {
        // 未初始化：连接串原样返回（不报错），保证极端情况下内置/SQLite 仍可用
        let out = prepare("Server=1;provider=oracle").expect("不应失败");
        assert_eq!(out, "Server=1;provider=oracle");
    }

    #[test]
    fn report_without_init_is_offline_but_lists_cache() {
        let dir = std::env::temp_dir().join(format!("rdbproxy-drv-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("dbserver-oracle").join("0.1.2")).unwrap();
        let mut guard = state().write().unwrap();
        guard.cache_dir = dir.clone();
        drop(guard);

        let report = report();
        assert!(report.items.iter().any(|i| i.id == "dbserver-oracle"
            && i.local.as_deref() == Some("0.1.2")
            && !i.builtin
            && i.latest.is_none()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
