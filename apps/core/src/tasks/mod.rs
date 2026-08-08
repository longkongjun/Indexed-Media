/// 序列化的任务事件信封和稳定事件名称。
pub mod events;
/// 扫描任务、租约、文件、错误和计数器数据类型。
pub mod model;
/// 绑定 revision 的单文件持久化处理任务。
pub mod processing;
/// 用于任务创建和控制的已认证 HTTP 路由。
pub mod routes;
/// 账户范围扫描用例。
pub mod service;
/// 事务性 `SQLite` 任务生命周期和结果持久化。
pub mod store;
/// 一个扫描尝试的租约感知工作器编排。
pub mod worker;
