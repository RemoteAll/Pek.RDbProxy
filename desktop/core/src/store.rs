//! 工作区持久化：**连接（按项目分组，令牌加密落盘）/ SQL 片段 / 执行历史**。
//!
//! 落盘位置由调用方给（Tauri 侧用应用数据目录，如
//! `%APPDATA%\com.peikesmart.rdbproxytool`）：
//!
//! ```text
//! {data_dir}/workspace.json   连接 + 片段 + 历史（令牌为 enc:v1: 密文）
//! {data_dir}/.key             本机密钥种子（不随导出/仓库走）
//! ```
//!
//! 安全口径（对齐项目规范「凭据不明文落盘」）：
//! - 令牌用 `dhrust::secret`（AES-256-GCM）加密后存盘，界面只显示掩码；
//! - 导出默认**明文令牌**（换机才能用），由界面明确提示；不导出本机密钥。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::client::Target;

/// 工作区文件名
const FILE_NAME: &str = "workspace.json";
/// 本机密钥文件名
const KEY_NAME: &str = ".key";
/// 当前数据版本
const VERSION: u32 = 1;
/// 执行历史上限
pub const HISTORY_LIMIT: usize = 200;

/* ── 数据模型（落盘） ───────────────────────────── */

/// 工作区（落盘结构）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// 数据版本（后续迁移用）
    #[serde(default = "default_version")]
    pub version: u32,
    /// **统一默认代理地址**（登录 / 检查更新 / 新建连接缺省都用它；空 = 没设过）
    #[serde(default)]
    pub default_server: String,
    /// 保存的连接
    #[serde(default)]
    pub connections: Vec<SavedConnection>,
    /// 保存的 SQL 片段
    #[serde(default)]
    pub snippets: Vec<SqlSnippet>,
    /// 执行历史（新→旧）
    #[serde(default)]
    pub history: Vec<SqlHistory>,
}

fn default_version() -> u32 {
    VERSION
}

impl Default for Workspace {
    fn default() -> Self {
        Self {
            version: VERSION,
            default_server: String::new(),
            connections: Vec::new(),
            snippets: Vec::new(),
            history: Vec::new(),
        }
    }
}

/// 一条保存的连接（令牌为密文）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConnection {
    /// 唯一 id
    pub id: String,
    /// 名称（如「HlktechIoT 开发库」）
    pub name: String,
    /// 项目分组（空 = 未分组）
    #[serde(default)]
    pub project: String,
    /// 代理地址
    pub server: String,
    /// 数据源名
    pub db: String,
    /// 令牌密文（`enc:v1:…`；空 = 未设置）
    #[serde(default)]
    pub token_sealed: String,
    /// 备注
    #[serde(default)]
    pub note: String,
    /// 创建时间（毫秒时间戳）
    #[serde(default)]
    pub created_ms: u64,
    /// 最近使用时间
    #[serde(default)]
    pub last_used_ms: u64,
    /// 最近一次连接结果（`None` = 还没连过）
    #[serde(default)]
    pub last_ok: Option<bool>,
    /// 最近一次连接尝试时间
    #[serde(default)]
    pub last_try_ms: u64,
}

/// 一条保存的 SQL 片段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlSnippet {
    /// 唯一 id
    pub id: String,
    /// 名称（必填，便于复用）
    pub name: String,
    /// SQL 文本
    pub sql: String,
    /// 备注（可选）
    #[serde(default)]
    pub note: String,
    /// 归属项目（空 = 通用）
    #[serde(default)]
    pub project: String,
    /// 绑定连接（空 = 通用，任何连接都能用）
    #[serde(default)]
    pub connection_id: String,
    /// 创建时间
    #[serde(default)]
    pub created_ms: u64,
    /// 使用次数
    #[serde(default)]
    pub used_count: u64,
    /// 最近使用时间
    #[serde(default)]
    pub last_used_ms: u64,
}

