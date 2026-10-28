//! 协议自检：把 e2e 断言做成「人一眼能看懂」的清单。
//!
//! 顺序按「能否用起来」排列：
//! 1. `Login` —— 令牌 + 数据源名对不对（拿到 `DbType`/`Version` 才算通）；
//! 2. `GetTables` —— 读链路真实往返（不依赖具体表）；
//! 3. `Query` —— 用户给的 SQL（默认 `SELECT 1`）；
//! 4. `QueryCount` —— 可选，需表名；
//! 5. `Execute` —— 可选，需写语句（只读数据源应在此被拒，这本身就是验证点）；
//! 6. 错误信封 —— 用错令牌确认失败响应带可读消息（默认关闭：会在服务端记一条失败审计）。

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::client::{self, Target};

/// 单项结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// 通过
    Pass,
    /// 失败
    Fail,
    /// 跳过（未配置或未启用）
    Skip,
}

/// 单个自检项。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    /// 项目名（界面直接显示）
    pub name: String,
    /// 结果
    pub status: CheckStatus,
    /// 说明（成功给关键信息，失败给完整原因）
    pub detail: String,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
}

impl CheckItem {
    fn new(name: &str, status: CheckStatus, detail: String, started: Instant) -> Self {
        Self {
            name: name.to_string(),
            status,
            detail,
            elapsed_ms: started.elapsed().as_millis() as u64,
        }
    }

    fn pass(name: &str, detail: String, started: Instant) -> Self {
        Self::new(name, CheckStatus::Pass, detail, started)
    }

    fn fail(name: &str, detail: String, started: Instant) -> Self {
        Self::new(name, CheckStatus::Fail, detail, started)
    }

    fn skip(name: &str, detail: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Skip,
            detail: detail.to_string(),
            elapsed_ms: 0,
        }
    }
}

/// 自检参数（均可留空 = 用默认/跳过）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SelfCheckOptions {
    /// 查询自检用的 SQL（默认 `SELECT 1`；Oracle 可写 `SELECT 1 FROM DUAL`）
    pub sql: String,
    /// 计数自检的表名（空 = 跳过）
    pub table: String,
    /// 写操作自检 SQL（空 = 跳过；只读数据源应被拒绝）
    pub write_sql: String,
    /// 用错令牌验证错误信封（会产生一条失败审计）
    pub check_wrong_token: bool,
}

impl Default for SelfCheckOptions {
    fn default() -> Self {
        Self {
            sql: "SELECT 1".to_string(),
            table: String::new(),
            write_sql: String::new(),
            check_wrong_token: false,
        }
    }
}

/// 自检报告。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelfCheckReport {
    /// 目标摘要（令牌已脱敏）
    pub target: String,
    /// 明细
    pub items: Vec<CheckItem>,
    /// 通过数
    pub passed: usize,
    /// 失败数
    pub failed: usize,
    /// 跳过数
    pub skipped: usize,
    /// 总耗时（毫秒）
    pub total_ms: u64,
}

impl SelfCheckReport {
    /// 是否全部通过（跳过不计失败）。
    pub fn ok(&self) -> bool {
        self.failed == 0
    }
}

