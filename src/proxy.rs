//! 代理核心：多数据源 + `/Db/*` 协议（与 C# `DbController` 对齐）+ 只读护栏 + 审计、统计 + 面板 API。
//!
//! 协议完全复用 [`pek_rcode::db_service::DbService`]，因此 **Rust / C# 客户端都能直连**：
//!
//! ```text
//! Server=http://127.0.0.1:5599;Database=demo;Password=<Token>;provider=network
//! ```

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use dhrust::net::http::{HttpRequest, HttpResponse};
use pek_rcode::dal::Dal;
use pek_rcode::db_service::DbService;
use serde_json::{Value, json};

use crate::config::{DatabaseSection, ProxyConfig, mask_connection};

/// 审计缓冲上限（内存环形；同时写日志文件）。
pub const AUDIT_CAPACITY: usize = 500;
/// 审计里 SQL 摘要的最大字符数。
pub const AUDIT_SQL_LIMIT: usize = 300;

/// 单个数据源的运行期状态。
pub struct DataSource {
    /// 数据源名（对外 `Database=<Name>`）
    pub name: String,
    /// 备注
    pub description: String,
    /// 访问令牌
    pub token: String,
    /// 只读（运行期可切换，重启恢复配置文件的值）
    pub read_only: AtomicBool,
    /// 数据访问层（打开失败时为 `None`）
    pub dal: Option<Arc<Dal>>,
    /// 打开失败原因
    pub open_error: String,
    /// 数据库类型（C# `DatabaseType` 规范名）
    pub kind: String,
    /// 连接串（**已脱敏**，仅用于面板展示）
    pub connection_masked: String,
    /// 请求统计
    pub stat: DbStat,
}

/// 单库请求统计（原子累加，热路径无锁）。
#[derive(Default)]
pub struct DbStat {
    /// 请求总数
    pub requests: AtomicU64,
    /// 失败数
    pub failures: AtomicU64,
    /// 累计耗时（微秒）
    pub total_us: AtomicU64,
    /// 最近一次错误
    pub last_error: Mutex<String>,
}

/// 数据源概览（CLI/面板共用的只读视图）。
pub struct SourceView {
    /// 名称
    pub name: String,
    /// 数据库类型
    pub kind: String,
    /// 是否只读
    pub read_only: bool,
    /// 是否配置了令牌
    pub token_set: bool,
    /// 是否可用
    pub available: bool,
    /// 打开失败原因
    pub error: String,
    /// 脱敏连接串
    pub connection: String,
}

/// 审计条目（内存环形 + 日志文件）。
#[derive(Clone)]
pub struct AuditEntry {
    /// 时间（`HH:MM:SS.mmm`）
    pub at: String,
    /// 数据源
    pub db: String,
    /// 操作（Query/Execute/InsertAndGetIdentity/QueryCount/GetTables/Login）
    pub op: String,
    /// 是否成功
    pub ok: bool,
    /// 耗时（微秒）
    pub micros: u64,
    /// 影响/返回行数
    pub rows: i64,
    /// 令牌指纹（`abcd****wxyz`）
    pub token: String,
    /// SQL 摘要
    pub sql: String,
    /// 错误信息
    pub error: String,
}

/// 代理实例（`Arc` 共享给 HTTP handler）。
pub struct Proxy {
    sources: Vec<DataSource>,
    /// 小写库名 → 下标
    by_name: BTreeMap<String, usize>,
    service: DbService,
    audit: Mutex<VecDeque<AuditEntry>>,
    admin_token: String,
    allow_remote: bool,
    started: Instant,
}

impl Proxy {
    /// 按配置构建（单个数据源连不上不影响整体启动，错误在面板可见）。
    pub fn open(config: &ProxyConfig) -> Result<Self, String> {
        let mut sources = Vec::with_capacity(config.databases.len());
        let mut by_name = BTreeMap::new();
        let mut service = DbService::new();

        for section in &config.databases {
            let source = open_source(section)?;
            let key = source.name.to_ascii_lowercase();
            if by_name.insert(key.clone(), sources.len()).is_some() {
                return Err(format!("数据源重名：{}（Name 忽略大小写后必须唯一）", source.name));
            }
            if !source.token.is_empty() {
                service.set_token(&source.token, &[source.name.as_str()]);
            }
            sources.push(source);
        }

        Ok(Self {
            sources,
            by_name,
            service,
            audit: Mutex::new(VecDeque::with_capacity(AUDIT_CAPACITY)),
            admin_token: config.server.admin_token.clone(),
            allow_remote: config.server.allow_remote,
            started: Instant::now(),
        })
    }

