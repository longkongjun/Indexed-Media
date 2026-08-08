/// 处理任务的传输与存储模型。
pub mod model;
/// 消费发现请求并暴露任务控制的应用服务。
pub mod service;
/// 处理任务生命周期的事务性持久化。
pub mod store;
/// 感知租约的处理工作器与已注册阶段处理器端口。
pub mod worker;