/// 跑一遍自检。
pub fn selfcheck(target: &Target, options: &SelfCheckOptions) -> SelfCheckReport {
    let started = Instant::now();
    let mut items = Vec::new();

    // ① Login
    let clock = Instant::now();
    match client::login(target) {
        Ok(view) => {
            let detail = if view.db_type.trim().is_empty() {
                "已连通（服务端未返回库类型）".to_string()
            } else if view.version.trim().is_empty() {
                view.db_type.clone()
            } else {
                format!("{} · {}", view.db_type, view.version)
            };
            items.push(CheckItem::pass("连接 / Login", detail, clock));
        }
        Err(e) => items.push(CheckItem::fail(
            "连接 / Login",
            format!("{e}（先确认：代理在运行、数据源名对、令牌未过期）"),
            clock,
        )),
    }

    // ② GetTables（读链路）
    let clock = Instant::now();
    match client::tables(target) {
        Ok(view) => items.push(CheckItem::pass(
            "读链路 / GetTables",
            format!("{} 张表", view.count),
            clock,
        )),
        Err(e) => items.push(CheckItem::fail("读链路 / GetTables", e, clock)),
    }

    // ③ Query
    if options.sql.trim().is_empty() {
        items.push(CheckItem::skip("查询 / Query", "未填写查询 SQL"));
    } else {
        let clock = Instant::now();
        match client::query(target, &options.sql) {
            Ok(view) => {
                let detail = if view.columns.is_empty() && view.row_count > 0 {
                    format!(
                        "{} 行 · 0 列（列被字段策略拦截，见面板「🛡 字段安全」）",
                        view.row_count
                    )
                } else {
                    format!("{} 行 · {} 列", view.row_count, view.columns.len())
                };
                items.push(CheckItem::pass(
                    &format!("查询 / Query（{}）", short(&options.sql, 40)),
                    detail,
                    clock,
                ));
            }
            Err(e) => items.push(CheckItem::fail(
                &format!("查询 / Query（{}）", short(&options.sql, 40)),
                format!("{e}（若为方言差异，换成该库可用的等价查询）"),
                clock,
            )),
        }
    }

    // ④ QueryCount
    if options.table.trim().is_empty() {
        items.push(CheckItem::skip("计数 / QueryCount", "未填写表名"));
    } else {
        let clock = Instant::now();
        match client::count(target, &options.table) {
            Ok(view) => items.push(CheckItem::pass(
                &format!("计数 / QueryCount（{}）", view.table),
                format!("{} 行", view.count),
                clock,
            )),
            Err(e) => items.push(CheckItem::fail(
                &format!("计数 / QueryCount（{}）", options.table),
                e,
                clock,
            )),
        }
    }

    // ⑤ Execute（写）
    if options.write_sql.trim().is_empty() {
        items.push(CheckItem::skip("写操作 / Execute", "未填写写语句"));
    } else {
        let clock = Instant::now();
        let name = format!("写操作 / Execute（{}）", short(&options.write_sql, 40));
        match client::execute(target, &options.write_sql) {
            Ok(view) => items.push(CheckItem::pass(&name, format!("影响 {} 行", view.affected), clock)),
            // 只读数据源拒绝写操作是**预期**行为（护栏生效），记通过并说明
            Err(e) if e.contains("只读") => items.push(CheckItem::pass(
                &name,
                format!("已按只读策略拒绝（预期）：{}", short(&e, 120)),
                clock,
            )),
            Err(e) => items.push(CheckItem::fail(
                &name,
                format!("{e}（若数据源本应可写，请检查面板上的只读开关与库权限）"),
                clock,
            )),
        }
    }

    // ⑥ 错误信封（错令牌）
    if !options.check_wrong_token {
        items.push(CheckItem::skip(
            "错误信封 / 错令牌",
            "未启用（会在服务端记一条失败审计）",
        ));
    } else {
        let clock = Instant::now();
        let wrong = Target {
            token: format!("{}-wrong-token", target.token),
            ..target.clone()
        };
        match client::login(&wrong) {
            Ok(_) => items.push(CheckItem::fail(
                "错误信封 / 错令牌",
                "错令牌竟然登录成功 —— 服务端鉴权可能没生效".to_string(),
                clock,
            )),
            Err(e) if e.trim().is_empty() => items.push(CheckItem::fail(
                "错误信封 / 错令牌",
                "已拒绝，但错误信息为空（错误信封不合规）".to_string(),
                clock,
            )),
            Err(e) => items.push(CheckItem::pass(
                "错误信封 / 错令牌",
                format!("已正确拒绝：{}", short(&e, 120)),
                clock,
            )),
        }
    }

    let passed = items.iter().filter(|i| i.status == CheckStatus::Pass).count();
    let failed = items.iter().filter(|i| i.status == CheckStatus::Fail).count();
    let skipped = items.iter().filter(|i| i.status == CheckStatus::Skip).count();
    SelfCheckReport {
        target: target.masked(),
        items,
        passed,
        failed,
        skipped,
        total_ms: started.elapsed().as_millis() as u64,
    }
}

/// 截断长文本（保留语义，界面不撑爆）。
fn short(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    if count <= limit {
        return flat;
    }
    let head: String = flat.chars().take(limit).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_truncates_by_chars() {
        assert_eq!(short("SELECT 1", 40), "SELECT 1");
        assert_eq!(short("SELECT   1\n FROM t", 40), "SELECT 1 FROM t");
        assert_eq!(short("abcdefghij", 4), "abcd…");
    }

    #[test]
    fn default_options_are_sane() {
        let options = SelfCheckOptions::default();
        assert_eq!(options.sql, "SELECT 1");
        assert!(options.table.is_empty());
        assert!(options.write_sql.is_empty());
        assert!(!options.check_wrong_token);
    }

    #[test]
    fn unreachable_target_fails_login_first() {
        let target = Target::parse("Server=http://127.0.0.1:1;Database=demo;Password=tk").unwrap();
        let report = selfcheck(&target, &SelfCheckOptions::default());
        assert_eq!(report.items[0].name, "连接 / Login");
        assert_eq!(report.items[0].status, CheckStatus::Fail);
        assert!(report.failed >= 1);
        assert!(!report.ok());
    }
}
