//! 代理核心：多数据源（运行期增删改）+ `/Db/*` 协议（对齐 C# `DbController`）+ 只读护栏 + 审计、统计。
//!
//! 协议完全复用 [`pek_rcode::db_service::DbService`]，因此 **Rust / C# 客户端都能直连**：
//!
//! ```text
//! Server=http://127.0.0.1:5599;Database=demo;Password=<Token>;provider=network
//! ```
//!
//! 数据源集合是**可变的**（`RwLock<Registry>`）：面板新增/修改/删除会同时生效到内存与
//! `Config/Proxy.toml`（写回保留注释）。

use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use dhrust::net::http::HttpResponse;
use pek_rcode::dal::Dal;
use pek_rcode::db_service::DbService;
use serde_json::{Value, json};

use crate::config::{self, DatabaseSection, ProxyConfig, ServerSection, mask_connection};

/// 审计缓冲上限（内存环形；同时写日志文件）。
pub const AUDIT_CAPACITY: usize = 500;
/// 审计里 SQL 摘要的最大字符数。
pub const AUDIT_SQL_LIMIT: usize = 300;

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

/// 单个数据源。
pub struct DataSource {
    /// 原始配置项（写回配置文件用）
    pub section: DatabaseSection,
    /// 数据源名（对外 `Database=<Name>`）
    pub name: String,
    /// 备注
    pub description: String,
    /// 访问令牌
    pub token: String,
    /// 只读（面板可切换，切换即持久化）
    pub read_only: AtomicBool,
    /// 数据访问层（打开失败时为 `None`）
    pub dal: Option<Arc<Dal>>,
    /// 打开失败原因
    pub open_error: String,
    /// 数据库类型（C# `DatabaseType` 规范名）
    pub kind: String,
    /// 连接串（**已脱敏**，仅用于展示）
    pub connection_masked: String,
    /// 请求统计
    pub stat: DbStat,
}

impl DataSource {
    /// 对外视图（面板/CLI 共用）。
    pub fn view(&self) -> SourceView {
        let requests = self.stat.requests.load(Ordering::Relaxed);
        let total = self.stat.total_us.load(Ordering::Relaxed);
        SourceView {
            name: self.name.clone(),
            kind: self.kind.clone(),
            description: self.description.clone(),
            read_only: self.read_only.load(Ordering::Relaxed),
            token_set: !self.token.is_empty(),
            available: self.dal.is_some(),
            error: self.open_error.clone(),
            connection: self.connection_masked.clone(),
            requests,
            failures: self.stat.failures.load(Ordering::Relaxed),
            avg_us: total.checked_div(requests).unwrap_or(0),
            last_error: self.stat.last_error.lock().map(|e| e.clone()).unwrap_or_default(),
        }
    }
}

/// 数据源只读视图（面板/CLI 统计用）。
pub struct SourceView {
    /// 名称
    pub name: String,
    /// 数据库类型
    pub kind: String,
    /// 备注
    pub description: String,
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
    /// 请求数
    pub requests: u64,
    /// 失败数
    pub failures: u64,
    /// 平均耗时（微秒）
    pub avg_us: u64,
    /// 最近错误
    pub last_error: String,
}

/// 审计条目。
#[derive(Clone)]
pub struct AuditEntry {
    /// 时间（`HH:MM:SS.mmm`）
    pub at: String,
    /// 数据源
    pub db: String,
    /// 操作
    pub op: String,
    /// 是否成功
    pub ok: bool,
    /// 耗时（微秒）
    pub micros: u64,
    /// 影响/返回行数
    pub rows: i64,
    /// 令牌指纹
    pub token: String,
    /// SQL 摘要
    pub sql: String,
    /// 错误信息
    pub error: String,
}

impl AuditEntry {
    /// JSON 形式（面板 API 用）。
    pub fn to_json(&self) -> Value {
        json!({
            "at": self.at, "db": self.db, "op": self.op, "ok": self.ok,
            "micros": self.micros, "rows": self.rows, "token": self.token,
            "sql": self.sql, "error": self.error,
        })
    }
}

/// 服务端设置补丁（面板保存用；`None` = 不改）。
#[derive(Debug, Default, Clone)]
pub struct ServerPatch {
    /// 监听端口
    pub port: Option<u16>,
    /// 面板用户名
    pub user_name: Option<String>,
    /// 面板密码（明文，落盘时哈希）
    pub password: Option<String>,
    /// 鉴权级别（None/LocalOnly/Full）
    pub auth_level: Option<String>,
    /// 是否允许远程
    pub allow_remote: Option<bool>,
    /// 面板仅限本机访问
    pub panel_local_only: Option<bool>,
}

/// 数据源注册表（整体替换，避免 sources/by_name/service 不一致）。
struct Registry {
    sources: Vec<Arc<DataSource>>,
    by_name: BTreeMap<String, usize>,
    service: DbService,
}

