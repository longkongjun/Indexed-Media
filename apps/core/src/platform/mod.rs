/// 仅追加的管理审计记录。
pub mod audit;
/// `SQLite` 备份、清单验证和恢复流程。
pub mod backup;
/// 受限目录能力的操作系统实现。
pub mod capability_fs;
/// Core `SQLite` 连接所有权和迁移初始化。
pub mod db;
/// 顶层 HTTP 路由、就绪门控和安全响应头。
pub mod http;
/// 内嵌 Schema 迁移及迁移期间的备份策略。
pub mod migrations;
/// 事务性任务事件、重放、保留和订阅者通知。
pub mod outbox;
/// 经校准的 Argon2id 密码哈希和验证。
pub mod password;
/// 加密令牌生成和按域分隔的 SHA-256 摘要。
pub mod random;
/// 会话、CSRF、来源和受信任代理请求守卫。
pub mod request_security;
/// 连接器凭据的每实例密钥创建与认证加密。
pub mod secrets;
/// 可重放的服务器发送任务事件流。
pub mod sse;
/// 安全的静态资源和 SPA 回退响应。
pub mod static_web;
/// 扫描工作器构建、生命周期控制和根目录验证。
pub mod task_runtime;
