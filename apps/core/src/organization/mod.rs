/// `ProcessingTask` 四阶段协调器与生产规划事实适配器。
pub mod coordinator;
/// journal-first 文件执行、恢复与受控补偿应用编排。
pub mod executor;
/// 受 deployment-root 能力约束的 no-clobber 文件操作端口与领域事实。
pub mod fs;
/// version-checked 文件 journal、LocalResult 与 rollback receipt 持久化。
pub mod journal_store;
/// 整理目标、profile 与有界规则的领域类型。
pub mod model;
/// 从已确认有界字段确定性生成缺失 NFO，或保留已有字节。
pub mod nfo;
/// 当前事实、纯 planner 与不可变 store 的应用编排。
pub mod plan_service;
/// 不可变配置快照、计划、operation 与幂等回执持久化。
pub mod plan_store;
/// 从已核对身份和固定配置生成安全不可变计划的纯函数边界。
pub mod planner;
/// discovery 用于反向保护 inbox 创建的目标路径只读投影。
pub mod target_projection;
/// 目标 API 的认证、请求安全、审计和路由边界。
pub mod target_routes;
/// 目标 preflight、版本化 CRUD 和安全公开投影的应用服务。
pub mod target_service;
/// 整理目标聚合的事务化 `SQLite` 持久化。
pub mod target_store;
/// `ProcessingTask` organization 详情与命令 HTTP/应用边界。
pub mod task_routes;
