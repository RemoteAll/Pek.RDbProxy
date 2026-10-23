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
            "配置文件不存在：{}（先运行 `pek-rdbproxy -init` 生成模板）",
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
    }
}
