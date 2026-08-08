//! 仅从已核对本地结果生成的正式媒体目录记录。

use async_trait::async_trait;
use uuid::Uuid;

use crate::shared::error::AppError;

use self::model::VerifiedLocalResult;

/// 目录领域类型及对外读模型。
pub mod model;
/// 已认证的目录查询 HTTP 路由。
pub mod routes;
/// 已核对结果的事务投影与目录查询存储。
pub mod store;

#[async_trait]
/// organization completion 只可提交已核对本地结果的窄应用端口。
pub trait CatalogLocalResultPort: Send + Sync {
    /// 幂等应用一个账户内本地结果，并返回稳定媒体项目 ID。
    ///
    /// # Errors
    ///
    /// 结果不合法、账户不匹配、幂等冲突或持久化失败时返回稳定应用错误。
    async fn apply_local_result(
        &self,
        account_id: Uuid,
        result: &VerifiedLocalResult,
        now_us: i64,
    ) -> Result<Uuid, AppError>;
}
