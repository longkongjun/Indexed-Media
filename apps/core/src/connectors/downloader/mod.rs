/// 下载器连接的认证 HTTP 纵向切片。
pub mod connection_routes;
/// 下载器连接用例、内置适配器注册表与脱敏映射。
pub mod connection_service;
/// 下载器连接的版本化加密持久化。
pub mod connection_store;
/// 产品无关的下载器领域模型。
pub mod model;
/// qBittorrent 与 Transmission 适配器共同实现的最小能力端口。
pub mod port;
/// 有界且禁止重定向的 qBittorrent `WebUI` API v2 适配器。
pub mod qbittorrent;
/// 下载任务后台循环、启动恢复与生产装配。
pub mod runtime;
/// 手工下载任务的认证 HTTP 纵向切片。
pub mod task_routes;
/// 手工任务验收、查询和脱敏投影用例。
pub mod task_service;
/// 下载任务源、幂等投影、租约和游标分页的加密持久化。
pub mod task_store;
/// 同时支持 legacy 5.3 与 JSON-RPC 2.0 6.x 的 Transmission 适配器。
pub mod transmission;
/// 下载任务的先查询后提交、批量监控与有限重试 worker。
pub mod worker;