/// 一条执行历史。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqlHistory {
    /// 唯一 id
    pub id: String,
    /// SQL 文本
    pub sql: String,
    /// 执行时用的连接 id
    #[serde(default)]
    pub connection_id: String,
    /// 执行时用的连接名（连接被删也留痕）
    #[serde(default)]
    pub connection_name: String,
    /// 时间
    pub at_ms: u64,
    /// 是否成功
    pub ok: bool,
    /// 影响/返回行数（-1 = 未知）
    #[serde(default)]
    pub rows: i64,
    /// 耗时（毫秒）
    #[serde(default)]
    pub micros: u64,
}

/* ── 视图（给前端，令牌已掩码） ─────────────────── */

/// 连接视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionView {
    /// id
    pub id: String,
    /// 名称
    pub name: String,
    /// 项目
    pub project: String,
    /// 代理地址
    pub server: String,
    /// 数据源名
    pub db: String,
    /// 令牌是否已设置
    pub token_set: bool,
    /// 令牌掩码（如 `tk-d…e-me`）
    pub token_masked: String,
    /// 备注
    pub note: String,
    /// 展示用连接串（令牌已掩码）
    pub connection: String,
    /// 创建时间
    pub created_ms: u64,
    /// 最近使用
    pub last_used_ms: u64,
    /// 最近一次连接结果
    pub last_ok: Option<bool>,
    /// 最近一次尝试时间
    pub last_try_ms: u64,
}

/// 片段视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetView {
    /// id
    pub id: String,
    /// 名称
    pub name: String,
    /// SQL
    pub sql: String,
    /// 备注
    pub note: String,
    /// 项目
    pub project: String,
    /// 绑定连接 id
    pub connection_id: String,
    /// 创建时间
    pub created_ms: u64,
    /// 使用次数
    pub used_count: u64,
    /// 最近使用
    pub last_used_ms: u64,
}

/// 历史视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryView {
    /// id
    pub id: String,
    /// SQL
    pub sql: String,
    /// 连接 id
    pub connection_id: String,
    /// 连接名
    pub connection_name: String,
    /// 时间
    pub at_ms: u64,
    /// 是否成功
    pub ok: bool,
    /// 行数
    pub rows: i64,
    /// 耗时（毫秒）
    pub micros: u64,
}

/// 整个工作区视图。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceView {
    /// 连接（按项目、名称排序）
    pub connections: Vec<ConnectionView>,
    /// 片段（按项目、名称排序）
    pub snippets: Vec<SnippetView>,
    /// 历史（新→旧）
    pub history: Vec<HistoryView>,
    /// **统一默认代理地址**（空 = 没设过，界面用 127.0.0.1:5599 兜底）
    pub default_server: String,
    /// 历史上限
    pub history_limit: usize,
    /// 数据目录（界面「关于」展示）
    pub data_dir: String,
}

/* ── 输入 ───────────────────────────────────────── */

/// 保存连接的入参。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInput {
    /// `None` = 新建
    #[serde(default)]
    pub id: Option<String>,
    /// 名称
    pub name: String,
    /// 项目
    #[serde(default)]
    pub project: String,
    /// 代理地址
    pub server: String,
    /// 数据源名
    pub db: String,
    /// 令牌（**留空 = 保留原值**）
    #[serde(default)]
    pub token: String,
    /// 备注
    #[serde(default)]
    pub note: String,
}

/// 保存片段的入参。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetInput {
    /// `None` = 新建
    #[serde(default)]
    pub id: Option<String>,
    /// 名称
    pub name: String,
    /// SQL
    pub sql: String,
    /// 备注
    #[serde(default)]
    pub note: String,
    /// 项目
    #[serde(default)]
    pub project: String,
    /// 绑定连接 id
    #[serde(default)]
    pub connection_id: String,
}

/* ── 存储 ───────────────────────────────────────── */

/// 工作区存储（**线程不安全**，调用方自行加锁）。
pub struct Store {
    dir: PathBuf,
    key: [u8; 32],
    data: Workspace,
}

