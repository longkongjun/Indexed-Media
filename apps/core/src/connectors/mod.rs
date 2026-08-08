/// 下载器连接、协议端口与持久化边界。
pub mod downloader;
/// Local-only identification enhancer configuration and protocol boundary.
pub mod enhancer;
/// 公开脱敏连接器 DTO 与稳定提供方错误。
pub mod model;
/// 已认证 TMDB 配置 HTTP 路由。
pub mod routes;
/// TMDB 凭据加密、连接测试与健康协调。
pub mod service;
/// 内置连接器配置的 `SQLite` 持久化。
pub mod store;
/// 有界 TMDB HTTP 客户端、缓存、映射与 single-flight 提供方。
pub mod tmdb;
