//! 字段级安全策略（「表管理」）：按 `数据源 + 列名` 登记允许/掩码/拒绝，
//! 未登记字段按当前模式处理（`audit` 只记录、`enforce` 不返回）。
//!
//! 存储：`Config/field-policy.json`（本机文件，`.gitignore` 已排除 `Config/`）。
//!
//! 设计取舍：
//! - **粒度用「数据源 + 列名」**：SQL 里表名可能有别名/子查询/联表，可靠解析代价高且易错，
//!   列名是返回结果里天然存在、且是真正决定"这个字段要不要给出去"的东西；
//! - **默认 `audit`（学习模式）**：先把未登记字段记进审计，不打断现有接入；确认后再切 `enforce`；
//! - **元数据接口（GetTables/GetSchema）不受约束**：只约束 Query 的数据行。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 生效模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// 学习模式：全部返回，未登记字段写审计（需显式开启）
    Audit,
    /// 强制模式（**默认**）：未登记/拒绝的列不返回；掩码列保留但值替换为 `***`
    Enforce,
}

impl Mode {
    /// 解析（未知值按 `Enforce`，宁严勿松）。
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "audit" => Self::Audit,
            _ => Self::Enforce,
        }
    }

    /// 文本。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audit => "audit",
            Self::Enforce => "enforce",
        }
    }
}

/// 单列动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// 允许返回
    Allow,
    /// 脱敏：列保留，每行值替换为 `***`（调用方仍能按列名取到，不会报“字段不存在”）
    Mask,
    /// 不返回
    Deny,
}

impl Action {
    /// 解析（未知值按 `Deny`，宁严勿松）。
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "allow" => Self::Allow,
            "mask" => Self::Mask,
            _ => Self::Deny,
        }
    }

    /// 文本。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Mask => "mask",
            Self::Deny => "deny",
        }
    }
}

/// 一条字段规则。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// 数据源名（`Database=`）
    pub db: String,
    /// 列名（大小写不敏感）
    pub column: String,
    /// 动作
    pub action: Action,
    /// 备注（用途说明；强制模式下**有备注也仍需 action=allow 才放行**）
    #[serde(default)]
    pub note: String,
}

/// 策略集合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Policy {
    /// 生效模式
    #[serde(default = "default_mode")]
    pub mode: Mode,
    /// 规则
    #[serde(default)]
    pub rules: Vec<Rule>,
}

fn default_mode() -> Mode {
    // 安全默认：**未登记列不放行**（白名单）。需要观察期时显式切到 `audit`。
    Mode::Enforce
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            mode: Mode::Enforce,
            rules: Vec::new(),
        }
    }
}

/// 配置文件名。
pub const FILE_NAME: &str = "field-policy.json";

/// 策略文件路径。
pub fn path_of(base: &Path) -> PathBuf {
    base.join("Config").join(FILE_NAME)
}

/// 读取策略（不存在返回默认；损坏时返回默认并保留原文件）。
pub fn load(base: &Path) -> Policy {
    let path = path_of(base);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            dhrust::warn!("字段策略文件解析失败（已按默认处理）：{e}");
            Policy::default()
        }),
        Err(_) => Policy::default(),
    }
}

