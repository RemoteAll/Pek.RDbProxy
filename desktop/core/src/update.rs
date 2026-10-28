//! 工具自升级：从**代理托管**的工具包（`GET /api/tool`）检测新版本并原地替换。
//!
//! 复用现成能力，不造新格式：
//! - 代理端存储 = `dhrust::upgrade::PackageStore`（`{base}/Tools/pek-rdbproxy-tool-<版本>`）；
//! - 版本比较 = `dhrust::version::is_newer`；
//! - 程序校验 = `dhrust::upgrade::is_executable`（PE / ELF 魔数）；
//! - 原地替换 = `dhrust::upgrade::replace_file`（Windows 先给运行中的 exe 改名让位，再放入新文件）。
//!
//! 因此「代理上升级工具包 → 工具点一下就能升级自己」不需要任何额外服务。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{USER_AGENT, normalize_server};

/// 当前工具版本（编译期）。
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 查询元数据的超时。
const META_TIMEOUT: Duration = Duration::from_secs(10);
/// 下载工具包的超时（几十 MB 走局域网也够）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// 代理上托管的最新工具包。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTool {
    /// 是否已上传过工具包
    pub present: bool,
    /// 版本号
    pub version: String,
    /// 字节数
    pub size: u64,
    /// SHA-256（十六进制）
    pub sha256: String,
    /// 上传时间
    pub uploaded_at: String,
    /// 上传备注
    pub notes: String,
    /// 存储文件名
    pub file: String,
    /// 下载路径（相对代理根，如 `/api/tool/download/pek-rdbproxy-tool-0.1.30`）
    pub download_url: String,
}

/// 一次「检查更新」的结果（界面直接展示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// 代理地址（已归一化）
    pub server: String,
    /// 本机当前版本
    pub current: String,
    /// 代理上是否有工具包
    pub present: bool,
    /// 代理上的最新版本（没有则空）
    pub latest: String,
    /// 是否有更新
    pub newer: bool,
    /// 新包字节数
    pub size: u64,
    /// 新包 SHA-256
    pub sha256: String,
    /// 上传时间
    pub uploaded_at: String,
    /// 上传备注（通常写这次改了什么）
    pub notes: String,
    /// 下载路径
    pub download_url: String,
}

/// 拉取代理上托管的工具包信息。
pub fn remote_tool(server: &str) -> Result<RemoteTool, String> {
    let server = normalize_server(server)?;
    let url = format!("{server}/api/tool");
    let text = pek_rcode::http::get_text_ua(&url, None, META_TIMEOUT, Some(USER_AGENT))
        .map_err(|e| format!("查询工具版本失败（{url}）：{e}"))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("工具版本响应不是 JSON（{url}）：{e}"))?;
    // 面板接口统一 `{ok,data}` 信封；兼容未包信封的实现
    if !value
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("（服务端未给出原因）");
        return Err(format!("查询工具版本失败（{url}）：{message}"));
    }
    let value = match value.get("data") {
        Some(data) if data.is_object() => data.clone(),
        _ => value,
    };
    Ok(RemoteTool {
        present: value
            .get("present")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        version: text_of(&value, "version"),
        size: value.get("size").and_then(Value::as_u64).unwrap_or(0),
        sha256: text_of(&value, "sha256"),
        uploaded_at: text_of(&value, "uploadedAt"),
        notes: text_of(&value, "notes"),
        file: text_of(&value, "file"),
        download_url: text_of(&value, "downloadUrl"),
    })
}

/// 检查更新（比对代理上托管的版本与本机版本）。
pub fn check(server: &str) -> Result<UpdateCheck, String> {
    let remote = remote_tool(server)?;
    let newer = remote.present
        && !remote.version.is_empty()
        && dhrust::version::is_newer(&remote.version, CURRENT_VERSION);
    Ok(UpdateCheck {
        server: normalize_server(server)?,
        current: CURRENT_VERSION.to_string(),
        present: remote.present,
        latest: remote.version,
        newer,
        size: remote.size,
        sha256: remote.sha256,
        uploaded_at: remote.uploaded_at,
        notes: remote.notes,
        download_url: remote.download_url,
    })
}

/// 下载并原地替换当前程序，返回**新程序路径**（调用方随后重启自己）。
///
/// 校验链：字节数 → SHA-256 → PE/ELF 魔数 → 原子替换（失败会回滚旧文件）。
pub fn install(check: &UpdateCheck) -> Result<PathBuf, String> {
    if !check.present || check.download_url.is_empty() {
        return Err("代理上还没有上传桌面工具（面板「设置 → 桌面工具」上传）".to_string());
    }
    let exe = std::env::current_exe().map_err(|e| format!("取当前程序路径失败：{e}"))?;
    guard_dev_build(&exe)?;

    let url = format!("{}{}", check.server, check.download_url);
    let bytes = pek_rcode::http::get_bytes_auth(&url, None, None, DOWNLOAD_TIMEOUT, Some(USER_AGENT))
        .map_err(|e| format!("下载工具包失败（{url}）：{e}"))?;
    if bytes.is_empty() {
        return Err(format!("下载工具包为空（{url}）"));
    }
    if check.size > 0 && bytes.len() as u64 != check.size {
        return Err(format!(
            "工具包大小不符：期望 {} 字节，实际 {} 字节（{url}）",
            check.size,
            bytes.len()
        ));
    }
    if !check.sha256.is_empty() {
        let actual = dhrust::sign::sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(check.sha256.trim()) {
            return Err(format!(
                "工具包 SHA-256 不符：期望 {}，实际 {actual}（{url}）",
                check.sha256
            ));
        }
    }
    if !dhrust::upgrade::is_executable(&bytes) {
        return Err(format!(
            "下载内容不是可执行程序（PE/ELF 头校验失败，{} 字节）：{url}",
            bytes.len()
        ));
    }

    let staged = PathBuf::from(format!("{}.new", exe.display()));
    std::fs::write(&staged, &bytes)
        .map_err(|e| format!("写入新程序失败（{}）：{e}", staged.display()))?;
    dhrust::upgrade::replace_file(&staged, &exe)?;
    Ok(exe)
}

/// 启动新程序（升级后立刻重启，用户无需手动再双击）。
pub fn restart(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动新程序失败（{}）：{e}", exe.display()))
}

/// 开发构建（`target/debug`、`target/release`）不允许自替换——避免把源码构建产物换掉。
fn guard_dev_build(exe: &Path) -> Result<(), String> {
    let text = exe.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    if text.contains("/target/debug/") || text.contains("/target/release/") {
        return Err(format!(
            "当前是开发构建（{}），不允许自我替换：请用打包后的程序验证升级",
            exe.display()
        ));
    }
    Ok(())
}

/// 取值助手（缺字段 / 类型不对都当空串）。
fn text_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_version_is_parsable() {
        assert!(dhrust::upgrade::valid_version(CURRENT_VERSION));
    }

    #[test]
    fn newer_versions_are_detected() {
        assert!(dhrust::version::is_newer("0.1.99", "0.1.10"));
        assert!(!dhrust::version::is_newer("0.1.10", "0.1.10"));
        assert!(!dhrust::version::is_newer("0.1.9", "0.1.10"));
    }

    #[test]
    fn dev_build_is_refused() {
        let exe = PathBuf::from("G:/Code/Rust/Pek.RDbProxy/desktop/target/debug/pek-rdbproxy-tool.exe");
        assert!(guard_dev_build(&exe).is_err());
        let exe = PathBuf::from("C:/Tools/pek-rdbproxy-tool.exe");
        assert!(guard_dev_build(&exe).is_ok());
    }
}