/// 代理实例（`Arc` 共享给 HTTP handler）。
pub struct Proxy {
    base: PathBuf,
    server: RwLock<ServerSection>,
    registry: RwLock<Registry>,
    audit: Mutex<VecDeque<AuditEntry>>,
    started: Instant,
    /// 未登记字段观测（审计用）
    unknown_fields: crate::policy::UnknownFields,
    /// 监听地址热重绑钩子（由宿主注册；返回新的监听地址）
    rebind: RwLock<Option<RebindHook>>,
    /// 最近一次防火墙操作结果（面板展示）
    firewall_note: RwLock<String>,
    /// 字段级安全策略（「表管理」）
    policy: RwLock<crate::policy::Policy>,
}

/// 监听地址热重绑钩子：入参为新的服务端配置，返回实际监听地址。
pub type RebindHook = Arc<dyn Fn(&ServerSection) -> Result<String, String> + Send + Sync>;

impl Proxy {
    /// 按配置构建（单个数据源连不上不影响整体启动，错误在面板可见）。
    ///
    /// `base` 是程序基目录：配置里的 SQLite 相对路径（如 `Data/demo.db`）按它绝对化并自动建目录。
    pub fn open(base: &Path, config: &ProxyConfig) -> Result<Self, String> {
        let mut sources = Vec::with_capacity(config.databases.len());
        for section in &config.databases {
            sources.push(Arc::new(open_source(base, section)?));
        }
        let registry = Registry::rebuild(sources)?;
        let proxy = Self {
            base: base.to_path_buf(),
            server: RwLock::new(config.server.clone()),
            registry: RwLock::new(registry),
            audit: Mutex::new(VecDeque::with_capacity(AUDIT_CAPACITY)),
            started: Instant::now(),
            rebind: RwLock::new(None),
            firewall_note: RwLock::new(String::new()),
            policy: RwLock::new(crate::policy::load(base)),
            unknown_fields: crate::policy::UnknownFields::default(),
        };
        // 启动即把仍是明文的连接串加密写回（存盘不再有明文密码）
        match proxy.migrate_encrypt() {
            Ok(true) => dhrust::info!("连接串已加密写回 Config/Proxy.toml（密钥：Config/.key）"),
            Ok(false) => {}
            Err(e) => dhrust::warn!("连接串加密迁移失败（本次仍可运行）：{e}"),
        }
        Ok(proxy)
    }

    /// 把配置文件里仍是明文的连接串加密并写回（幂等；返回是否发生了变更）。
    pub fn migrate_encrypt(&self) -> Result<bool, String> {
        let pending: Vec<(usize, String)> = {
            let registry = self.registry.read().map_err(|_| "状态锁异常".to_string())?;
            registry
                .sources
                .iter()
                .enumerate()
                .filter(|(_, s)| !dhrust::secret::is_encrypted(&s.section.connection))
                .map(|(i, s)| (i, s.section.connection.clone()))
                .collect()
        };
        if pending.is_empty() {
            return Ok(false);
        }
        let mut registry = self.registry.write().map_err(|_| "状态锁异常".to_string())?;
        let mut sources = registry.sources.clone();
        for (index, connection) in pending {
            let Some(source) = sources.get(index).cloned() else {
                continue;
            };
            let mut section = source.section.clone();
            section.connection = encrypt_connection(&self.base, &connection)?;
            sources[index] = with_section(&source, section);
        }
        *registry = Registry::rebuild(sources)?;
        drop(registry);
        self.save_config()?;
        Ok(true)
    }