/// 保存策略（原子写）。
pub fn save(base: &Path, policy: &Policy) -> Result<PathBuf, String> {
    let path = path_of(base);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败：{e}"))?;
    }
    let text = serde_json::to_string_pretty(policy).map_err(|e| format!("序列化失败：{e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写入失败：{e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("替换失败：{e}"))?;
    Ok(path)
}

/// 归一化列名（大小写不敏感比较）。
fn key(column: &str) -> String {
    column.trim().to_ascii_lowercase()
}

impl Policy {
    /// 查某数据源某列的规则（无则 `None`）。
    pub fn rule_of(&self, db: &str, column: &str) -> Option<&Rule> {
        let col = key(column);
        self.rules
            .iter()
            .find(|r| r.db.eq_ignore_ascii_case(db) && key(&r.column) == col)
    }

    /// 列动作：有规则按规则；无规则在 `enforce` 下视为 `Deny`、`audit` 下视为 `Allow`。
    pub fn decide(&self, db: &str, column: &str) -> (Action, bool) {
        match self.rule_of(db, column) {
            Some(rule) => (rule.action, true),
            None => match self.mode {
                Mode::Enforce => (Action::Deny, false),
                Mode::Audit => (Action::Allow, false),
            },
        }
    }

    /// 新增或更新规则（同名覆盖，保留备注）。
    pub fn upsert(&mut self, rule: Rule) {
        let col = key(&rule.column);
        if let Some(slot) = self
            .rules
            .iter_mut()
            .find(|r| r.db.eq_ignore_ascii_case(&rule.db) && key(&r.column) == col)
        {
            slot.action = rule.action;
            if !rule.note.trim().is_empty() {
                slot.note = rule.note;
            }
            return;
        }
        self.rules.push(rule);
    }

    /// 删除规则（返回是否删除）。
    pub fn remove(&mut self, db: &str, column: &str) -> bool {
        let col = key(column);
        let before = self.rules.len();
        self.rules
            .retain(|r| !(r.db.eq_ignore_ascii_case(db) && key(&r.column) == col));
        self.rules.len() != before
    }

    /// 按数据源分组统计（面板展示用）。
    pub fn summary(&self) -> BTreeMap<String, usize> {
        let mut map = BTreeMap::new();
        for rule in &self.rules {
            *map.entry(rule.db.clone()).or_insert(0) += 1;
        }
        map
    }
}

/// 审计里「未登记字段」的聚合（内存环形，按 `数据源` 归并最近出现的列名）。
#[derive(Default)]
pub struct UnknownFields {
    inner: std::sync::Mutex<BTreeMap<String, BTreeMap<String, u64>>>,
}

impl UnknownFields {
    /// 记录一次观测（列名 → 出现次数）。
    pub fn observe(&self, db: &str, columns: &[String]) {
        if let Ok(mut map) = self.inner.lock() {
            let entry = map.entry(db.to_string()).or_default();
            for column in columns {
                *entry.entry(column.clone()).or_insert(0) += 1;
            }
            // 控制规模：每个数据源最多保留 200 个
            if entry.len() > 200 {
                let keep: Vec<String> = entry.keys().take(200).cloned().collect();
                entry.retain(|k, _| keep.contains(k));
            }
        }
    }

    /// 快照（数据源 → 列名 → 次数）。
    pub fn snapshot(&self) -> BTreeMap<String, BTreeMap<String, u64>> {
        self.inner
            .lock()
            .map(|m| m.clone())
            .unwrap_or_default()
    }

    /// 清空。
    pub fn clear(&self) {
        if let Ok(mut map) = self.inner.lock() {
            map.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_enforce_and_denies_unknown() {
        let policy = Policy::default();
        assert_eq!(policy.mode, Mode::Enforce, "默认应为强制：未登记列不放行");
        assert_eq!(policy.decide("demo", "name"), (Action::Deny, false));
        assert_eq!(Mode::parse("whatever"), Mode::Enforce, "未知模式按宁严处理");
        assert_eq!(Mode::parse("audit"), Mode::Audit);
    }

    #[test]
    fn audit_mode_allows_unknown_and_flags_it() {
        let policy = Policy {
            mode: Mode::Audit,
            rules: Vec::new(),
        };
        assert_eq!(policy.decide("demo", "name"), (Action::Allow, false));
    }

    #[test]
    fn enforce_mode_denies_unknown_but_keeps_registered() {
        let mut policy = Policy {
            mode: Mode::Enforce,
            rules: Vec::new(),
        };
        policy.upsert(Rule {
            db: "demo".into(),
            column: "Name".into(),
            action: Action::Allow,
            note: "设备名".into(),
        });
        assert_eq!(policy.decide("demo", "name"), (Action::Allow, true));
        assert_eq!(policy.decide("demo", "pwd"), (Action::Deny, false));
        assert_eq!(policy.decide("other", "name"), (Action::Deny, false));
    }

    #[test]
    fn upsert_updates_action_and_keeps_note() {
        let mut policy = Policy::default();
        policy.upsert(Rule {
            db: "demo".into(),
            column: "pwd".into(),
            action: Action::Deny,
            note: "密码".into(),
        });
        policy.upsert(Rule {
            db: "demo".into(),
            column: "PWD".into(),
            action: Action::Mask,
            note: String::new(),
        });
        let rule = policy.rule_of("demo", "pwd").expect("应有规则");
        assert_eq!(rule.action, Action::Mask);
        assert_eq!(rule.note, "密码");
        assert!(policy.remove("demo", "pwd"));
        assert!(!policy.remove("demo", "pwd"));
    }
}
