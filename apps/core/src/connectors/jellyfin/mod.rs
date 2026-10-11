//! Jellyfin 独立连接：配置、加密凭据与连接测试（`ServiceHealthProvider` 边界）。
//!
//! 边界不变量（`MVP-JF-001/002/003`）：只提供地址、凭据、连接测试与健康投影；
//! 不建立资源库映射、不请求扫描、不验证媒体可见性；未配置或不可用只体现为独立
//! 健康状态，绝不阻塞、降级或改变本地任务。

pub mod client;
pub mod model;
pub mod store;

pub use client::{HttpJellyfinTester, JellyfinConnectionTester, JellyfinProbe, probe_to_result};
pub use model::{JellyfinConfigCommand, JellyfinConnectionTestResult, JellyfinIntegrationView};
pub(crate) use store::JellyfinStore;
pub(crate) use store::project as project_jellyfin;
