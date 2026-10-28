//! 目标（连接字符串）与 `pek_rcode::db_service::DbClient` 的薄封装。
//!
//! **不重写协议**：所有请求都走 Pek.RCode 的 `DbClient`（与项目侧 `provider=network`
//! 完全同一条实现），本模块只负责连接串解析、UA 注入与结果整形。

use std::time::Instant;

use pek_rcode::dal::ConnectionString;
use pek_rcode::db_service::DbClient;
use serde::Serialize;
use serde_json::Value;

/// 出站 `User-Agent`（服务端 WAF 打开 `block_empty_ua` 时机器客户端不被拦）。
pub const USER_AGENT: &str = concat!("pek-rdbproxy-tool/", env!("CARGO_PKG_VERSION"));

/// 请求超时（秒级；查询慢时可在界面调大）。
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// 一个代理目标（= 项目侧会写进配置的那一行连接串）。
#[derive(Debug, Clone, Serialize)]
pub struct Target {
    /// 代理地址，如 `http://127.0.0.1:5599`
    pub server: String,
    /// 数据源名（面板卡片上的 Name，**不是**真实库名）
    pub db: String,
    /// 数据源令牌
    pub token: String,
    /// 用户令牌（登录后拿到；有则按用户身份鉴权，审计里能看出是谁）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer: Option<String>,
}

/// 归一化代理地址（登录 / 检查更新时用）：去首尾空白与末尾 `/`，缺协议时补 `http://`。
pub fn normalize_server(raw: &str) -> Result<String, String> {
    let text = raw.trim();
    if text.is_empty() {
        return Err("代理地址不能为空（例：http://127.0.0.1:5599）".to_string());
    }
    let mut server = text.trim_end_matches('/').to_string();
    let lower = server.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        server = format!("http://{server}");
    }
    Ok(server)
}

impl Target {
    /// 解析项目侧连接串：`Server=…;Database=…;Password=…;provider=network`。
    ///
    /// 键大小写不敏感；`Password` 与 `Token` 等价（后者是面板里的叫法）。
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("连接串为空".to_string());
        }
        let cs = ConnectionString::parse(raw);
        let server = cs
            .get("server")
            .map(str::trim)
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        if server.is_empty() {
            return Err("连接串缺少 Server（例：Server=http://127.0.0.1:5599）".to_string());
        }
        if !server.to_ascii_lowercase().starts_with("http://")
            && !server.to_ascii_lowercase().starts_with("https://")
        {
            return Err(format!("Server 需要带 http:// 或 https://：{server}"));
        }
        let db = cs
            .get("database")
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if db.is_empty() {
            return Err("连接串缺少 Database（填**数据源名**，不是真实库名）".to_string());
        }
        let token = cs
            .get("password")
            .or_else(|| cs.get("token"))
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        Ok(Self {
            server,
            db,
            token,
            bearer: None,
        })
    }

    /// 带上用户令牌（登录后拿到的 JWT）；`None` = 只带数据源令牌（旧式调用）。
    pub fn with_bearer(mut self, bearer: Option<String>) -> Self {
        self.bearer = bearer;
        self
    }

    /// 拼回项目侧连接串。
    pub fn connection_string(&self) -> String {
        format!(
            "Server={};Database={};Password={};provider=network",
            self.server, self.db, self.token
        )
    }

    /// 展示用摘要（令牌只留前 4 位，避免截图泄露）。
    pub fn masked(&self) -> String {
        let token = if self.token.len() > 4 {
            format!("{}…（{} 位）", &self.token[..4], self.token.chars().count())
        } else if self.token.is_empty() {
            "（空）".to_string()
        } else {
            "…".to_string()
        };
        format!("{} · {} · 令牌 {}", self.server, self.db, token)
    }

    /// 构造协议客户端（统一 UA + 超时 + 可选用户令牌）。
    pub(crate) fn client(&self) -> DbClient {
        let mut client = DbClient::new(&self.server, &self.db, Some(&self.token))
            .with_user_agent(USER_AGENT);
        if let Some(bearer) = self.bearer.as_deref() {
            client = client.with_bearer(bearer);
        }
        client.timeout = DEFAULT_TIMEOUT;
        client
    }
}

/// `Login` 结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginView {
    /// 数据库类型（C# `DatabaseType` 规范名）
    pub db_type: String,
    /// 版本
    pub version: String,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
}

/// `Query` 结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryView {
    /// 列名
    pub columns: Vec<String>,
    /// 数据行（单元格为 JSON 值）
    pub rows: Vec<Value>,
    /// 行数
    pub row_count: usize,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
    /// 需提醒用户的说明（如「列被字段策略拦截」），无则为空
    pub note: String,
}

/// `Execute` 结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteView {
    /// 受影响行数
    pub affected: i64,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
}