impl Store {
    /// 打开（目录不存在则创建；文件损坏则备份为 `.bad-<时间>` 后重建）。
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("创建工作区目录失败（{}）：{e}", dir.display()))?;
        let key = ensure_key(&dir)?;
        let data = load_workspace(&dir)?;
        Ok(Self { dir, key, data })
    }

    /// 数据目录。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 工作区视图。
    pub fn snapshot(&self) -> WorkspaceView {
        let mut connections: Vec<ConnectionView> =
            self.data.connections.iter().map(connection_view).collect();
        connections.sort_by(|a, b| {
            a.project
                .to_lowercase()
                .cmp(&b.project.to_lowercase())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        let mut snippets: Vec<SnippetView> = self.data.snippets.iter().map(snippet_view).collect();
        snippets.sort_by(|a, b| {
            a.project
                .to_lowercase()
                .cmp(&b.project.to_lowercase())
                .then_with(|| b.last_used_ms.cmp(&a.last_used_ms))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        WorkspaceView {
            connections,
            snippets,
            history: self.data.history.iter().map(history_view).collect(),
            default_server: self.data.default_server.clone(),
            history_limit: HISTORY_LIMIT,
            data_dir: self.dir.display().to_string(),
        }
    }

    /// 设置**统一默认代理地址**（登录 / 检查更新 / 新建连接都拿它当缺省）。
    ///
    /// 传空 = 清空（界面回到 `127.0.0.1:5599` 兜底）；缺协议前缀会自动补 `http://`。
    pub fn set_default_server(&mut self, server: &str) -> Result<(), String> {
        let text = server.trim();
        let next = if text.is_empty() {
            String::new()
        } else {
            crate::client::normalize_server(text)?
        };
        if next == self.data.default_server {
            return Ok(());
        }
        self.data.default_server = next;
        self.persist()
    }

    /// 取连接解出的可用 `Target`（令牌解密）。
    pub fn connection_target(&self, id: &str) -> Result<Target, String> {
        let connection = self
            .find_connection(id)?
            .ok_or_else(|| format!("连接不存在：{id}"))?;
        let token = if connection.token_sealed.trim().is_empty() {
            String::new()
        } else {
            dhrust::secret::decrypt(&connection.token_sealed, &self.key)
                .map_err(|e| format!("令牌解密失败（本机密钥不匹配？）：{e}"))?
        };
        Target::parse(&format!(
            "Server={};Database={};Password={};provider=network",
            connection.server, connection.db, token
        ))
    }

    /// 新建/更新连接。
    pub fn save_connection(&mut self, input: ConnectionInput) -> Result<(), String> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err("连接名称不能为空".to_string());
        }
        let server = input.server.trim().trim_end_matches('/');
        if server.is_empty() {
            return Err("代理地址不能为空（例：http://127.0.0.1:5599）".to_string());
        }
        let db = input.db.trim();
        if db.is_empty() {
            return Err("数据源名不能为空（填面板里的数据源名）".to_string());
        }
        let token = input.token.trim();
        let project = input.project.trim().to_string();
        let note = input.note.trim().to_string();
        let now = now_ms();

        match input.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => {
                let index = self
                    .data
                    .connections
                    .iter()
                    .position(|c| c.id == id)
                    .ok_or_else(|| format!("连接不存在：{id}"))?;
                let sealed = if token.is_empty() {
                    // 留空 = 保留原令牌
                    self.data.connections[index].token_sealed.clone()
                } else {
                    seal(token, &self.key)?
                };
                let connection = &mut self.data.connections[index];
                connection.name = name.to_string();
                connection.project = project;
                connection.server = server.to_string();
                connection.db = db.to_string();
                connection.token_sealed = sealed;
                connection.note = note;
            }
            None => {
                if token.is_empty() {
                    return Err("新建连接必须填写令牌".to_string());
                }
                self.data.connections.push(SavedConnection {
                    id: new_id(),
                    name: name.to_string(),
                    project,
                    server: server.to_string(),
                    db: db.to_string(),
                    token_sealed: seal(token, &self.key)?,
                    note,
                    created_ms: now,
                    last_used_ms: 0,
                    last_ok: None,
                    last_try_ms: 0,
                });
            }
        }
        self.persist()
    }

    /// 删除连接（同时解绑引用它的片段、保留历史留痕）。
    pub fn delete_connection(&mut self, id: &str) -> Result<(), String> {
        let before = self.data.connections.len();
        self.data.connections.retain(|c| c.id != id);
        if self.data.connections.len() == before {
            return Err(format!("连接不存在：{id}"));
        }
        for snippet in &mut self.data.snippets {
            if snippet.connection_id == id {
                snippet.connection_id.clear();
            }
        }
        self.persist()
    }

    /// 记录一次连接尝试（健康状态 + 最近使用时间）。
    pub fn touch_connection(&mut self, id: &str, ok: bool) -> Result<(), String> {
        let Some(connection) = self
            .data
            .connections
            .iter_mut()
            .find(|c| c.id == id)
        else {
            return Ok(()); // 连接可能刚好被删，不当作错误
        };
        let now = now_ms();
        connection.last_try_ms = now;
        connection.last_ok = Some(ok);
        if ok {
            connection.last_used_ms = now;
        }
        self.persist()
    }

    /// 新建/更新片段。
    pub fn save_snippet(&mut self, input: SnippetInput) -> Result<(), String> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err("片段名称不能为空（方便以后找得到）".to_string());
        }
        let sql = input.sql.trim();
        if sql.is_empty() {
            return Err("SQL 不能为空".to_string());
        }
        let note = input.note.trim().to_string();
        let project = input.project.trim().to_string();
        let connection_id = input.connection_id.trim().to_string();
        let now = now_ms();
        match input.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(id) => {
                let snippet = self
                    .data
                    .snippets
                    .iter_mut()
                    .find(|s| s.id == id)
                    .ok_or_else(|| format!("片段不存在：{id}"))?;
                snippet.name = name.to_string();
                snippet.sql = sql.to_string();
                snippet.note = note;
                snippet.project = project;
                snippet.connection_id = connection_id;
            }
            None => self.data.snippets.push(SqlSnippet {
                id: new_id(),
                name: name.to_string(),
                sql: sql.to_string(),
                note,
                project,
                connection_id,
                created_ms: now,
                used_count: 0,
                last_used_ms: 0,
            }),
        }
        self.persist()
    }

    /// 删除片段。
    pub fn delete_snippet(&mut self, id: &str) -> Result<(), String> {
        let before = self.data.snippets.len();
        self.data.snippets.retain(|s| s.id != id);
        if self.data.snippets.len() == before {
            return Err(format!("片段不存在：{id}"));
        }
        self.persist()
    }

    /// 记录片段被使用一次（用于排序）。
    pub fn touch_snippet(&mut self, id: &str) -> Result<(), String> {
        if let Some(snippet) = self.data.snippets.iter_mut().find(|s| s.id == id) {
            snippet.used_count = snippet.used_count.saturating_add(1);
            snippet.last_used_ms = now_ms();
            self.persist()?;
        }
        Ok(())
    }

    /// 追加一条执行历史（同连接同 SQL 去重，只留最新；超出上限丢弃最旧）。
    pub fn push_history(
        &mut self,
        sql: &str,
        connection_id: &str,
        ok: bool,
        rows: i64,
        micros: u64,
    ) -> Result<(), String> {
        let sql = sql.trim();
        if sql.is_empty() {
            return Ok(());
        }
        let connection_name = self
            .data
            .connections
            .iter()
            .find(|c| c.id == connection_id)
            .map(|c| c.name.clone())
            .unwrap_or_default();
        // 去重：同一连接 + 同一 SQL 只保留最新一条
        self.data
            .history
            .retain(|h| !(h.connection_id == connection_id && h.sql == sql));
        self.data.history.insert(
            0,
            SqlHistory {
                id: new_id(),
                sql: sql.to_string(),
                connection_id: connection_id.to_string(),
                connection_name,
                at_ms: now_ms(),
                ok,
                rows,
                micros,
            },
        );
        self.data.history.truncate(HISTORY_LIMIT);
        self.persist()
    }

    /// 删除单条历史。
    pub fn delete_history(&mut self, id: &str) -> Result<(), String> {
        self.data.history.retain(|h| h.id != id);
        self.persist()
    }

    /// 清空历史。
    pub fn clear_history(&mut self) -> Result<(), String> {
        self.data.history.clear();
        self.persist()
    }

    /// 导出为 JSON（**令牌为明文**，换机导入后才能直接可用）。
    pub fn export(&self) -> Result<String, String> {
        #[derive(Serialize)]
        struct ExportConnection<'a> {
            name: &'a str,
            project: &'a str,
            server: &'a str,
            db: &'a str,
            token: String,
            note: &'a str,
        }
        let mut connections = Vec::with_capacity(self.data.connections.len());
        for connection in &self.data.connections {
            let token = if connection.token_sealed.trim().is_empty() {
                String::new()
            } else {
                dhrust::secret::decrypt(&connection.token_sealed, &self.key)
                    .map_err(|e| format!("令牌解密失败（{}）：{e}", connection.name))?
            };
            connections.push(ExportConnection {
                name: &connection.name,
                project: &connection.project,
                server: &connection.server,
                db: &connection.db,
                token,
                note: &connection.note,
            });
        }
        let payload = serde_json::json!({
            "kind": "pek-rdbproxy-tool-workspace",
            "version": VERSION,
            "exportedAtMs": now_ms(),
            "note": "含明文令牌，请妥善保管",
            "defaultServer": self.data.default_server,
            "connections": connections,
            "snippets": self.data.snippets,
        });
        serde_json::to_string_pretty(&payload).map_err(|e| format!("导出序列化失败：{e}"))
    }

    /// 导入（按「名称 + 数据源」合并连接；按 id 合并片段；返回导入条数）。
    pub fn import(&mut self, json: &str) -> Result<(usize, usize), String> {
        #[derive(Deserialize)]
        struct ImportConnection {
            name: String,
            #[serde(default)]
            project: String,
            server: String,
            db: String,
            #[serde(default)]
            token: String,
            #[serde(default)]
            note: String,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ImportFile {
            #[serde(default)]
            default_server: String,
            #[serde(default)]
            connections: Vec<ImportConnection>,
            #[serde(default)]
            snippets: Vec<SqlSnippet>,
        }
        let file: ImportFile =
            serde_json::from_str(json).map_err(|e| format!("导入文件格式不对：{e}"))?;
        // 本机没设过默认代理地址时才采纳导入的（不覆盖使用者已选定的地址）
        if self.data.default_server.trim().is_empty() {
            let incoming = file.default_server.trim();
            if !incoming.is_empty() {
                self.data.default_server = crate::client::normalize_server(incoming)?;
            }
        }
        let mut imported_connections = 0usize;
        for item in file.connections {
            let existing = self.data.connections.iter().position(|c| {
                c.name.eq_ignore_ascii_case(&item.name) && c.db.eq_ignore_ascii_case(&item.db)
            });
            let sealed = if item.token.trim().is_empty() {
                String::new()
            } else {
                seal(item.token.trim(), &self.key)?
            };
            match existing {
                Some(index) => {
                    let connection = &mut self.data.connections[index];
                    connection.server = item.server.trim().trim_end_matches('/').to_string();
                    connection.project = item.project.trim().to_string();
                    connection.note = item.note.trim().to_string();
                    if !sealed.is_empty() {
                        connection.token_sealed = sealed;
                    }
                }
                None => self.data.connections.push(SavedConnection {
                    id: new_id(),
                    name: item.name.trim().to_string(),
                    project: item.project.trim().to_string(),
                    server: item.server.trim().trim_end_matches('/').to_string(),
                    db: item.db.trim().to_string(),
                    token_sealed: sealed,
                    note: item.note.trim().to_string(),
                    created_ms: now_ms(),
                    last_used_ms: 0,
                    last_ok: None,
                    last_try_ms: 0,
                }),
            }
            imported_connections += 1;
        }
        let mut imported_snippets = 0usize;
        for mut snippet in file.snippets {
            if snippet.id.trim().is_empty() {
                snippet.id = new_id();
            }
            // 片段 id 冲突时换新 id，避免覆盖本地同名
            while self.data.snippets.iter().any(|s| s.id == snippet.id) {
                snippet.id = new_id();
            }
            snippet.connection_id.clear(); // 跨机导入时连接 id 无意义
            self.data.snippets.push(snippet);
            imported_snippets += 1;
        }
        self.persist()?;
        Ok((imported_connections, imported_snippets))
    }

    fn find_connection(&self, id: &str) -> Result<Option<&SavedConnection>, String> {
        Ok(self.data.connections.iter().find(|c| c.id == id))
    }

    /// 写盘。
    fn persist(&self) -> Result<(), String> {
        let path = self.dir.join(FILE_NAME);
        let text = serde_json::to_string_pretty(&self.data)
            .map_err(|e| format!("工作区序列化失败：{e}"))?;
        std::fs::write(&path, text)
            .map_err(|e| format!("写入工作区失败（{}）：{e}", path.display()))
    }
}

