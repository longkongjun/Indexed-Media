/// 基于显式路径标记、不会按大小误伤正片的辅助视频分类。
pub mod auxiliary;
/// 部署根目录声明与文件系统能力抽象。
pub mod capability;
/// 启动/周期/监听恢复对账任务与健康投影协调器。
pub mod coordinator;
/// 已校验的根目录/路径标识符与发现 API 数据类型。
pub mod model;
/// 扫描器观测批次及其持久化接收端契约。
pub mod observations;
/// 提供给 organization 的只读收件箱路径投影，不暴露 discovery 私有 store。
pub mod organization_projection;
/// 版本化稳定性、监听与周期对账策略。
pub mod policy;
/// Idempotent registered-inbox reconcile request boundary for automation.
pub mod reconcile_service;
/// 不可变文件 revision、稳定门禁与下游处理请求投影。
pub mod revisions;
/// 部署根目录与收件箱目录的已认证 HTTP 路由。
pub mod routes;
/// 受能力约束的递归目录遍历。
pub mod scanner;
/// 发现用例以及文件系统/数据库编排。
pub mod service;
/// 已配置收件箱目录的 `SQLite` 持久化。
pub mod store;
/// 有界 watcher callback 入口、路径映射和 2 秒事件合并。
pub mod watcher;

/// 发现用例实现及 HTTP/任务层使用的契约。
pub use service::{DiscoveryService, DiscoveryUseCases};
