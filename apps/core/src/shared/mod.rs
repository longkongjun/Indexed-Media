/// 稳定的应用错误码与安全的 HTTP 错误封装。
pub mod error;
/// 以 16 字节 blob 存储、由 `UUIDv7` 支撑的标识符。
pub mod id;
/// 已校验的游标分页请求与响应封装。
pub mod page;
/// 用于持久化边界的 UTC 微秒时间戳。
pub mod time;
