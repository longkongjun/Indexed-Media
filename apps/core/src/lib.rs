//! `MediaFlow` 的本地优先服务端能力。
//!
//! 此 crate 负责服务端引导、已认证的管理能力、受能力约束的文件系统发现、持久化扫描任务，
//! 以及由 `SQLite` 支撑的事件交付基础设施。它不播放或转码媒体、不聚合第三方站点，
//! 也不需要云端控制面。

/// Versioned automation sources and durable source execution.
pub mod automation;

/// 启动配置与顶层 HTTP 路由构建。
pub mod bootstrap;
/// 正式本地媒体记录及已核对本地结果投影边界。
pub mod catalog;
/// 内置元数据连接器配置、健康与提供方边界。
pub mod connectors;
/// 受能力约束的部署根目录、收件箱目录与文件系统扫描。
pub mod discovery;
/// 确定性本地身份线索、有界 NFO 解析与候选身份类型。
pub mod identification;
/// 单管理员引导、会话认证与凭据持久化。
pub mod identity;
/// 版本化整理目标、计划、安全文件操作和可恢复结果。
pub mod organization;
/// `SQLite`、文件系统、HTTP、安全、备份、发件箱与运行时适配器。
pub mod platform;
/// 跨功能的 ID、分页、错误与时间辅助工具。
pub mod shared;
/// 持久化扫描任务生命周期、观测结果、工作器与 API 事件。
pub mod tasks;
