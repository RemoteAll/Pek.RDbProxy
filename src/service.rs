//! 服务管理（薄壳；实现已下沉 `dhrust::service`，与 Pek.RAgent 同款）。
//!
//! 服务名/显示名固定写死（部署标准：星尘托管 Mode=11 影子模式）。

use std::path::Path;

pub use dhrust::service::{ServiceManager, ServiceState};

/// Windows 服务名 / systemd 单元名
pub const SERVICE_NAME: &str = "PekRDbProxy";
/// 服务显示名
pub const DISPLAY_NAME: &str = "Pek.RDbProxy";
/// 服务描述
pub const DESCRIPTION: &str = "Pek 本地数据库代理（项目侧只见代理地址与令牌）";

/// 按程序基目录装配服务管理器。
pub fn manager(base: &Path) -> ServiceManager {
    ServiceManager::new(base, SERVICE_NAME, DISPLAY_NAME, DESCRIPTION, &[])
}
