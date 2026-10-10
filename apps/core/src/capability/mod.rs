//! AI 控制面（DSH）能力网关域：`media-capability.v1` 协议类型与后续的 capability
//! registry、鉴权、幂等与事件分发。P0 冻结信封契约；网关路由与能力实现自 A1 起落地。
//!
//! 不变量：Core 不信任调用方校验——capability、输入 schema、路径与凭据在 Core 侧
//! 再次校验；AI 不得经由此协议提交任意绝对路径、shell、SQL 或凭据。

pub mod model;