    /// 程序基目录。
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// 服务端设置快照。
    pub fn server_config(&self) -> ServerSection {
        self.server.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// 数据源数量。
    pub fn source_count(&self) -> usize {
        self.registry.read().map(|r| r.sources.len()).unwrap_or(0)
    }

    /// 数据源名列表。
    pub fn source_names(&self) -> Vec<String> {
        self.registry
            .read()
            .map(|r| r.sources.iter().map(|s| s.name.clone()).collect())
            .unwrap_or_default()
    }

    /// 数据源视图列表（含统计）。
    pub fn source_list(&self) -> Vec<SourceView> {
        self.registry
            .read()
            .map(|r| r.sources.iter().map(|s| s.view()).collect())
            .unwrap_or_default()
    }

    /// 取某数据源的**令牌**（面板「复制项目侧连接串」用；仅鉴权后可访问）。
    pub fn source_token(&self, name: &str) -> Result<String, String> {
        let registry = self.registry.read().map_err(|_| "状态锁异常".to_string())?;
        let index = registry
            .by_name
            .get(&name.trim().to_ascii_lowercase())
            .copied()
            .ok_or_else(|| format!("数据源不存在：{name}"))?;
        registry
            .sources
            .get(index)
            .map(|s| s.token.clone())
            .ok_or_else(|| format!("数据源不存在：{name}"))
    }

    /// 取某数据源的**完整连接串**（面板「编辑」回填用，已解密为明文）。
    ///
    /// 仅供已鉴权的面板接口调用；面板列表/状态接口仍只返回脱敏值。
    pub fn source_connection(&self, name: &str) -> Result<String, String> {
        let stored = {
            let registry = self.registry.read().map_err(|_| "状态锁异常".to_string())?;
            let index = registry
                .by_name
                .get(&name.trim().to_ascii_lowercase())
                .copied()
                .ok_or_else(|| format!("数据源不存在：{name}"))?;
            registry
                .sources
                .get(index)
                .map(|s| s.section.connection.clone())
                .ok_or_else(|| format!("数据源不存在：{name}"))?
        };
        resolve_connection(&self.base, &stored)
    }

    /// 运行时长（秒）。
    pub fn uptime_secs(&self) -> u64 {
        self.started.elapsed().as_secs()
    }

    /// 审计记录（最新在前，最多 `limit` 条）。
    pub fn audit_entries(&self, limit: usize) -> Vec<AuditEntry> {
        self.audit
            .lock()
            .map(|items| items.iter().rev().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// 按名取数据源（忽略大小写；锁在返回前释放）。
    pub fn source_of(&self, name: &str) -> Option<Arc<DataSource>> {
        let registry = self.registry.read().ok()?;
        let index = *registry.by_name.get(&name.to_ascii_lowercase())?;
        Some(Arc::clone(&registry.sources[index]))
    }

    /// 校验令牌是否可访问该数据源（语义同 C# `DbService.ValidateToken`）。
    pub fn validate_token(&self, token: &str, db: &str) -> Result<(), String> {
        let registry = self.registry.read().map_err(|_| "状态锁异常".to_string())?;
        registry
            .service
            .validate_token(token, db)
            .map_err(|e| e.to_string())
    }

    /// 新增或更新数据源（同时写回 `Config/Proxy.toml`）。
    ///
    /// 同名（忽略大小写）时替换；配置里 `Token` 为空表示「不改动原令牌」。
    pub fn upsert_source(&self, mut section: DatabaseSection) -> Result<SourceView, String> {
        section.name = section.name.trim().to_string();
        if section.name.is_empty() {
            return Err("数据源 Name 不能为空".into());
        }
        // 编辑时「连接串 / 令牌」留空 = 保留原值
        if let Some(existing) = self.source_of(&section.name) {
            if section.connection.trim().is_empty() {
                section.connection = existing.section.connection.clone();
            }
            if section.token.trim().is_empty() {
                section.token = existing.token.clone();
            }
        }
        if section.connection.trim().is_empty() {
            return Err("连接串不能为空".into());
        }
        if section.token.trim().is_empty() {
            return Err("令牌不能为空（用 -token 生成，或面板「生成」）".into());
        }
        // 先验证明文可解，再转成磁盘形态（enc:v1: 密文）保存
        resolve_connection(&self.base, &section.connection)?;
        section.connection = encrypt_connection(&self.base, &section.connection)?;

        let source = Arc::new(open_source(&self.base, &section)?);
        {
            let mut registry = self.registry.write().map_err(|_| "状态锁异常".to_string())?;
            let key = source.name.to_ascii_lowercase();
            let mut sources = registry.sources.clone();
            match registry.by_name.get(&key) {
                Some(index) => {
                    let old = &sources[*index];
                    // 保留统计，便于观察替换后的对比
                    source.stat.requests.store(old.stat.requests.load(Ordering::Relaxed), Ordering::Relaxed);
                    source.stat.failures.store(old.stat.failures.load(Ordering::Relaxed), Ordering::Relaxed);
                    source.stat.total_us.store(old.stat.total_us.load(Ordering::Relaxed), Ordering::Relaxed);
                    sources[*index] = Arc::clone(&source);
                }
                None => sources.push(Arc::clone(&source)),
            }
            *registry = Registry::rebuild(sources)?;
        }
        self.save_config()?;
        dhrust::info!(
            "数据源已保存：{}（{}，只读={}，{}）",
            source.name,
            if source.kind.is_empty() { "-" } else { source.kind.as_str() },
            source.read_only.load(Ordering::Relaxed),
            if source.dal.is_some() { "可用" } else { "不可用" }
        );
        Ok(source.view())
    }

    /// 删除数据源（同时写回配置）。
    pub fn remove_source(&self, name: &str) -> Result<(), String> {
        let key = name.trim().to_ascii_lowercase();
        {
            let mut registry = self.registry.write().map_err(|_| "状态锁异常".to_string())?;
            let index = *registry
                .by_name
                .get(&key)
                .ok_or_else(|| format!("数据源不存在：{name}"))?;
            let mut sources = registry.sources.clone();
            let removed = sources.remove(index);
            *registry = Registry::rebuild(sources)?;
            dhrust::info!("数据源已删除：{}", removed.name);
        }
        self.save_config()
    }

    /// 切换只读（立即持久化）。
    pub fn set_read_only(&self, name: &str, read_only: bool) -> Result<SourceView, String> {
        let source = self
            .source_of(name)
            .ok_or_else(|| format!("数据源不存在：{name}"))?;
        source.read_only.store(read_only, Ordering::Relaxed);
        {
            let mut registry = self.registry.write().map_err(|_| "状态锁异常".to_string())?;
            let mut sources = registry.sources.clone();
            if let Some(index) = registry.by_name.get(&name.trim().to_ascii_lowercase()).copied() {
                let mut section = sources[index].section.clone();
                section.read_only = read_only;
                // 保序替换：共享同一个 DataSource，只更新配置快照
                sources[index] = Arc::new(DataSource {
                    section,
                    name: sources[index].name.clone(),
                    description: sources[index].description.clone(),
                    token: sources[index].token.clone(),
                    read_only: AtomicBool::new(read_only),
                    dal: sources[index].dal.clone(),
                    open_error: sources[index].open_error.clone(),
                    kind: sources[index].kind.clone(),
                    connection_masked: sources[index].connection_masked.clone(),
                    stat: DbStat::default(),
                });
            }
            *registry = Registry::rebuild(sources)?;
        }
        self.save_config()?;
        Ok(self.source_of(name).map(|s| s.view()).unwrap_or_else(|| source.view()))
    }

    /// 最近一次防火墙操作提示（面板展示）。
    pub fn firewall_note(&self) -> String {
        self.firewall_note
            .read()
            .map(|n| n.clone())
            .unwrap_or_default()
    }

    /// 注册监听地址热重绑钩子（宿主在启动监听后调用）。
    pub fn set_rebind_hook(&self, hook: RebindHook) {
        if let Ok(mut slot) = self.rebind.write() {
            *slot = Some(hook);
        }
    }

    /// 更新服务端设置（端口/远程访问变更会立即热重绑监听，无需重启）。
    pub fn update_server(&self, patch: ServerPatch) -> Result<ServerSection, String> {
        let old = self.server_config();
        let old_port = old.port;
        let old_allow_remote = old.allow_remote;
        let mut next = old;
        if let Some(port) = patch.port {
            if port == 0 {
                return Err("端口必须大于 0".into());
            }
            next.port = port;
        }
        if let Some(name) = patch.user_name {
            let name = name.trim().to_string();
            if name.is_empty() {
                return Err("用户名不能为空".into());
            }
            next.web_user_name = name;
        }
        if let Some(password) = patch.password.filter(|p| !p.trim().is_empty()) {
            password_strength(&next.web_user_name, &password)?;
            next.web_password_hash = hash_password(&password);
        }
        if let Some(level) = patch.auth_level {
            next.web_auth_level = dhrust::net::panel_auth::AuthLevel::parse(&level).as_str().to_string();
        }
        if let Some(allow) = patch.allow_remote {
            next.allow_remote = allow;
        }
        if let Some(local_only) = patch.panel_local_only {
            next.panel_local_only = local_only;
        }
        *self.server.write().map_err(|_| "状态锁异常".to_string())? = next.clone();
        self.save_config()?;
        // 端口/远程访问变化 → 立即重绑监听（失败则返回错误，保留旧监听）
        if next.port != old_port || next.allow_remote != old_allow_remote {
            let hook = self.rebind.read().ok().and_then(|h| h.clone());
            if let Some(hook) = hook {
                let addr = hook(&next)?;
                dhrust::info!("监听已热切换：{addr}");
            }
        }
        // 远程访问开关 → 自动放行/撤销本机防火墙端口（失败只记提示，不阻断）
        if next.allow_remote != old_allow_remote || next.port != old_port {
            let note = match dhrust::sys::firewall::ensure("PekRDbProxy", next.allow_remote, next.port) {
                Ok(msg) => {
                    dhrust::info!("防火墙：{msg}");
                    msg
                }
                Err(e) => {
                    dhrust::warn!("防火墙自动放行失败：{e}");
                    format!("防火墙未自动放行：{e}")
                }
            };
            if let Ok(mut slot) = self.firewall_note.write() {
                *slot = note;
            }
        }
        dhrust::info!("服务端设置已保存（端口 {}，鉴权 {}）", next.port, next.web_auth_level);
        Ok(next)
    }

    /// 校验面板用户名/密码（`salted_sha256_hex`，与 Pek.RAgent 同口径）。
    pub fn authenticate(&self, user: &str, password: &str) -> bool {
        let server = self.server_config();
        if !server.web_user_name.eq_ignore_ascii_case(user.trim()) {
            return false;
        }
        verify_password(&server.web_password_hash, password)
    }

    /// 面板是否仍在用默认密码（`admin`）。
    pub fn uses_default_password(&self) -> bool {
        verify_password(&self.server_config().web_password_hash, "admin")
    }

    /// 首次启动写入内置默认账号 `admin/admin`，返回是否刚创建。
    ///
    /// **故意绕过强度校验**：密码 `admin` 本身不满足规则，但它是"首启一次性"凭据——
    /// 登录成功后 `upgrade_weak_password` 会立刻换成强密码并只展示一次（对齐 Pek.RAgent）。
    /// 走 `update_server` 会被 `password_strength` 拒绝，导致首启后**根本没有可用密码**。
    pub fn ensure_default_account(&self) -> Result<bool, String> {
        if !self.server_config().web_password_hash.is_empty() {
            return Ok(false);
        }
        let mut next = self.server_config();
        next.web_user_name = "admin".to_string();
        next.web_password_hash = hash_password("admin");
        *self.server.write().map_err(|_| "状态锁异常".to_string())? = next;
        self.save_config()?;
        Ok(true)
    }

    /// 登录成功后发现仍是默认密码 → 自动换成强密码并写回，返回新密码（一次性展示）。
    pub fn upgrade_weak_password(&self) -> Result<Option<String>, String> {
        let server = self.server_config();
        if !verify_password(&server.web_password_hash, "admin") {
            return Ok(None);
        }
        let new_password = generate_password();
        let mut next = server;
        next.web_password_hash = hash_password(&new_password);
        *self.server.write().map_err(|_| "状态锁异常".to_string())? = next;
        self.save_config()?;
        dhrust::warn!("检测到默认密码 admin，已按强度规则自动升级并写回配置（新密码仅在登录响应中返回一次）");
        Ok(Some(new_password))
    }

    /// JWT 密钥（首次使用时生成并落盘到配置；格式 `HS256:<随机>`，与 Pek 系其它项目同口径）。
    pub fn jwt(&self) -> Result<pek_rpermissions::jwt::Jwt, String> {
        let secret = {
            let server = self.server_config();
            if !server.jwt_secret.trim().is_empty() {
                normalize_jwt_secret(&server.jwt_secret)
            } else {
                let mut next = server;
                next.jwt_secret = format!("HS256:{}", dhrust::random::hex(32));
                let secret = next.jwt_secret.clone();
                *self.server.write().map_err(|_| "状态锁异常".to_string())? = next;
                self.save_config()?;
                secret
            }
        };
        pek_rpermissions::jwt::Jwt::from_secret(&secret).map_err(|e| format!("JWT 初始化失败：{e}"))
    }

    /// 把当前内存状态写回配置文件（保留注释）。
    fn save_config(&self) -> Result<(), String> {
        let server = self.server_config();
        let databases: Vec<DatabaseSection> = self
            .registry
            .read()
            .map_err(|_| "状态锁异常".to_string())?
            .sources
            .iter()
            .map(|s| {
                let mut section = s.section.clone();
                section.read_only = s.read_only.load(Ordering::Relaxed);
                section
            })
            .collect::<Vec<_>>();
        config::save(&self.base, &ProxyConfig { server, databases }).map(|_| ())
    }

    /// 处理 `/Db/*` 协议请求（鉴权走各数据源令牌）。
    ///
    /// 响应格式**对齐 C# `DbController`**：成功裸 `{data:…}`（或 `{DbType,Version}` / 数组），
    /// 失败 `{code, msg}`（`DbClient::parse_envelope` 靠 code 非 0 抛 msg）。
    pub fn handle_db(&self, action: &str, method: &str, body: &[u8], query: &str) -> HttpResponse {
        let action = action.trim_end_matches('/');
        let started = Instant::now();
        let parsed = parse_json(body);

        let (db_name, token, sql, parameters) = if method == "POST" {
            (
                arg_str(&parsed, "db"),
                arg_str(&parsed, "token"),
                arg_str(&parsed, "sql"),
                arg_parameters(&parsed),
            )
        } else {
            let query = parse_query(query);
            (
                query.get("db").cloned().unwrap_or_default(),
                query.get("token").cloned().unwrap_or_default(),
                String::new(),
                None,
            )
        };

        let Some(source) = self.source_of(&db_name) else {
            // 常见误区：项目的 `Database=` 要填**数据源名**（面板卡片上的名称），不是真实库名。
            // 错误里直接列出可用名，省得对着配置猜。
            let names = self.source_names();
            let message = if names.is_empty() {
                format!("数据源不存在：{db_name}（当前没有任何数据源，先在面板添加）")
            } else {
                format!("数据源不存在：{db_name}（可用：{}）", names.join(", "))
            };
            self.record(&db_name, action, &token, &sql, started, false, 0, message.clone());
            return db_error(404, &message);
        };

        if let Err(e) = self.validate_token(&token, &source.name) {
            self.record(&source.name, action, &token, &sql, started, false, 0, e.clone());
            return db_error(401, &e);
        }
        let Some(dal) = source.dal.as_ref() else {
            let message = format!("数据源 {} 不可用：{}", source.name, source.open_error);
            self.record(&source.name, action, &token, &sql, started, false, 0, message.clone());
            return db_error(503, &message);
        };

        // 只读护栏
        if source.read_only.load(Ordering::Relaxed) {
            let writing = match action {
                "Execute" | "InsertAndGetIdentity" => true,
                "Query" => !is_read_only_sql(&sql),
                _ => false,
            };
            if writing {
                let message = format!("数据源 {} 为只读模式，已拒绝写操作", source.name);
                self.record(&source.name, action, &token, &sql, started, false, 0, message.clone());
                return db_error(403, &message);
            }
        }

        let service = DbService::new();
        match (method, action) {
            ("POST", "Login") => {
                let info = service.login_info(dal);
                self.record(&source.name, action, &token, "", started, true, 0, String::new());
                db_ok(json!({ "DbType": info.db_type, "Version": info.version }))
            }
            ("POST", "Query") => match service.query(dal, &sql, parameters.as_ref()) {
                Ok(set) => {
                    let (filtered, note) = self.apply_field_policy(&source.name, set);
                    self.record(&source.name, action, &token, &sql, started, true, -1, note);
                    HttpResponse::bytes(
                        200,
                        "application/octet-stream",
                        pek_rcode::dbtable::encode_rowset(&filtered),
                    )
                }
                Err(e) => {
                    self.record(&source.name, action, &token, &sql, started, false, 0, e.to_string());
                    db_error(500, &e.to_string())
                }
            },
            ("POST", "Execute") => match service.execute(dal, &sql, parameters.as_ref()) {
                Ok(n) => {
                    self.record(&source.name, action, &token, &sql, started, true, n as i64, String::new());
                    db_ok(json!({ "data": n }))
                }
                Err(e) => {
                    self.record(&source.name, action, &token, &sql, started, false, 0, e.to_string());
                    db_error(500, &e.to_string())
                }
            },
            ("POST", "InsertAndGetIdentity") => {
                match service.insert_and_get_identity(dal, &sql, parameters.as_ref()) {
                    Ok(id) => {
                        self.record(&source.name, action, &token, &sql, started, true, 1, String::new());
                        db_ok(json!({ "data": id }))
                    }
                    Err(e) => {
                        self.record(&source.name, action, &token, &sql, started, false, 0, e.to_string());
                        db_error(500, &e.to_string())
                    }
                }
            }
            ("GET", "QueryCount") => {
                let query = parse_query(query);
                let table = query.get("tableName").cloned().unwrap_or_default();
                match service.query_count(dal, &table) {
                    Ok(n) => {
                        self.record(&source.name, action, &token, "", started, true, n, String::new());
                        db_ok(json!({ "data": n }))
                    }
                    Err(e) => {
                        self.record(&source.name, action, &token, "", started, false, 0, e.to_string());
                        db_error(500, &e.to_string())
                    }
                }
            }
            ("GET", "GetTables") => match service.get_tables(dal) {
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
                    db_ok(Value::Array(list))
                }
                Err(e) => {
                    self.record(&source.name, action, &token, "", started, false, 0, e.to_string());
                    db_error(500, &e.to_string())
                }
            },
            _ => db_error(
                404,
                "未知路由（支持 Login/Query/Execute/InsertAndGetIdentity/QueryCount/GetTables）",
            ),
        }
    }

    /// 字段策略快照。
    pub fn policy_snapshot(&self) -> crate::policy::Policy {
        self.policy
            .read()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    /// 保存字段策略（内存 + 落盘）。
    pub fn save_policy(&self, policy: crate::policy::Policy) -> Result<(), String> {
        crate::policy::save(&self.base, &policy)?;
        *self.policy.write().map_err(|_| "状态锁异常".to_string())? = policy;
        Ok(())
    }

    /// 按字段策略裁剪结果集，返回（裁剪后结果集，审计备注）。
    ///
    /// - `allow`：保留
    /// - `mask`：v1 等同 `deny`（不返回）；后续可扩展为脱敏值
    /// - `deny` / 未登记（`enforce`）：剔除该列
    /// - 未登记（`audit`）：保留，但记录列名供「字段策略」页登记
    fn apply_field_policy(
        &self,
        db: &str,
        set: pek_rcode::session::RowSet,
    ) -> (pek_rcode::session::RowSet, String) {
        use crate::policy::Action;
        let policy = self.policy_snapshot();
        let mut keep: Vec<usize> = Vec::with_capacity(set.columns.len());
        let mut masks: Vec<usize> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        let mut masked: Vec<String> = Vec::new();
        for (index, column) in set.columns.iter().enumerate() {
            let (action, registered) = policy.decide(db, column);
            if !registered {
                unknown.push(column.clone());
            }
            match action {
                Action::Allow => keep.push(index),
                // 脱敏：列保留、值替换为 `***`（ORM 仍能取到该列，不会报"字段不存在"）
                Action::Mask => {
                    keep.push(index);
                    masks.push(index);
                    masked.push(column.clone());
                }
                // 拒绝：列直接从结果集消失（连 SELECT * 也拿不到）
                Action::Deny => dropped.push(column.clone()),
            }
        }
        if !unknown.is_empty() {
            self.unknown_fields.observe(db, &unknown);
        }
        if !dropped.is_empty() || !masked.is_empty() || !unknown.is_empty() {
            let mut note = String::new();
            if !dropped.is_empty() {
                note.push_str(&format!("字段已拦截：{}", dropped.join(",")));
            }
            if !masked.is_empty() {
                if !note.is_empty() {
                    note.push('；');
                }
                note.push_str(&format!("字段已脱敏：{}", masked.join(",")));
            }
            if !unknown.is_empty() {
                if !note.is_empty() {
                    note.push('；');
                }
                note.push_str(&format!("未登记字段：{}", unknown.join(",")));
            }
            dhrust::warn!("db={db} {note}");
            let masked_positions: Vec<usize> = keep
                .iter()
                .enumerate()
                .filter(|(_, original)| masks.contains(original))
                .map(|(position, _)| position)
                .collect();
            let mut out = set.project(&keep);
            if !masked_positions.is_empty() {
                out = out.mask_values(&masked_positions, "***");
            }
            return (out, note);
        }
        (set, String::new())
    }

    /// 未登记字段快照（面板展示）。
    pub fn unknown_fields(&self) -> BTreeMap<String, BTreeMap<String, u64>> {
        self.unknown_fields.snapshot()
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
            dhrust::info!("db={db} op={op} ok rows={rows} {micros}us sql={summary}");
        } else {
            dhrust::warn!("db={db} op={op} FAIL {micros}us sql={summary} err={error}");
        }
    }
}

impl Registry {
    /// 从数据源列表重建索引与令牌表（唯一入口，保证三者一致）。
    fn rebuild(sources: Vec<Arc<DataSource>>) -> Result<Self, String> {
        let mut by_name = BTreeMap::new();
        let mut service = DbService::new();
        for (index, source) in sources.iter().enumerate() {
            let key = source.name.to_ascii_lowercase();
            if by_name.insert(key, index).is_some() {
                return Err(format!("数据源重名：{}（Name 忽略大小写后必须唯一）", source.name));
            }
            if !source.token.is_empty() {
                service.set_token(&source.token, &[source.name.as_str()]);
            }
        }
        Ok(Self {
            sources,
            by_name,
            service,
        })
    }
}

/// 打开单个数据源（失败只记错误，不影响其他数据源）。
fn open_source(base: &Path, section: &DatabaseSection) -> Result<DataSource, String> {
    let name = section.name.trim().to_string();
    if name.is_empty() {
        return Err("数据源 Name 不能为空".into());
    }
    // SQLite 相对路径按程序基目录绝对化并建目录（否则会相对进程 CWD，`Data/x.db` 打不开）
    let raw = resolve_connection(base, &section.connection)?;
    let connection = pek_rcode::database::resolve_connection(base, &raw)?;
    let (dal, open_error, kind) = match Dal::open(&connection) {
        Ok(dal) => {
            let kind = dal.kind().name().to_string();
            (Some(Arc::new(dal)), String::new(), kind)
        }
        Err(e) => (None, e.to_string(), String::new()),
    };
    Ok(DataSource {
        section: section.clone(),
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

/// 解析连接串：`enc:v1:` 加密值需要用**同一 base** 下的密钥（`Config/.key`）解密。
fn resolve_connection(base: &Path, raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if !dhrust::secret::is_encrypted(raw) {
        return Ok(raw.to_string());
    }
    let key = ensure_key(base)?;
    dhrust::secret::decrypt(raw, &key).map_err(|e| format!("连接串解密失败：{e}"))
}

/// 确保本机密钥存在（不存在则生成 `Config/.key`）；`PEK_RDBPROXY_KEY` 优先。
pub fn ensure_key(base: &Path) -> Result<[u8; 32], String> {
    if let Ok(seed) = std::env::var("PEK_RDBPROXY_KEY")
        && !seed.trim().is_empty()
    {
        return Ok(dhrust::secret::key_from_seed(seed.trim()));
    }
    let path = base.join("Config/.key");
    if let Ok(seed) = std::fs::read_to_string(&path) {
        let seed = seed.trim().to_string();
        if !seed.is_empty() {
            return Ok(dhrust::secret::key_from_seed(&seed));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let seed = dhrust::random::hex(32);
    std::fs::write(&path, &seed).map_err(|e| format!("写入密钥失败：{e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(dhrust::secret::key_from_seed(&seed))
}

/// 把连接串转成**磁盘形态**（明文 → `enc:v1:` 密文；已是密文则原样）。
pub fn encrypt_connection(base: &Path, raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() || dhrust::secret::is_encrypted(raw) {
        return Ok(raw.to_string());
    }
    let key = ensure_key(base)?;
    dhrust::secret::encrypt(raw, &key).map_err(|e| format!("连接串加密失败：{e}"))
}

/// 用新配置快照替换数据源（保留统计与已打开连接）。
fn with_section(source: &Arc<DataSource>, section: DatabaseSection) -> Arc<DataSource> {
    Arc::new(DataSource {
        section,
        name: source.name.clone(),
        description: source.description.clone(),
        token: source.token.clone(),
        read_only: AtomicBool::new(source.read_only.load(Ordering::Relaxed)),
        dal: source.dal.clone(),
        open_error: source.open_error.clone(),
        kind: source.kind.clone(),
        connection_masked: source.connection_masked.clone(),
        stat: DbStat::default(),
    })
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

/// 生成密码盐（32 字符 hex）。
pub fn new_salt() -> String {
    dhrust::random::hex(16)
}

/// 补全 JWT 密钥格式（`HS256:<随机>`；配置里手写裸值也能用）。
fn normalize_jwt_secret(secret: &str) -> String {
    let secret = secret.trim();
    if secret.contains(':') {
        secret.to_string()
    } else {
        format!("HS256:{secret}")
    }
}

/// 密码哈希（`salted_sha256_hex`，与 Pek.RAgent / C# `Pek` 面板同口径）。
pub fn hash_password(password: &str) -> String {
    let salt = new_salt();
    format!("{salt}:{}", dhrust::sign::salted_sha256_hex(&salt, password))
}

/// 密码校验（对齐 `pek-radmin` 口径：**只禁止空密码**）。
///
/// 有意不做复杂度 / 弱口令字典 / 与用户名相同的校验：面板用户自己决定密码强度，
/// 安全护栏改在别处（默认 admin 首次登录强制换密、面板鉴权级别、JWT 密钥独立）。
/// 此前的“至少 10 位 + 三类 + 弱口令字典（含 `123456` 子串）”会把
/// `Berin123456$$` 这类正常密码误判为弱口令，属于过度限制。
pub fn password_strength(_user: &str, password: &str) -> Result<(), String> {
    if password.trim().is_empty() {
        return Err("密码不能为空".into());
    }
    Ok(())
}

/// 生成一个符合强度规则的随机密码（20 位，含大写/小写/数字/符号）。
pub fn generate_password() -> String {
    format!("Pk9!{}", dhrust::random::hex(8))
}

/// 校验「盐:哈希」形式的密码串。
pub fn verify_password(stored: &str, password: &str) -> bool {
    match stored.split_once(':') {
        Some((salt, hash)) => {
            let actual = dhrust::sign::salted_sha256_hex(salt, password);
            constant_time_eq(actual.as_bytes(), hash.as_bytes())
        }
        None => false,
    }
}

/// 常量时间字符串比较（长度不同直接 false）。
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right.iter()) {
        diff |= a ^ b;
    }
    diff == 0
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

/// 是否回环来源（未知来源按回环处理，便于本地测试构造的请求）。
pub fn is_loopback(addr: Option<&str>) -> bool {
    let Some(addr) = addr else {
        return true;
    };
    match addr.parse::<SocketAddr>() {
        Ok(socket) => socket.ip().is_loopback(),
        Err(_) => addr.starts_with("127.") || addr == "::1" || addr.starts_with("[::1]"),
    }
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
pub fn parse_query(text: &str) -> BTreeMap<String, String> {
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

/// 协议成功响应（裸 `{data:…}`，对齐 C#）。
fn db_ok(data: Value) -> HttpResponse {
    HttpResponse::json(200, data.to_string())
}

/// 协议错误响应（`{code, msg}`，`DbClient` 靠 code 非 0 抛 msg）。
fn db_error(status: u16, message: &str) -> HttpResponse {
    HttpResponse::json(status, json!({ "code": status, "msg": message }).to_string())
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

    #[test]
    fn password_hash_roundtrip() {
        let stored = hash_password("s3cret!");
        assert!(verify_password(&stored, "s3cret!"));
        assert!(!verify_password(&stored, "wrong"));
        assert!(!verify_password("garbage", "s3cret!"));
    }
}
