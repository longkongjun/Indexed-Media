//! 通知中心域：应用内通知与可选出站 Webhook 的边界。
//!
//! `NotificationSink` 是类型化端口；应用内存储是唯一必达实现，Webhook 投递是
//! 附加通道，其健康独立表达且绝不成为本地任务的成功条件。

pub mod routes;
pub mod store;
pub mod webhook;
