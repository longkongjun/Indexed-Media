use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::shared::error::{AppError, ErrorCode};

use super::plan_store::{OrganizationPlanRecord, OrganizationPlanStore};
use super::planner::{OrganizationPlanner, PlanningDecision, PlanningInput};

#[async_trait]
/// 为 planner 聚合当前任务、身份、来源、目标配置和安全事实的应用端口。
pub trait OrganizationPlanningPort: Send + Sync {
    /// 加载一次规划使用的有界当前事实。
    ///
    /// # Errors
    ///
    /// 任务/身份/目标不存在，事实不完整或底层读取失败时返回稳定应用错误。
    async fn load(&self, account_id: Uuid, task_id: Uuid) -> Result<PlanningInput, AppError>;
}

#[derive(Clone)]
/// 协调当前事实读取、纯 planner 与不可变 plan store。
pub struct OrganizationPlanService {
    port: Arc<dyn OrganizationPlanningPort>,
    store: OrganizationPlanStore,
}

impl OrganizationPlanService {
    /// 使用显式规划端口和 store 创建服务。
    #[must_use]
    pub fn new(port: Arc<dyn OrganizationPlanningPort>, pool: sqlx::SqlitePool) -> Self {
        Self {
            port,
            store: OrganizationPlanStore::new(pool),
        }
    }

    /// 返回与当前输入完全等价的最新计划，否则持久化下一版本。
    ///
    /// # Errors
    ///
    /// 当前事实读取、纯规划或事务持久化失败时返回稳定应用错误。
    pub async fn ensure_current(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<OrganizationPlanRecord, AppError> {
        let draft = OrganizationPlanner::plan(&self.port.load(account_id, task_id).await?)
            .map_err(planning_error)?;
        if let Some(current) = self.store.current(account_id, task_id).await?
            && equivalent_draft(&current.draft, &draft)
        {
            return Ok(current);
        }
        self.store.persist_next(account_id, draft, now_us()).await
    }

    /// 以幂等键显式创建使用当前事实的下一计划版本。
    ///
    /// # Errors
    ///
    /// 当前事实读取、规划、幂等绑定或事务持久化失败时返回稳定应用错误。
    pub async fn recalculate(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        idempotency_key: &str,
    ) -> Result<OrganizationPlanRecord, AppError> {
        let draft = OrganizationPlanner::plan(&self.port.load(account_id, task_id).await?)
            .map_err(planning_error)?;
        self.store
            .recalculate(account_id, idempotency_key, draft, now_us())
            .await
    }

    /// 为当前计划保存一次性授权；不会修改计划行或创建复用规则。
    ///
    /// # Errors
    ///
    /// 版本过期、计划含不可覆盖风险、幂等冲突或数据库失败时返回稳定应用错误。
    pub async fn authorize_once(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        plan_version: i64,
        idempotency_key: &str,
    ) -> Result<OrganizationPlanRecord, AppError> {
        self.store
            .authorize_once(account_id, task_id, plan_version, idempotency_key, now_us())
            .await
    }
}

fn equivalent_draft(left: &super::planner::PlanDraft, right: &super::planner::PlanDraft) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    // snapshot schema intentionally retains semantic config version/fields, not the target
    // aggregate's display-only update timestamp.
    left.target.updated_at_us = 0;
    right.target.updated_at_us = 0;
    left == right
}

fn planning_error(decision: PlanningDecision) -> AppError {
    let code = match decision {
        PlanningDecision::PathOutsideRoot => ErrorCode::PathInvalid,
        PlanningDecision::GenericGroupingAmbiguous
        | PlanningDecision::TargetKindMismatch
        | PlanningDecision::IdentityIncomplete => ErrorCode::ValidationFailed,
    };
    AppError::new(code, format!("organization planning paused: {decision:?}"))
}

fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}