    /// 数据源数量
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// 数据源名列表（面板/CLI 展示）
    pub fn source_names(&self) -> Vec<&str> {
        self.sources.iter().map(|s| s.name.as_str()).collect()
    }

    /// 数据源概览（CLI `-status` 用；连接串已脱敏）。
    pub fn source_list(&self) -> Vec<SourceView> {
        self.sources
            .iter()
            .map(|s| SourceView {
                name: s.name.clone(),
                kind: s.kind.clone(),
                read_only: s.read_only.load(Ordering::Relaxed),
                token_set: !s.token.is_empty(),
                available: s.dal.is_some(),
                error: s.open_error.clone(),
                connection: s.connection_masked.clone(),
            })
            .collect()
    }

    /// 处理一个 HTTP 请求。
    pub fn handle(&self, req: HttpRequest) -> HttpResponse {
        // 非回环来源默认拒绝（防误暴露到公网）
        if !self.allow_remote && !is_loopback(req.remote_addr.as_deref()) {
            return fail(403, "本代理仅允许本机访问（配置 AllowRemote=true 可放开）");
        }
        let path = req.path.as_str();
        if let Some(rest) = path.strip_prefix("/Db/") {
            return self.handle_db(rest, &req);
        }
        match (req.method.as_str(), path) {
            ("GET", "/") | ("GET", "/index.html") => HttpResponse::bytes(
                200,
                "text/html; charset=utf-8",
                crate::panel::INDEX_HTML.as_bytes().to_vec(),
            ),
            ("GET", "/api/status") => match self.check_panel(&req) {
                Ok(()) => ok(self.status_json()),
                Err(resp) => resp,
            },
            ("GET", "/api/audit") => match self.check_panel(&req) {
                Ok(()) => ok(self.audit_json(&req.query)),
                Err(resp) => resp,
            },
            ("POST", "/api/toggle") => match self.check_panel(&req) {
                Ok(()) => self.toggle_read_only(&req),
                Err(resp) => resp,
            },
            ("GET", "/api/ping") => ok(json!({ "ok": true })),
            _ => fail(404, "未知路由"),
        }
    }