/// `GetTables` 结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TablesView {
    /// 表结构数组（原样透传服务端 JSON）
    pub tables: Vec<Value>,
    /// 表数量
    pub count: usize,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
}

/// `QueryCount` 结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountView {
    /// 表名
    pub table: String,
    /// 记录数
    pub count: i64,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
}

/// 登录：探明远端库类型与版本（顺带验证令牌与数据源名）。
pub fn login(target: &Target) -> Result<LoginView, String> {
    let started = Instant::now();
    let info = target
        .client()
        .login()
        .map_err(|e| format!("Login 失败：{e}"))?;
    Ok(LoginView {
        db_type: info.db_type,
        version: info.version,
        elapsed_ms: ms(started),
    })
}

/// 查询并返回结果集。
pub fn query(target: &Target, sql: &str) -> Result<QueryView, String> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("SQL 不能为空".to_string());
    }
    let started = Instant::now();
    let value = target
        .client()
        .query(sql, None)
        .map_err(|e| format!("Query 失败：{e}"))?;
    let columns: Vec<String> = value
        .get("columns")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let rows = value
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // 代理默认「字段策略 = enforce 白名单」：未登记的列一律不返回。
    // 有行无列时大概率是策略拦截，不是查询失败，必须说清楚。
    let note = if columns.is_empty() && !rows.is_empty() {
        "服务端字段策略（默认白名单）未放行任何列：到代理面板「🛡 字段安全」为这些列登记 allow/mask，或把模式切到 audit".to_string()
    } else {
        String::new()
    };
    Ok(QueryView {
        row_count: rows.len(),
        columns,
        rows,
        elapsed_ms: ms(started),
        note,
    })
}

/// 执行写语句，返回受影响行数（只读数据源会被服务端拒绝 —— 这正是要验证的行为之一）。
pub fn execute(target: &Target, sql: &str) -> Result<ExecuteView, String> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err("SQL 不能为空".to_string());
    }
    let started = Instant::now();
    let affected = target
        .client()
        .execute(sql, None)
        .map_err(|e| format!("Execute 失败：{e}"))?;
    Ok(ExecuteView {
        affected,
        elapsed_ms: ms(started),
    })
}

/// 读取表结构（`GetTables`）。
pub fn tables(target: &Target) -> Result<TablesView, String> {
    let started = Instant::now();
    let value = target
        .client()
        .get_tables()
        .map_err(|e| format!("GetTables 失败：{e}"))?;
    let list = value.as_array().cloned().unwrap_or_default();
    Ok(TablesView {
        count: list.len(),
        tables: list,
        elapsed_ms: ms(started),
    })
}

/// 单表记录数（`QueryCount`）。
pub fn count(target: &Target, table: &str) -> Result<CountView, String> {
    let table = table.trim();
    if table.is_empty() {
        return Err("表名不能为空".to_string());
    }
    let started = Instant::now();
    let count = target
        .client()
        .query_count(table)
        .map_err(|e| format!("QueryCount 失败（表 {table}）：{e}"))?;
    Ok(CountView {
        table: table.to_string(),
        count,
        elapsed_ms: ms(started),
    })
}

/// 毫秒耗时（不小于 0）。
fn ms(started: Instant) -> u64 {
    started.elapsed().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_project_connection_string() {
        let target =
            Target::parse("Server=http://127.0.0.1:5599;Database=demo;Password=tk-1;provider=network")
                .unwrap();
        assert_eq!(target.server, "http://127.0.0.1:5599");
        assert_eq!(target.db, "demo");
        assert_eq!(target.token, "tk-1");
        assert!(target.connection_string().contains("provider=network"));
    }

    #[test]
    fn parse_is_case_insensitive_and_accepts_token_key() {
        let target = Target::parse("server=HTTP://10.0.0.5:5599/;DATABASE=dev;Token=abc").unwrap();
        // 地址尾部斜杠去掉，但保留大小写
        assert_eq!(target.server, "HTTP://10.0.0.5:5599");
        assert_eq!(target.db, "dev");
        assert_eq!(target.token, "abc");
    }

    #[test]
    fn parse_rejects_missing_or_bad_fields() {
        assert!(Target::parse("").is_err());
        assert!(Target::parse("Database=demo;Password=t").unwrap_err().contains("Server"));
        assert!(Target::parse("Server=127.0.0.1:5599;Database=demo").unwrap_err().contains("http"));
        assert!(
            Target::parse("Server=http://127.0.0.1:5599;Password=t")
                .unwrap_err()
                .contains("Database")
        );
    }

    #[test]
    fn masked_hides_token() {
        let target = Target {
            server: "http://127.0.0.1:5599".into(),
            db: "demo".into(),
            token: "tk-secret-123456".into(),
            bearer: None,
        };
        let masked = target.masked();
        assert!(masked.contains("tk-s"));
        assert!(!masked.contains("secret"));
    }
}
