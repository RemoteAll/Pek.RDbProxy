//! `Config/Proxy.toml`：真实连接串只存在这里（项目侧只有代理地址 + 令牌）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 首次运行写出的配置模板（全字段中文说明）。
pub const TEMPLATE: &str = include_str!("../res/Proxy.toml.template");

/// 配置相对路径（相对程序基目录）。
pub const CONFIG_RELATIVE: &str = "Config/Proxy.toml";

/// 代理配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ProxyConfig {
    /// 服务端设置
    pub server: ServerSection,
    /// 数据源列表
    pub databases: Vec<DatabaseSection>,
    /// 驱动组件源（未内置驱动的库类型运行时按需下载）
    pub driver_store: DriverStoreSection,
}

/// `[DriverStore]` 段：驱动组件源（Pek.RCode `driver-pack`）。
///
/// 本程序**只内置常用驱动**（SQLite 永远内置；另见 `Cargo.toml` 的 `driver-*` 特性），
/// 其余库类型（Oracle / 达梦 / MongoDB / HANA / Firebird / ODBC …）首次使用某个数据源时
/// 自动从组件源下载对应 `dbserver-<类型>` 驱动包到本地缓存，再拉起宿主进程转接——
/// 因此**驱动组件升级不需要重新编译/重新发布本程序**。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DriverStoreSection {
    /// 组件源地址（空 = 平台默认地址；`off` = 关闭联网，仅用本机缓存）
    pub url: String,
    /// 组件源 Ed25519 公钥（hex / SPKI；非空则强制验签，并允许内网 http 组件源）
    pub pub_key: String,
    /// 组件源 https 自定义根证书（PEM 文件；相对路径按程序基目录）
    pub ca_file: String,
    /// 组件优先：`true` = 本机已下载的驱动组件优先于内置驱动（驱动可单独升级）
    pub component_first: bool,
    /// 驱动缓存目录（空 = `Data/Drivers`）
    pub cache_dir: String,
}

impl Default for DriverStoreSection {
    fn default() -> Self {
        Self {
            url: String::new(),
            pub_key: String::new(),
            ca_file: String::new(),
            component_first: true,
            cache_dir: String::new(),
        }
    }
}

/// `[Server]` 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct ServerSection {
    /// 监听地址
    pub host: String,
    /// 监听端口
    pub port: u16,
    /// 是否允许非回环来源
    pub allow_remote: bool,
    /// 面版仅限本机访问（默认 false）
    ///
    /// - `false`（默认）：面板可远程打开，**靠账号 + 页签权限**控制能看到什么；
    /// - `true`：面板只在回环打开，远程只能用令牌走 `/Db/*`（最高锁死级别）。
    #[serde(default)]
    pub panel_local_only: bool,
    /// 面板用户名
    pub web_user_name: String,
    /// 面板密码哈希（`盐:哈希`，由 `-init` 生成，面板可改）
    pub web_password_hash: String,
    /// 鉴权级别：None / LocalOnly（默认）/ Full
    pub web_auth_level: String,
    /// JWT 密钥（首次使用自动生成）
    pub jwt_secret: String,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 5599,
            allow_remote: false,
            panel_local_only: false,
            web_user_name: "admin".to_string(),
            web_password_hash: String::new(),
            web_auth_level: "LocalOnly".to_string(),
            jwt_secret: String::new(),
        }
    }
}

/// `[[Databases]]` 段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DatabaseSection {
    /// 数据源名（项目侧 `Database=<Name>`）
    pub name: String,
    /// 真实连接串（可为 `enc:v1:` 加密值）
    pub connection: String,
    /// 访问令牌（项目侧 `Password=<Token>`）
    #[serde(default)]
    pub token: String,
    /// 只读模式
    #[serde(default)]
    pub read_only: bool,
    /// 备注
    #[serde(default)]
    pub description: String,
}

/// 配置文件路径。
pub fn config_path(base: &Path) -> PathBuf {
    base.join(CONFIG_RELATIVE)
}