    /// `/Db/*` 协议处理（鉴权走各数据源的令牌）。
    fn handle_db(&self, action: &str, req: &HttpRequest) -> HttpResponse {
        // 协议响应必须**对齐 C# `DbController`**（`DbClient` 的解包规则）：
        //   成功 → 裸 `{data: ...}`（或 `{DbType, Version}` / 数组）；
        //   失败 → `{code, msg}`（`parse_envelope` 靠 code 非 0 抛出 msg）。
        // 局部函数遮蔽模块级的 `{ok,data}` 信封版本（那个只给面板 API 用）。
        fn ok(data: Value) -> HttpResponse {
            HttpResponse::json(200, data.to_string())
        }
        fn fail(status: u16, message: &str) -> HttpResponse {
            HttpResponse::json(status, json!({ "code": status, "msg": message }).to_string())
        }

        let action = action.trim_end_matches('/');
        let started = Instant::now();
        let body = parse_json(&req.body);

        // 取 (db, token)：POST 走 JSON，GET 走查询串
        let (db_name, token, sql, parameters) = if req.method == "POST" {
            (
                arg_str(&body, "db"),
                arg_str(&body, "token"),
                arg_str(&body, "sql"),
                arg_parameters(&body),
            )
        } else {
            let query = parse_query(&req.query);
            (
                query.get("db").cloned().unwrap_or_default(),
                query.get("token").cloned().unwrap_or_default(),
                String::new(),
                None,
            )
        };

        let Some(source) = self.source_of(&db_name) else {
            self.record(
                &db_name,
                action,
                &token,
                &sql,
                started,
                false,
                0,
                format!("数据源不存在：{db_name}"),
            );
            return fail(404, &format!("数据源不存在：{db_name}"));
        };

        // 令牌校验（对齐 C# `DbService.ValidateToken`）
        if let Err(e) = self.service.validate_token(&token, &source.name) {
            self.record(&source.name, action, &token, &sql, started, false, 0, e.to_string());
            return fail(401, &e.to_string());
        }
        let Some(dal) = source.dal.as_ref() else {
            let message = format!("数据源 {} 不可用：{}", source.name, source.open_error);
            self.record(&source.name, action, &token, &sql, started, false, 0, message.clone());
            return fail(503, &message);
        };

        // 只读护栏：写操作与写语句一律拒绝
        if source.read_only.load(Ordering::Relaxed) {
            let writing = match action {
                "Execute" | "InsertAndGetIdentity" => true,
                "Query" => !is_read_only_sql(&sql),
                _ => false,
            };
            if writing {
                let message = format!("数据源 {} 为只读模式，已拒绝写操作", source.name);
                self.record(&source.name, action, &token, &sql, started, false, 0, message.clone());
                return fail(403, &message);
            }
        }

        match (req.method.as_str(), action) {
            ("POST", "Login") => match source.dal.as_ref() {
                Some(dal) => {
                    let info = self.service.login_info(dal);
                    self.record(&source.name, action, &token, "", started, true, 0, String::new());
                    ok(json!({ "DbType": info.db_type, "Version": info.version }))
                }
                None => fail(503, "数据源不可用"),
            },
            ("POST", "Query") => match self
                .service
                .query_packet(dal, &sql, parameters.as_ref())
            {
                Ok(packet) => {
                    self.record(&source.name, action, &token, &sql, started, true, -1, String::new());
                    HttpResponse::bytes(200, "application/octet-stream", packet)
                }
                Err(e) => {
                    self.record(&source.name, action, &token, &sql, started, false, 0, e.to_string());
                    fail(500, &e.to_string())
                }
            },
            ("POST", "Execute") => match self.service.execute(dal, &sql, parameters.as_ref()) {
                Ok(n) => {
                    self.record(&source.name, action, &token, &sql, started, true, n as i64, String::new());
                    ok(json!({ "data": n }))
                }
                Err(e) => {
                    self.record(&source.name, action, &token, &sql, started, false, 0, e.to_string());
                    fail(500, &e.to_string())
                }
            },
            ("POST", "InsertAndGetIdentity") => {
                match self.service.insert_and_get_identity(dal, &sql, parameters.as_ref()) {
                    Ok(id) => {
                        self.record(&source.name, action, &token, &sql, started, true, 1, String::new());
                        ok(json!({ "data": id }))
                    }
                    Err(e) => {
                        self.record(&source.name, action, &token, &sql, started, false, 0, e.to_string());
                        fail(500, &e.to_string())
                    }
                }
            }
            ("GET", "QueryCount") => {
                let query = parse_query(&req.query);
                let table = query.get("tableName").cloned().unwrap_or_default();
                match self.service.query_count(dal, &table) {
                    Ok(n) => {
                        self.record(&source.name, action, &token, "", started, true, n, String::new());
                        ok(json!({ "data": n }))
                    }
                    Err(e) => {
                        self.record(&source.name, action, &token, "", started, false, 0, e.to_string());
                        fail(500, &e.to_string())
                    }
                }
            }
            ("GET", "GetTables") => match self.service.get_tables(dal) {
                Ok(tables) => {
                    let list: Vec<Value> = tables
                        .iter()
                        .map(|t| {
                            let columns: Vec<Value> = t
                                .columns
                                .iter()
                                .map(|c| json!({ "Name": t.effective_column_name(c) }))
                                .collect();
                            json!({ "Name": t.effective_table_name(), "Columns": columns })
                        })
                        .collect();
                    self.record(&source.name, action, &token, "", started, true, -1, String::new());
                    ok(Value::Array(list))
                }
                Err(e) => {
                    self.record(&source.name, action, &token, "", started, false, 0, e.to_string());
                    fail(500, &e.to_string())
                }
            },
            _ => fail(
                404,
                "未知路由（支持 Login/Query/Execute/InsertAndGetIdentity/QueryCount/GetTables）",
            ),
        }
    }