/* ── 辅助 ───────────────────────────────────────── */

fn connection_view(connection: &SavedConnection) -> ConnectionView {
    let token_set = !connection.token_sealed.trim().is_empty();
    ConnectionView {
        id: connection.id.clone(),
        name: connection.name.clone(),
        project: connection.project.clone(),
        server: connection.server.clone(),
        db: connection.db.clone(),
        token_set,
        token_masked: if token_set {
            "••••••".to_string()
        } else {
            "（未设置）".to_string()
        },
        note: connection.note.clone(),
        connection: format!(
            "Server={};Database={};Password={};provider=network",
            connection.server,
            connection.db,
            if token_set { "<令牌>" } else { "" }
        ),
        created_ms: connection.created_ms,
        last_used_ms: connection.last_used_ms,
        last_ok: connection.last_ok,
        last_try_ms: connection.last_try_ms,
    }
}

fn snippet_view(snippet: &SqlSnippet) -> SnippetView {
    SnippetView {
        id: snippet.id.clone(),
        name: snippet.name.clone(),
        sql: snippet.sql.clone(),
        note: snippet.note.clone(),
        project: snippet.project.clone(),
        connection_id: snippet.connection_id.clone(),
        created_ms: snippet.created_ms,
        used_count: snippet.used_count,
        last_used_ms: snippet.last_used_ms,
    }
}