/// 读取配置（缺失/解析失败都给出可操作的错误信息）。
pub fn load(base: &Path) -> Result<ProxyConfig, String> {
    let path = config_path(base);
    if !path.is_file() {
        return Err(format!(
            "配置文件不存在：{}（直接双击运行会自动生成模板，或运行 `pek-rdbproxy -init`）",
            path.display()
        ));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取配置失败（{}）：{e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("配置解析失败（{}）：{e}", path.display()))
}

/// 写出配置模板（已存在则报错，避免覆盖真实连接串）。
pub fn write_template(base: &Path) -> Result<PathBuf, String> {
    let path = config_path(base);
    if path.exists() {
        return Err(format!("配置文件已存在，未覆盖：{}", path.display()));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    std::fs::write(&path, TEMPLATE).map_err(|e| format!("写入模板失败：{e}"))?;
    Ok(path)
}

/// 保存配置：**保留原有注释与排版**（就地改标量值 + 同步 `[[Databases]]` 数组表），
/// 写入前自动备份为 `Proxy.toml.bak`。
pub fn save(base: &Path, config: &ProxyConfig) -> Result<PathBuf, String> {
    let path = config_path(base);
    let text = if path.is_file() {
        std::fs::read_to_string(&path).map_err(|e| format!("读取配置失败：{e}"))?
    } else {
        TEMPLATE.to_string()
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败：{e}"))?;
    }
    let mut doc = dhrust::config::toml::parse_document(&text)?;
    let original = dhrust::config::toml::document_to_json(&doc)?;
    let target = serde_json::to_value(config).map_err(|e| format!("配置序列化失败：{e}"))?;

    // ① 补齐模板里有、文件里缺的键（如新版本新增的 WebUserName/JwtSecret），带注释
    if let Ok(template) = dhrust::config::toml::parse_document(TEMPLATE) {
        dhrust::config::toml::merge_missing_keys(doc.as_table_mut(), template.as_table());
    }
    // ② 标量（Server 段等）：就地改值，保留原有注释
    dhrust::config::toml::apply_type_fixes(&mut doc, &original, &target);
    // ③ 数组表（[[Databases]]）：按条目同步，多余的删除、新增的追加
    let entries: Vec<serde_json::Value> = target
        .get("Databases")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    dhrust::config::toml::sync_array_of_tables(&mut doc, "Databases", &entries);

    let content = doc.to_string();
    if path.is_file() {
        dhrust::config::toml::backup_and_write(&path, &content)
            .map_err(|e| format!("写入配置失败：{e}"))
    } else {
        std::fs::write(&path, content).map_err(|e| format!("写入配置失败：{e}"))?;
        Ok(path)
    }
}

/// 连接串脱敏：用于日志与面板展示（`Pwd=`/`Password=` 等一律替换为 `***`）。
pub fn mask_connection(connection: &str) -> String {
    let mut out = String::with_capacity(connection.len());
    for (index, part) in connection.split(';').enumerate() {
        if index > 0 {
            out.push(';');
        }
        match part.split_once('=') {
            Some((key, _)) if is_secret_key(key) => {
                out.push_str(key);
                out.push_str("=***");
            }
            _ => out.push_str(part),
        }
    }
    out
}

/// 判断连接串键是否为凭据类（大小写不敏感）。
fn is_secret_key(key: &str) -> bool {
    let key = key.trim();
    ["pwd", "password", "pass", "secret", "token", "apikey", "api_key"]
        .iter()
        .any(|name| key.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_connection_hides_credentials() {
        let masked = mask_connection(
            "Server=10.0.0.5;Port=3306;Database=demo;Uid=readonly;Pwd=p@ss;provider=mysql",
        );
        assert!(!masked.contains("p@ss"), "密码必须被遮盖：{masked}");
        assert!(masked.contains("Pwd=***"));
        assert!(masked.contains("Uid=readonly"), "非凭据字段应保留");
        assert!(masked.contains("provider=mysql"));
    }

    #[test]
    fn mask_connection_keeps_sqlite_path() {
        let masked = mask_connection("Data Source=Data/demo.db;Provider=SQLite");
        assert_eq!(masked, "Data Source=Data/demo.db;Provider=SQLite");
    }

    #[test]
    fn template_parses() {
        let cfg: ProxyConfig = toml::from_str(TEMPLATE).expect("模板必须可解析");
        assert_eq!(cfg.server.port, 5599);
        assert_eq!(cfg.databases.len(), 1);
        assert_eq!(cfg.databases[0].name, "demo");
        assert!(cfg.databases[0].read_only);
        // [DriverStore]：空 Url = 平台默认组件源；组件优先默认关闭（内置驱动零网络）
        assert!(cfg.driver_store.url.is_empty());
        assert!(!cfg.driver_store.component_first);
        assert!(cfg.driver_store.cache_dir.is_empty());
    }

    #[test]
    fn missing_config_is_reported_with_double_click_hint() {
        // 首次运行（双击）依赖这条判断：不存在即自动写模板，文案要告诉用户两条路径
        let base = std::env::temp_dir().join(format!(
            "rdbproxy-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let error = load(&base).expect_err("缺失配置应报错");
        assert!(error.contains("配置文件不存在"), "{error}");
        assert!(error.contains("双击"), "提示要给出双击这条路：{error}");
    }
}