    /// 面板鉴权：配置了 `AdminToken` 时校验 Bearer；否则只允许回环。
    fn check_panel(&self, req: &HttpRequest) -> Result<(), HttpResponse> {
        if self.admin_token.is_empty() {
            if is_loopback(req.remote_addr.as_deref()) {
                return Ok(());
            }
            return Err(fail(401, "面板已启用登录令牌，请先配置 AdminToken 或从本机访问"));
        }
        let expected = format!("Bearer {}", self.admin_token);
        match req.header("authorization") {
            Some(value) if value == expected => Ok(()),
            _ => Err(fail(401, "面板令牌无效")),
        }
    }

    /// 数据源列表 + 统计（**裸数据**，由 [`ok`] 包一层信封）。
    fn status_json(&self) -> Value {
        let sources: Vec<Value> = self
            .sources
            .iter()
            .map(|s| {
                let requests = s.stat.requests.load(Ordering::Relaxed);
                let total = s.stat.total_us.load(Ordering::Relaxed);
                json!({
                    "name": s.name,
                    "description": s.description,
                    "kind": s.kind,
                    "readOnly": s.read_only.load(Ordering::Relaxed),
                    "available": s.dal.is_some(),
                    "error": s.open_error,
                    "connection": s.connection_masked,
                    "tokenSet": !s.token.is_empty(),
                    "requests": requests,
                    "failures": s.stat.failures.load(Ordering::Relaxed),
                    "avgUs": total.checked_div(requests).unwrap_or(0),
                    "lastError": s.stat.last_error.lock().map(|e| e.clone()).unwrap_or_default(),
                })
            })
            .collect();
        json!({
            "uptimeSecs": self.started.elapsed().as_secs(),
            "allowRemote": self.allow_remote,
            "panelTokenSet": !self.admin_token.is_empty(),
            "sources": sources,
        })
    }