fn history_view(history: &SqlHistory) -> HistoryView {
    HistoryView {
        id: history.id.clone(),
        sql: history.sql.clone(),
        connection_id: history.connection_id.clone(),
        connection_name: history.connection_name.clone(),
        at_ms: history.at_ms,
        ok: history.ok,
        rows: history.rows,
        micros: history.micros,
    }
}

/// 确保本机密钥存在（不存在则生成）。
fn ensure_key(dir: &Path) -> Result<[u8; 32], String> {
    let path = dir.join(KEY_NAME);
    if let Ok(seed) = std::fs::read_to_string(&path) {
        let seed = seed.trim();
        if !seed.is_empty() {
            return Ok(dhrust::secret::key_from_seed(seed));
        }
    }
    let seed = dhrust::random::hex(32);
    std::fs::write(&path, &seed)
        .map_err(|e| format!("写入本机密钥失败（{}）：{e}", path.display()))?;
    Ok(dhrust::secret::key_from_seed(&seed))
}

/// 读取工作区（损坏时备份后重建，不让用户卡在启动）。
fn load_workspace(dir: &Path) -> Result<Workspace, String> {
    let path = dir.join(FILE_NAME);
    if !path.is_file() {
        return Ok(Workspace::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取工作区失败（{}）：{e}", path.display()))?;
    match serde_json::from_str::<Workspace>(&text) {
        Ok(workspace) => Ok(workspace),
        Err(e) => {
            let backup = dir.join(format!("{FILE_NAME}.bad-{}", now_ms()));
            let _ = std::fs::rename(&path, &backup);
            Err(format!(
                "工作区文件损坏（{e}）；已备份为 {}，本次以空工作区启动",
                backup.display()
            ))
        }
    }
}

/// 加密令牌（空值不解密/不加密）。
fn seal(token: &str, key: &[u8; 32]) -> Result<String, String> {
    dhrust::secret::encrypt(token, key).map_err(|e| format!("令牌加密失败：{e}"))
}

/// 当前毫秒时间戳。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 生成短 id。
fn new_id() -> String {
    dhrust::random::hex(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rdbproxy-tool-store-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn input(name: &str, token: &str) -> ConnectionInput {
        ConnectionInput {
            id: None,
            name: name.to_string(),
            project: "Demo".to_string(),
            server: "http://127.0.0.1:5599".to_string(),
            db: "demo".to_string(),
            token: token.to_string(),
            note: String::new(),
        }
    }

    #[test]
    fn connection_is_encrypted_on_disk_and_usable_after_reopen() {
        let dir = temp_dir("conn");
        let mut store = Store::open(&dir).unwrap();
        store.save_connection(input("开发库", "tk-secret-123456")).unwrap();

        let raw = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(!raw.contains("tk-secret-123456"), "令牌不能明文落盘：{raw}");
        assert!(raw.contains("enc:v1:"), "应为加密形态：{raw}");

        // 重开：能解出令牌
        let reopened = Store::open(&dir).unwrap();
        let id = reopened.snapshot().connections[0].id.clone();
        let target = reopened.connection_target(&id).unwrap();
        assert_eq!(target.token, "tk-secret-123456");
        assert_eq!(target.db, "demo");
        assert_eq!(target.server, "http://127.0.0.1:5599");

        // 视图里不应出现明文
        let view = reopened.snapshot();
        let text = serde_json::to_string(&view).unwrap();
        assert!(!text.contains("tk-secret-123456"));
        assert!(view.connections[0].token_set);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn editing_with_empty_token_keeps_original() {
        let dir = temp_dir("edit");
        let mut store = Store::open(&dir).unwrap();
        store.save_connection(input("开发库", "tk-1")).unwrap();
        let id = store.snapshot().connections[0].id.clone();

        let mut edit = input("开发库-改名", "");
        edit.id = Some(id.clone());
        edit.project = "Other".into();
        store.save_connection(edit).unwrap();

        let target = store.connection_target(&id).unwrap();
        assert_eq!(target.token, "tk-1", "留空应保留原令牌");
        let view = store.snapshot();
        assert_eq!(view.connections[0].name, "开发库-改名");
        assert_eq!(view.connections[0].project, "Other");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn new_connection_requires_token_and_name() {
        let dir = temp_dir("require");
        let mut store = Store::open(&dir).unwrap();
        assert!(store.save_connection(input("x", "")).is_err());
        assert!(store.save_connection(input("", "tk")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_connection_unbinds_snippets_but_keeps_history() {
        let dir = temp_dir("delconn");
        let mut store = Store::open(&dir).unwrap();
        store.save_connection(input("开发库", "tk")).unwrap();
        let id = store.snapshot().connections[0].id.clone();
        store
            .save_snippet(SnippetInput {
                id: None,
                name: "取用户".into(),
                sql: "SELECT 1".into(),
                note: String::new(),
                project: "Demo".into(),
                connection_id: id.clone(),
            })
            .unwrap();
        store.push_history("SELECT 1", &id, true, -1, 3).unwrap();

        store.delete_connection(&id).unwrap();
        let view = store.snapshot();
        assert!(view.connections.is_empty());
        assert_eq!(view.snippets[0].connection_id, "", "片段应解绑");
        assert_eq!(view.history.len(), 1, "历史要留痕");
        assert_eq!(view.history[0].connection_name, "开发库");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn history_dedupes_and_respects_limit() {
        let dir = temp_dir("hist");
        let mut store = Store::open(&dir).unwrap();
        store.save_connection(input("开发库", "tk")).unwrap();
        let id = store.snapshot().connections[0].id.clone();

        store.push_history("SELECT 1", &id, true, -1, 1).unwrap();
        store.push_history("SELECT 1", &id, true, -1, 2).unwrap();
        let view = store.snapshot();
        assert_eq!(view.history.len(), 1, "同 SQL 应去重");
        assert_eq!(view.history[0].micros, 2, "保留最新一条");

        for i in 0..(HISTORY_LIMIT + 20) {
            store
                .push_history(&format!("SELECT {i}"), &id, true, -1, 1)
                .unwrap();
        }
        assert_eq!(store.snapshot().history.len(), HISTORY_LIMIT);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_has_plaintext_token_and_import_roundtrips() {
        let dir = temp_dir("export");
        let mut store = Store::open(&dir).unwrap();
        store.save_connection(input("开发库", "tk-move")).unwrap();
        let exported = store.export().unwrap();
        assert!(exported.contains("tk-move"), "导出应含明文令牌（换机可用）");
        assert!(!exported.contains("enc:v1:"), "导出不该带密文");

        let other_dir = temp_dir("import");
        let mut other = Store::open(&other_dir).unwrap();
        let (c, s) = other.import(&exported).unwrap();
        assert_eq!((c, s), (1, 0));
        let id = other.snapshot().connections[0].id.clone();
        assert_eq!(other.connection_target(&id).unwrap().token, "tk-move");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&other_dir).ok();
    }

    #[test]
    fn default_server_is_normalized_persisted_and_cleared() {
        let dir = temp_dir("default-server");
        let mut store = Store::open(&dir).unwrap();
        assert_eq!(store.snapshot().default_server, "", "初始为空");

        // 缺协议 + 末尾斜杠 → 归一化
        store.set_default_server("  10.0.0.5:5599/  ").unwrap();
        assert_eq!(store.snapshot().default_server, "http://10.0.0.5:5599");

        // 已落盘（重开还在）
        let mut reopened = Store::open(&dir).unwrap();
        assert_eq!(reopened.snapshot().default_server, "http://10.0.0.5:5599");

        // 清空
        reopened.set_default_server("").unwrap();
        assert_eq!(reopened.snapshot().default_server, "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn old_workspace_without_default_server_field_still_loads() {
        // 旧版本 workspace.json 没有 defaultServer 字段：不能因为新字段读不出来就丢工作区
        let dir = temp_dir("old-ws");
        std::fs::write(
            dir.join(FILE_NAME),
            r#"{"version":1,"connections":[],"snippets":[],"history":[]}"#,
        )
        .unwrap();
        let store = Store::open(&dir).unwrap();
        let view = store.snapshot();
        assert_eq!(view.default_server, "");
        assert!(view.connections.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_carries_default_server_and_import_only_fills_when_empty() {
        let dir = temp_dir("ds-export");
        let mut store = Store::open(&dir).unwrap();
        store.set_default_server("http://10.0.0.5:5599").unwrap();
        let exported = store.export().unwrap();
        assert!(exported.contains("defaultServer"), "导出应带默认代理地址");
        assert!(exported.contains("10.0.0.5:5599"));

        // 本机没设过 → 采纳导入的
        let blank_dir = temp_dir("ds-import-blank");
        let mut blank = Store::open(&blank_dir).unwrap();
        blank.import(&exported).unwrap();
        assert_eq!(blank.snapshot().default_server, "http://10.0.0.5:5599");

        // 本机已设过 → 不被导入覆盖
        let set_dir = temp_dir("ds-import-set");
        let mut existing = Store::open(&set_dir).unwrap();
        existing.set_default_server("http://127.0.0.1:5599").unwrap();
        existing.import(&exported).unwrap();
        assert_eq!(existing.snapshot().default_server, "http://127.0.0.1:5599");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&blank_dir).ok();
        std::fs::remove_dir_all(&set_dir).ok();
    }

    #[test]
    fn snippet_save_touch_and_delete() {
        let dir = temp_dir("snip");
        let mut store = Store::open(&dir).unwrap();
        store
            .save_snippet(SnippetInput {
                id: None,
                name: "取表结构".into(),
                sql: "SELECT name FROM sqlite_master".into(),
                note: "自检用".into(),
                project: String::new(),
                connection_id: String::new(),
            })
            .unwrap();
        let id = store.snapshot().snippets[0].id.clone();
        store.touch_snippet(&id).unwrap();
        assert_eq!(store.snapshot().snippets[0].used_count, 1);
        store.delete_snippet(&id).unwrap();
        assert!(store.snapshot().snippets.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
