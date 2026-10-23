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
    /// 面板登录令牌（空 = 面板仅回环免登录）
    pub admin_token: String,
    /// 是否允许非回环来源
    pub allow_remote: bool,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 5599,
            admin_token: String::new(),
            allow_remote: false,
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