    /// 审计记录（`?n=` 限制条数，默认 100）。
    fn audit_json(&self, query: &str) -> Value {
        let params = parse_query(query);
        let limit = params
            .get("n")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(100)
            .min(AUDIT_CAPACITY);
        let entries: Vec<Value> = self
            .audit
            .lock()
            .map(|items| {
                items
                    .iter()
                    .rev()
                    .take(limit)
                    .map(|e| {
                        json!({
                            "at": e.at, "db": e.db, "op": e.op, "ok": e.ok,
                            "micros": e.micros, "rows": e.rows, "token": e.token,
                            "sql": e.sql, "error": e.error,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!(entries)
    }

    /// 运行期切换只读（不写回配置文件）。
    fn toggle_read_only(&self, req: &HttpRequest) -> HttpResponse {
        let body = parse_json(&req.body);
        let name = arg_str(&body, "db");
        let read_only = body.get("readOnly").and_then(Value::as_bool).unwrap_or(true);
        let Some(source) = self.source_of(&name) else {
            return fail(404, &format!("数据源不存在：{name}"));
        };
        source.read_only.store(read_only, Ordering::Relaxed);
        dhrust::info!(
            "面板切换只读：db={} readOnly={}（仅运行期生效，重启恢复配置文件）",
            source.name,
            read_only
        );
        ok(json!({ "ok": true, "db": source.name, "readOnly": read_only }))
    }

    /// 按名找数据源（忽略大小写）。
    fn source_of(&self, name: &str) -> Option<&DataSource> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|index| &self.sources[*index])
    }

    /// 记账：统计 + 审计（内存环形 + 日志文件）。
    #[allow(clippy::too_many_arguments)]
    fn record(
        &self,
        db: &str,
        op: &str,
        token: &str,
        sql: &str,
        started: Instant,
        ok: bool,
        rows: i64,
        error: String,
    ) {
        let micros = started.elapsed().as_micros() as u64;
        if let Some(source) = self.source_of(db) {
            source.stat.requests.fetch_add(1, Ordering::Relaxed);
            source.stat.total_us.fetch_add(micros, Ordering::Relaxed);
            if !ok {
                source.stat.failures.fetch_add(1, Ordering::Relaxed);
                if let Ok(mut slot) = source.stat.last_error.lock() {
                    *slot = error.clone();
                }
            }
        }
        let summary = summarize_sql(sql);
        let entry = AuditEntry {
            at: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            db: db.to_string(),
            op: op.to_string(),
            ok,
            micros,
            rows,
            token: mask_token(token),
            sql: summary.clone(),
            error: error.clone(),
        };
        if let Ok(mut items) = self.audit.lock() {
            if items.len() == AUDIT_CAPACITY {
                items.pop_front();
            }
            items.push_back(entry);
        }
        if ok {
            dhrust::info!(
                "db={db} op={op} ok rows={rows} {micros}us sql={summary}"
            );
        } else {
            dhrust::warn!("db={db} op={op} FAIL {micros}us sql={summary} err={error}");
        }
    }
}

/// 打开单个数据源（失败只记错误，不影响其他数据源）。
fn open_source(section: &DatabaseSection) -> Result<DataSource, String> {
    let name = section.name.trim().to_string();
    if name.is_empty() {
        return Err("数据源 Name 不能为空".into());
    }
    let connection = resolve_connection(&section.connection)?;
    let (dal, open_error, kind) = match Dal::open(&connection) {
        Ok(dal) => {
            let kind = dal.kind().name().to_string();
            (Some(Arc::new(dal)), String::new(), kind)
        }
        Err(e) => (None, e.to_string(), String::new()),
    };
    Ok(DataSource {
        name,
        description: section.description.clone(),
        token: section.token.clone(),
        read_only: AtomicBool::new(section.read_only),
        dal,
        open_error,
        kind,
        connection_masked: mask_connection(&connection),
        stat: DbStat::default(),
    })
}

/// 解析连接串：`enc:v1:` 加密值需要用本机密钥解密。
fn resolve_connection(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if !dhrust::secret::is_encrypted(raw) {
        return Ok(raw.to_string());
    }
    let key = load_key()?;
    dhrust::secret::decrypt(raw, &key).map_err(|e| format!("连接串解密失败：{e}"))
}

/// 读取本机密钥（环境变量 `PEK_RDBPROXY_KEY` 优先，其次 `Config/.key`）。
pub fn load_key() -> Result<[u8; 32], String> {
    if let Ok(seed) = std::env::var("PEK_RDBPROXY_KEY")
        && !seed.trim().is_empty()
    {
        return Ok(dhrust::secret::key_from_seed(seed.trim()));
    }
    let path = crate::base_dir().join("Config/.key");
    let seed = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "连接串是加密值，但找不到密钥（设置环境变量 PEK_RDBPROXY_KEY，或先用 -encrypt 生成 {}）",
            path.display()
        )
    })?;
    let seed = seed.trim();
    if seed.is_empty() {
        return Err(format!("密钥文件为空：{}", path.display()));
    }
    Ok(dhrust::secret::key_from_seed(seed))
}

/// 校验语句是否为只读（`SELECT`/`SHOW`/`DESC`/`EXPLAIN`/`PRAGMA`；`WITH` 需不含写关键字）。
pub fn is_read_only_sql(sql: &str) -> bool {
    let trimmed = sql.trim_start();
    let head = trimmed
        .split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .find(|part| !part.is_empty())
        .unwrap_or("")
        .to_ascii_uppercase();
    match head.as_str() {
        "SELECT" | "SHOW" | "DESC" | "DESCRIBE" | "EXPLAIN" | "PRAGMA" | "VALUES" => true,
        "WITH" => !contains_write_keyword(trimmed),
        _ => false,
    }
}

/// 是否包含写关键字（CTE 里可能夹带写操作）。
fn contains_write_keyword(sql: &str) -> bool {
    const WRITE: [&str; 12] = [
        "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "CREATE", "ALTER", "DROP", "TRUNCATE",
        "GRANT", "REVOKE", "CALL",
    ];
    let upper = sql.to_ascii_uppercase();
    upper
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|token| !token.is_empty())
        .any(|token| WRITE.contains(&token))
}

