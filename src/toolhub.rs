//! 桌面工具包托管：面板上传新版本，工具端检测 / 下载并自我升级。
//!
//! 存储**复用** `dhrust::upgrade::PackageStore`（版本化文件名 + `<file>.json` 元数据 + SHA-256），
//! 不自己造包格式：
//!
//! ```text
//! {base}/Tools/pek-rdbproxy-tool-0.1.29        二进制
//! {base}/Tools/pek-rdbproxy-tool-0.1.29.json   版本 / 大小 / SHA-256 / 上传时间 / 备注
//! ```

use std::path::Path;

use dhrust::upgrade::{Package, PackageStore};
use serde_json::{Value, json};

/// 存储目录（相对程序基目录）
pub const DIR: &str = "Tools";
/// 文件名前缀
pub const PREFIX: &str = "pek-rdbproxy-tool";
/// 上传大小上限（64MB，与代理自升级一致）
pub const MAX_BYTES: usize = 64 * 1024 * 1024;

/// 工具包仓库。
pub fn store(base: &Path) -> PackageStore {
    PackageStore::new(base.join(DIR), PREFIX)
}

/// 最新一版（`list()` 已按上传时间倒序）。
pub fn latest(base: &Path) -> Option<Package> {
    store(base).list().into_iter().next()
}

/// `GET /api/tool` 的响应体（供工具端检查更新）。
pub fn info(base: &Path) -> Value {
    match latest(base) {
        Some(pkg) => json!({
            "present": true,
            "version": pkg.version,
            "size": pkg.size,
            "sha256": pkg.sha256,
            "uploadedAt": pkg.uploaded_at,
            "notes": pkg.notes,
            "file": pkg.file,
            "downloadUrl": format!("/api/tool/download/{}", pkg.file),
        }),
        None => json!({ "present": false }),
    }
}
