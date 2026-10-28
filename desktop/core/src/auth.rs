//! 代理用户登录：复用面板 `POST /api/login`，拿 JWT 当 `Authorization: Bearer`。
//!
//! 工具不再只是「拿着数据源令牌直连」——登录后每个请求都带上用户令牌，
//! 代理端就能按**这个人**的权限放行，并把用户名写进审计（面板能看出谁在查）。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{USER_AGENT, normalize_server};

/// 登录超时（登录接口本身很快；慢说明网络/地址不对）。
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15);

/// 登录结果（**只在进程内保存**，令牌不落盘）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// 代理地址（已归一化）
    pub server: String,
    /// 登录用户名
    pub user: String,
    /// 用户令牌（JWT）
    pub token: String,
    /// 内置管理员（全权限）
    pub builtin: bool,
    /// 授予的权限键（页签 + 数据源；形如 `db:read:demo`）
    pub permissions: Vec<String>,
    /// 面板把弱口令自动升级后的新密码（**只在这一次登录返回**，必须提示用户记住）
    pub new_password: Option<String>,
}

impl Session {
    /// 权限中文摘要（顶栏提示用；空 = 没有任何页面权限）。
    pub fn permission_summary(&self) -> String {
        if self.builtin {
            return "内置管理员（全部权限）".to_string();
        }
        if self.permissions.is_empty() {
            return "未授予任何权限".to_string();
        }
        self.permissions.join("、")
    }

    /// 是否能读某个数据源（`db:*` / `db:read:<名>` / `db:write:<名>` 任一即可）。
    pub fn can_read(&self, db: &str) -> bool {
        self.builtin
            || self.permissions.iter().any(|p| {
                p == "db:*" || p == &format!("db:read:{db}") || p == &format!("db:write:{db}")
            })
    }

    /// 是否能写某个数据源（`db:*` / `db:write:<名>`）。
    pub fn can_write(&self, db: &str) -> bool {
        self.builtin
            || self
                .permissions
                .iter()
                .any(|p| p == "db:*" || p == &format!("db:write:{db}"))
    }
}

/// 用面板账号登录代理。
pub fn login(server: &str, user: &str, password: &str) -> Result<Session, String> {
    let server = normalize_server(server)?;
    let user = user.trim();
    if user.is_empty() {
        return Err("用户名不能为空".to_string());
    }
    if password.is_empty() {
        return Err("密码不能为空".to_string());
    }
    let url = format!("{server}/api/login");
    let body = serde_json::json!({ "user": user, "password": password }).to_string();
    let text = pek_rcode::http::post_json_ua(&url, &body, None, LOGIN_TIMEOUT, Some(USER_AGENT))
        .map_err(|e| format!("登录请求失败（{url}）：{e}"))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("登录响应不是 JSON（{url}）：{e}；原文：{}", clip(&text)))?;
    if !value
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("（服务端未给出原因）");
        return Err(format!("登录失败（{url}）：{message}"));
    }
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    let token = data
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "登录成功但响应里没有 token".to_string())?;
    let permissions = data
        .get("permissions")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Ok(Session {
        server,
        user: data
            .get("user")
            .and_then(Value::as_str)
            .unwrap_or(user)
            .to_string(),
        token,
        builtin: data
            .get("builtin")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        permissions,
        new_password: data
            .get("newPassword")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// 截断长文本（日志/错误文案用）。
fn clip(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= 200 {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(200).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_checks_follow_db_keys() {
        let session = Session {
            server: "http://127.0.0.1:5599".into(),
            user: "zhang".into(),
            token: "t".into(),
            builtin: false,
            permissions: vec!["db:read:demo".into(), "db:write:other".into()],
            new_password: None,
        };
        assert!(session.can_read("demo"));
        assert!(!session.can_write("demo"));
        assert!(session.can_write("other"));
        assert!(!session.can_read("missing"));
        assert!(session.permission_summary().contains("db:read:demo"));
    }

    #[test]
    fn builtin_passes_everything() {
        let session = Session {
            server: "http://127.0.0.1:5599".into(),
            user: "admin".into(),
            token: "t".into(),
            builtin: true,
            permissions: Vec::new(),
            new_password: None,
        };
        assert!(session.can_read("anything"));
        assert!(session.can_write("anything"));
        assert!(session.permission_summary().contains("管理员"));
    }

    #[test]
    fn login_rejects_empty_credentials_before_network() {
        assert!(login("http://127.0.0.1:1", "  ", "x").is_err());
        assert!(login("http://127.0.0.1:1", "u", "").is_err());
    }
}