/// 令牌指纹（保留首尾各 4 位）。
fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.is_empty() {
        return "-".to_string();
    }
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}****{tail}")
}

/// SQL 摘要（压空白 + 截断）。
fn summarize_sql(sql: &str) -> String {
    let squashed = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() <= AUDIT_SQL_LIMIT {
        return squashed;
    }
    let truncated: String = squashed.chars().take(AUDIT_SQL_LIMIT).collect();
    format!("{truncated}…")
}

/// 是否回环来源（未知来源按回环处理，便于本地测试构造的请求）。
fn is_loopback(addr: Option<&str>) -> bool {
    let Some(addr) = addr else {
        return true;
    };
    match addr.parse::<SocketAddr>() {
        Ok(socket) => socket.ip().is_loopback(),
        Err(_) => addr.starts_with("127.") || addr == "::1" || addr.starts_with("[::1]"),
    }
}

/// 解析 JSON 请求体（非法返回空对象）。
fn parse_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
}

/// 取字符串参数。
fn arg_str(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 取参数字典（兼容 JSON 字符串与对象两种形态）。
fn arg_parameters(args: &Value) -> Option<BTreeMap<String, Value>> {
    let value = args.get("parameters")?;
    if value.is_null() {
        return None;
    }
    let parsed: Value = match value {
        Value::String(text) if !text.is_empty() => serde_json::from_str(text).ok()?,
        Value::String(_) => return None,
        other => other.clone(),
    };
    parsed
        .as_object()
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// 解析查询串（`a=1&b=2`，百分号解码）。
fn parse_query(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for pair in text.trim_start_matches('?').split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(key), percent_decode(value));
    }
    map
}

/// 百分号解码（`+` 视为空格）。
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `{ok:true, data}` 信封。
fn ok(data: Value) -> HttpResponse {
    HttpResponse::json(200, json!({ "ok": true, "data": data }).to_string())
}

/// 错误信封（`{ok:false, message}` + 对应状态码）。
fn fail(status: u16, message: &str) -> HttpResponse {
    HttpResponse::json(status, json!({ "ok": false, "message": message }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_detection() {
        assert!(is_read_only_sql("SELECT * FROM t"));
        assert!(is_read_only_sql("  select 1"));
        assert!(is_read_only_sql("SHOW TABLES"));
        assert!(is_read_only_sql("PRAGMA table_info(t)"));
        assert!(is_read_only_sql("(SELECT 1)"));
        assert!(!is_read_only_sql("INSERT INTO t VALUES (1)"));
        assert!(!is_read_only_sql("update t set a=1"));
        assert!(!is_read_only_sql("DELETE FROM t"));
        assert!(!is_read_only_sql("DROP TABLE t"));
        assert!(
            !is_read_only_sql("WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x"),
            "CTE 里夹带写操作必须拒绝"
        );
        assert!(is_read_only_sql("WITH x AS (SELECT 1) SELECT * FROM x"));
    }

    #[test]
    fn token_mask_keeps_edges() {
        assert_eq!(mask_token("abcdefghijkl"), "abcd****ijkl");
        assert_eq!(mask_token("short"), "****");
        assert_eq!(mask_token(""), "-");
    }

    #[test]
    fn sql_summary_squashes_and_truncates() {
        assert_eq!(summarize_sql("SELECT\n  *\tFROM   t"), "SELECT * FROM t");
        let long = "SELECT ".to_string() + &"a".repeat(400);
        let summary = summarize_sql(&long);
        assert_eq!(summary.chars().count(), AUDIT_SQL_LIMIT + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback(None));
        assert!(is_loopback(Some("127.0.0.1:5000")));
        assert!(is_loopback(Some("[::1]:5000")));
        assert!(!is_loopback(Some("10.0.0.5:5000")));
    }

    #[test]
    fn query_parsing_decodes() {
        let map = parse_query("db=de%6Do&token=a+b");
        assert_eq!(map.get("db").map(String::as_str), Some("demo"));
        assert_eq!(map.get("token").map(String::as_str), Some("a b"));
    }
}
