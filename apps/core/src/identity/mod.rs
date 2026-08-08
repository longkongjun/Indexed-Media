/// 身份命令及公开会话/账户表示。
pub mod model;
/// 引导和管理员会话的 HTTP 端点。
pub mod routes;
/// 身份用例、密码策略、限流和会话轮换。
pub mod service;
/// 管理员账户和会话的 `SQLite` 持久化。
pub mod store;

/// 身份用例实现，以及请求守卫和路由使用的契约。
pub use service::{IdentityService, IdentityUseCases};
