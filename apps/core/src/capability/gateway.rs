//! `media-capability.v1` 能力网关：AI 通道的 HTTP 边界。
//!
//! 责任链：令牌认证 → 信封解析（serde 严格）→ 能力登记检查 → 幂等重放/冲突 → 输入
//! 校验（白名单 + 危险模式）→ 执行模式与 revision 检查 → 执行 → 结果持久化与事件留痕。
//! 错误一律映射为结构化 [`CapabilityError`] 信封；诊断细节不回传 AI。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::Row as _;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::capability::handlers::{self, CapabilityOutcome};
use crate::capability::model::{
    CapabilityCall, CapabilityError, CapabilityErrorCategory, CapabilityErrorPhase,
    CapabilityEvent, CapabilityEventType, CapabilityEvidenceKind, CapabilityEvidenceRef,
    CapabilityName, CapabilityNextAction, CapabilityResult, CapabilityResultStatus,
    PROTOCOL_VERSION,
};
use crate::capability::registry::{self, CapabilitySpec};
use crate::capability::store::{CapabilityCallRecord, CapabilityStore};
use crate::platform::db::Db;
use crate::platform::task_runtime::{SystemTaskClock, TaskClock};
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone)]
struct CapabilityHttpState {
    config: AppConfig,
    store: CapabilityStore,
    organization: Option<Arc<crate::organization::task_routes::OrganizationTaskService>>,
    clock: Arc<dyn TaskClock>,
}

/// 组装能力协议路由；依赖部署时注入的 capability 令牌。
///
/// `organization` 为部署根可用时组装的整理命令服务；缺失时写能力稳定拒绝为
/// precondition（部署根不可用不改变只读能力）。
pub fn router(config: AppConfig, db: &Db) -> Router {
    router_with_organization(config, db, None)
}

/// 使用整理命令服务组装能力协议路由。
pub fn router_with_organization(
    config: AppConfig,
    db: &Db,
    organization: Option<Arc<crate::organization::task_routes::OrganizationTaskService>>,
) -> Router {
    Router::new()
        .route("/api/capability/v1/calls", post(invoke_capability))
        .route("/api/capability/v1/events", get(list_events))
        .with_state(Arc::new(CapabilityHttpState {
            config,
            store: CapabilityStore::new(db.pool().clone()),
            organization,
            clock: Arc::new(SystemTaskClock),
        }))
}

fn authenticate(state: &CapabilityHttpState, headers: &HeaderMap) -> Result<(), CapabilityError> {
    let Some(configured) = state.config.capability_token.as_deref() else {
        return Err(capability_error(
            ErrorCode::IntegrationNotConfigured,
            CapabilityErrorPhase::Scan,
            "capability gateway token is not configured",
        ));
    };
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if presented.is_empty()
        || presented.len() != configured.len()
        || !bool::from(presented.as_bytes().ct_eq(configured.as_bytes()))
    {
        return Err(capability_error(
            ErrorCode::PermissionDenied,
            CapabilityErrorPhase::Scan,
            "capability gateway authentication failed",
        ));
    }
    Ok(())
}

async fn invoke_capability(
    State(state): State<Arc<CapabilityHttpState>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Err(error) = authenticate(&state, &headers) {
        return error_response(CapabilityResultStatus::Rejected, None, error);
    }
    let Ok(call) = serde_json::from_slice::<CapabilityCall>(&body) else {
        return error_response(
            CapabilityResultStatus::Rejected,
            None,
            capability_error(
                ErrorCode::ValidationFailed,
                CapabilityErrorPhase::Scan,
                "capability call envelope is invalid",
            ),
        );
    };
    match invoke_inner(&state, call).await {
        Ok(result) => (StatusCode::OK, Json(result)).into_response(),
        Err(outcome) => outcome,
    }
}

/// 网关内层执行链；错误统一转为结果信封响应（HTTP 层仍 200，结构化 status 表达语义）。
async fn invoke_inner(
    state: &Arc<CapabilityHttpState>,
    call: CapabilityCall,
) -> Result<CapabilityResult, Response> {
    let Some(spec) = registry::lookup(call.capability) else {
        return Err(error_response(
            CapabilityResultStatus::Rejected,
            call.task_id,
            capability_error(
                ErrorCode::NotFound,
                CapabilityErrorPhase::Scan,
                "capability is not registered",
            ),
        ));
    };
    if let Some(record) = state
        .store
        .find_call(call.request_id)
        .await
        .map_err(|error| storage_failure(&error))?
    {
        return Ok(record.result);
    }
    if let Some(_owner) = state
        .store
        .idempotency_owner(
            &call.execution.idempotency_key,
            spec.name.as_str(),
            call.request_id,
        )
        .await
        .map_err(|error| storage_failure(&error))?
    {
        return Err(error_response(
            CapabilityResultStatus::Rejected,
            call.task_id,
            capability_error(
                ErrorCode::RequestConflict,
                CapabilityErrorPhase::Scan,
                "idempotency key is already bound to another request",
            ),
        ));
    }
    preflight(state, &call, spec).await?;
    let outcome = match call.capability {
        CapabilityName::MediaTaskStatus => {
            handlers::task_status(state.store.pool(), &call.input).await
        }
        CapabilityName::MediaMatchCandidates => {
            handlers::match_candidates(state.store.pool(), &call.input).await
        }
        CapabilityName::MediaCreatePlan
            if call.input.get("dry_run") == Some(&serde_json::json!(true)) =>
        {
            recalculate_plan(state, &call).await
        }
        CapabilityName::MediaCreatePlan => {
            handlers::plan_preview(state.store.pool(), &call.input).await
        }
        CapabilityName::MediaRequestApproval => approval_state(state, &call).await,
        CapabilityName::MediaApplyPlan => apply_plan(state, &call).await,
        CapabilityName::MediaRollback => rollback_plan(state, &call).await,
        CapabilityName::MediaScan
        | CapabilityName::MediaRetry
        | CapabilityName::MediaCancel
        | CapabilityName::MediaSyncExternal => {
            return Err(error_response(
                CapabilityResultStatus::Rejected,
                call.task_id,
                capability_error(
                    ErrorCode::NotFound,
                    CapabilityErrorPhase::Scan,
                    "capability has no handler in this build",
                ),
            ));
        }
    };
    let now_us = state.clock.now_us();
    let result = match outcome {
        Ok(outcome) => assemble_result(&call, outcome),
        Err(error) => {
            let status = if error.code() == ErrorCode::NotReady {
                CapabilityResultStatus::Rejected
            } else {
                CapabilityResultStatus::Failed
            };
            let error = app_error_to_capability(&error);
            CapabilityResult {
                protocol_version: PROTOCOL_VERSION.to_owned(),
                request_id: call.request_id,
                task_id: call.task_id,
                status,
                result: None,
                evidence: Vec::new(),
                next_actions: vec![],
                error: Some(error),
            }
        }
    };
    persist(state, &call, &result, now_us).await?;
    Ok(result)
}

async fn persist(
    state: &Arc<CapabilityHttpState>,
    call: &CapabilityCall,
    result: &CapabilityResult,
    now_us: i64,
) -> Result<(), Response> {
    let record = CapabilityCallRecord {
        request_id: call.request_id,
        idempotency_key: call.execution.idempotency_key.clone(),
        capability: call.capability.as_str().to_owned(),
        result: result.clone(),
    };
    state
        .store
        .persist_call(&record, "ai", "dsh", now_us)
        .await
        .map_err(|error| storage_failure(&error))?;
    let event_type = if matches!(
        result.status,
        CapabilityResultStatus::Succeeded | CapabilityResultStatus::NeedsInput
    ) {
        CapabilityEventType::CapabilityCompleted
    } else {
        CapabilityEventType::CapabilityFailed
    };
    let event = CapabilityEvent {
        event_id: Uuid::now_v7(),
        sequence: 0,
        event_type,
        occurred_at: chrono::TimeZone::timestamp_micros(&chrono::Utc, now_us)
            .single()
            .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
            .unwrap_or_default(),
        task_id: result.task_id,
        request_id: Some(call.request_id),
        operation_id: None,
        state: serde_json::to_value(result.status)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned)),
        progress: None,
        evidence_refs: Some(
            result
                .evidence
                .iter()
                .map(|evidence| format!("{}:{}", evidence.kind_id_prefix(), evidence.id))
                .collect(),
        ),
        error: result.error.clone(),
    };
    state
        .store
        .append_event(&event)
        .await
        .map_err(|error| storage_failure(&error))?;
    Ok(())
}

/// `media.create-plan`（`dry_run`）：重算并持久化新计划版本，返回明细但不执行。
async fn recalculate_plan(
    state: &Arc<CapabilityHttpState>,
    call: &CapabilityCall,
) -> Result<CapabilityOutcome, AppError> {
    let service = organization_service(state)?;
    let task_id = input_task(call)?;
    let account_id = account_of(state, task_id).await?;
    let view = service
        .recalculate(account_id, task_id, &call.execution.idempotency_key)
        .await?;
    plan_outcome_from_view(state, task_id, view).await
}

/// `media.request-approval`：如实反馈当前计划授权状态；审批本体由 A4 的 Web 审批链承载。
async fn approval_state(
    state: &Arc<CapabilityHttpState>,
    call: &CapabilityCall,
) -> Result<CapabilityOutcome, AppError> {
    let task_id = input_task(call)?;
    let account_id = account_of(state, task_id).await?;
    let plan = sqlx::query("SELECT id,version,authorization FROM organization_plans WHERE account_id=? AND task_id=? ORDER BY version DESC LIMIT 1")
        .bind(account_id.as_bytes().to_vec())
        .bind(task_id.as_bytes().to_vec())
        .fetch_optional(state.store.pool())
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
    let Some(plan) = plan else {
        return Ok(needs_input_outcome(
            task_id,
            "task has no persisted plan yet",
        ));
    };
    let authorization: String = plan.try_get("authorization").map_err(internal)?;
    let version: i64 = plan.try_get("version").map_err(internal)?;
    if authorization == "paused" {
        return Ok(CapabilityOutcome {
            status: CapabilityResultStatus::NeedsApproval,
            task_id: Some(task_id),
            result: Some(single_field_map("plan_version", &version.to_string())),
            evidence: vec![CapabilityEvidenceRef {
                id: uuid_string(
                    plan.try_get::<Vec<u8>, _>("id")
                        .map_err(internal)?
                        .as_slice(),
                ),
                kind: CapabilityEvidenceKind::Plan,
            }],
            next_actions: vec![CapabilityNextAction::AskUser],
        });
    }
    Ok(CapabilityOutcome {
        status: CapabilityResultStatus::Succeeded,
        task_id: Some(task_id),
        result: Some(single_field_map("authorization", &authorization)),
        evidence: Vec::new(),
        next_actions: Vec::new(),
    })
}

/// `media.apply-plan`（`reversible-write`）：只交付已获 Web 人工授权的计划版本。
///
/// SA-1-001 修复：capability 通道不再产生授权回执——无有效人工回执时返回
/// `needs_approval`（无执行、无回执、无文件副作用），审批权威界面是 Web。
async fn apply_plan(
    state: &Arc<CapabilityHttpState>,
    call: &CapabilityCall,
) -> Result<CapabilityOutcome, AppError> {
    let service = organization_service(state)?;
    let task_id = input_task(call)?;
    let plan_version = revision_number(call, "plan.version")?;
    let account_id = account_of(state, task_id).await?;
    match service
        .deliver_authorized(
            account_id,
            task_id,
            plan_version,
            &call.execution.idempotency_key,
        )
        .await?
    {
        crate::organization::task_routes::DeliverOutcome::Delivered(view) => {
            plan_outcome_from_view(state, task_id, *view).await
        }
        crate::organization::task_routes::DeliverOutcome::NeedsApproval {
            plan_id,
            plan_version,
        } => Ok(CapabilityOutcome {
            status: CapabilityResultStatus::NeedsApproval,
            task_id: Some(task_id),
            result: Some(single_field_map("plan_version", &plan_version.to_string())),
            evidence: vec![CapabilityEvidenceRef {
                id: uuid_string(plan_id.as_bytes()),
                kind: CapabilityEvidenceKind::Plan,
            }],
            next_actions: vec![CapabilityNextAction::RequestApproval],
        }),
    }
}

/// `media.rollback`（`reversible-write`）：`expected_revision`（`result.version:N`）绑定结果版本回滚。
async fn rollback_plan(
    state: &Arc<CapabilityHttpState>,
    call: &CapabilityCall,
) -> Result<CapabilityOutcome, AppError> {
    let service = organization_service(state)?;
    let task_id = input_task(call)?;
    let result_version = revision_number(call, "result.version")?;
    let account_id = account_of(state, task_id).await?;
    service
        .rollback(
            account_id,
            task_id,
            result_version,
            &call.execution.idempotency_key,
        )
        .await?;
    Ok(CapabilityOutcome {
        status: CapabilityResultStatus::Accepted,
        task_id: Some(task_id),
        result: Some(single_field_map("rollback", "requested")),
        evidence: Vec::new(),
        next_actions: Vec::new(),
    })
}

fn organization_service(
    state: &Arc<CapabilityHttpState>,
) -> Result<Arc<crate::organization::task_routes::OrganizationTaskService>, AppError> {
    state.organization.clone().ok_or_else(|| {
        AppError::new(
            ErrorCode::NotReady,
            "organization commands are unavailable while deployment roots are missing",
        )
    })
}

fn input_task(call: &CapabilityCall) -> Result<Uuid, AppError> {
    call.input
        .get("task_id")
        .and_then(|value| value.as_str())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| AppError::new(ErrorCode::ValidationFailed, "task_id must be a uuid"))
}

async fn account_of(state: &Arc<CapabilityHttpState>, task_id: Uuid) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Vec<u8>>("SELECT account_id FROM tasks_processing_tasks WHERE id=?")
        .bind(task_id.as_bytes().to_vec())
        .fetch_optional(state.store.pool())
        .await
        .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
        .and_then(|bytes| Uuid::from_slice(&bytes).ok())
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))
}

fn revision_number(call: &CapabilityCall, prefix: &str) -> Result<i64, AppError> {
    let revision = call
        .execution
        .expected_revision
        .as_deref()
        .and_then(|value| value.strip_prefix(&format!("{prefix}:")))
        .and_then(|value| value.parse::<i64>().ok());
    revision.ok_or_else(|| {
        AppError::new(
            ErrorCode::ValidationFailed,
            "expected_revision must use plan.version:<n> or result.version:<n>",
        )
    })
}

async fn plan_outcome_from_view(
    state: &Arc<CapabilityHttpState>,
    task_id: Uuid,
    _view: crate::organization::task_routes::ProcessingTaskOrganizationView,
) -> Result<CapabilityOutcome, AppError> {
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM organization_plans WHERE task_id=? ORDER BY version DESC LIMIT 1",
    )
    .bind(task_id.as_bytes().to_vec())
    .fetch_optional(state.store.pool())
    .await
    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
    .unwrap_or_default();
    Ok(CapabilityOutcome {
        status: CapabilityResultStatus::Accepted,
        task_id: Some(task_id),
        result: Some(single_field_map("plan_version", &version.to_string())),
        evidence: Vec::new(),
        next_actions: Vec::new(),
    })
}

fn needs_input_outcome(task_id: Uuid, detail: &str) -> CapabilityOutcome {
    CapabilityOutcome {
        status: CapabilityResultStatus::NeedsInput,
        task_id: Some(task_id),
        result: Some(single_field_map("detail", detail)),
        evidence: Vec::new(),
        next_actions: vec![CapabilityNextAction::AskUser],
    }
}

fn single_field_map(key: &str, value: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert(key.to_owned(), Value::String(value.to_owned()));
    map
}

fn uuid_string(bytes: &[u8]) -> String {
    Uuid::from_slice(bytes)
        .map(|id| id.to_string())
        .unwrap_or_default()
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

/// 幂等重放与幂等键冲突检查之后、执行之前的调用预检。
///
/// 输入白名单与危险模式、执行模式许可、`expected_revision` 一致性；任一失败即稳定拒绝。
async fn preflight(
    state: &Arc<CapabilityHttpState>,
    call: &CapabilityCall,
    spec: &'static CapabilitySpec,
) -> Result<(), Response> {
    if let Err(error) = registry::validate_input(spec, &call.input) {
        return Err(error_response(
            CapabilityResultStatus::Rejected,
            call.task_id,
            app_error_to_capability(&error),
        ));
    }
    if !spec.allowed_modes.contains(&call.execution.mode) {
        return Err(error_response(
            CapabilityResultStatus::Rejected,
            call.task_id,
            capability_error(
                ErrorCode::TaskInvalidState,
                CapabilityErrorPhase::Planning,
                "execution mode is not allowed for this capability",
            ),
        ));
    }
    // 通用 revision 防护只覆盖 task.version 前缀；写能力以 plan.version:/result.version:
    // 前缀由各自 handler 解析并交由整理命令的版本校验精确防护。
    if let (Some(task_id), Some(expected)) =
        (call.task_id, call.execution.expected_revision.as_deref())
        && expected.starts_with("task.version:")
        && let Err(error) =
            handlers::assert_task_revision(state.store.pool(), task_id, expected).await
    {
        return Err(error_response(
            CapabilityResultStatus::Rejected,
            Some(task_id),
            app_error_to_capability(&error),
        ));
    }
    Ok(())
}

fn assemble_result(call: &CapabilityCall, outcome: CapabilityOutcome) -> CapabilityResult {
    CapabilityResult {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        request_id: call.request_id,
        task_id: outcome.task_id,
        status: outcome.status,
        result: outcome
            .result
            .map(|map| map.into_iter().collect::<Map<String, Value>>()),
        evidence: outcome.evidence,
        next_actions: outcome.next_actions,
        error: None,
    }
}

#[derive(Deserialize)]
struct EventQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

async fn list_events(
    State(state): State<Arc<CapabilityHttpState>>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> Response {
    if let Err(error) = authenticate(&state, &headers) {
        return error_response(CapabilityResultStatus::Rejected, None, error);
    }
    let cursor = query
        .cursor
        .as_deref()
        .and_then(|cursor| cursor.strip_prefix("capability-events:"))
        .and_then(|value| value.parse::<i64>().ok());
    match state
        .store
        .list_events(cursor, query.limit.unwrap_or(50))
        .await
    {
        Ok(page) => (StatusCode::OK, Json(page)).into_response(),
        Err(_) => error_response(
            CapabilityResultStatus::Failed,
            None,
            capability_error(
                ErrorCode::Internal,
                CapabilityErrorPhase::Scan,
                "capability events are temporarily unavailable",
            ),
        ),
    }
}

fn storage_failure(error: &AppError) -> Response {
    error_response(
        CapabilityResultStatus::Retryable,
        None,
        app_error_to_capability(error),
    )
}

fn error_response(
    status: CapabilityResultStatus,
    task_id: Option<Uuid>,
    error: CapabilityError,
) -> Response {
    let result = CapabilityResult {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        request_id: Uuid::nil(),
        task_id: error.task_id.or(task_id),
        status,
        result: None,
        evidence: Vec::new(),
        next_actions: Vec::new(),
        error: Some(error),
    };
    (StatusCode::OK, Json(result)).into_response()
}

/// 把稳定应用错误映射为结构化能力错误；诊断细节不跨出 Core。
fn app_error_to_capability(error: &AppError) -> CapabilityError {
    capability_error(
        error.code(),
        CapabilityErrorPhase::Scan,
        "capability call failed",
    )
}

fn capability_error(
    code: ErrorCode,
    phase: CapabilityErrorPhase,
    message: &'static str,
) -> CapabilityError {
    let (category, retryable, required_action) = classify(code);
    CapabilityError {
        error_code: format!("capability.{}", code.as_str()),
        category,
        retryable,
        phase,
        task_id: None,
        operation_id: None,
        required_action,
        safe_details: single_detail(message),
        evidence_refs: Vec::new(),
    }
}

/// 错误码到机器分类、可重试性与补救动作的稳定映射。
fn classify(
    code: ErrorCode,
) -> (
    CapabilityErrorCategory,
    bool,
    crate::capability::model::CapabilityRequiredAction,
) {
    use crate::capability::model::CapabilityRequiredAction as Action;
    use CapabilityErrorCategory as Category;
    match code {
        ErrorCode::ValidationFailed
        | ErrorCode::PathInvalid
        | ErrorCode::PathEscape
        | ErrorCode::PathSymlinkForbidden
        | ErrorCode::PayloadTooLarge => (Category::Validation, false, Action::AskUser),
        ErrorCode::PermissionDenied
        | ErrorCode::InvalidCredentials
        | ErrorCode::SessionExpired
        | ErrorCode::CsrfInvalid
        | ErrorCode::OriginUntrusted
        | ErrorCode::IntegrationUnauthorized => (Category::Permission, false, Action::AskUser),
        ErrorCode::ConfigVersionConflict
        | ErrorCode::RequestConflict
        | ErrorCode::TaskLeaseLost
        | ErrorCode::ResourceConflict => (Category::Conflict, false, Action::Replan),
        ErrorCode::TaskNotFound
        | ErrorCode::NotFound
        | ErrorCode::InboxNotFound
        | ErrorCode::RootNotFound => (Category::NotFound, false, Action::None),
        ErrorCode::ProviderUnavailable
        | ErrorCode::OrganizationTargetUnavailable
        | ErrorCode::IntegrationNotConfigured
        | ErrorCode::IntegrationRateLimited => (Category::Provider, true, Action::Retry),
        ErrorCode::RateLimited => (Category::Transient, true, Action::Retry),
        ErrorCode::AutomationSourceDisabled
        | ErrorCode::AutomationSignatureInvalid
        | ErrorCode::AutomationReplay
        | ErrorCode::AutomationActionInvalid
        | ErrorCode::AutomationPayloadInvalid
        | ErrorCode::TaskInvalidState
        | ErrorCode::OrganizationRootReadOnly
        | ErrorCode::OrganizationTargetOverlap
        | ErrorCode::InboxOverlap
        | ErrorCode::NotReady => (Category::Precondition, false, Action::AskUser),
        ErrorCode::RootUnavailable
        | ErrorCode::BackupFailed
        | ErrorCode::ConfigInvalid
        | ErrorCode::DatabaseInvalid
        | ErrorCode::RestoreTargetNotEmpty
        | ErrorCode::BootstrapAlreadyCompleted
        | ErrorCode::BootstrapInvalidSecret
        | ErrorCode::Internal => (Category::Internal, false, Action::None),
    }
}

fn single_detail(message: &str) -> Map<String, Value> {
    let mut map = Map::new();
    map.insert("message".to_owned(), Value::String(message.to_owned()));
    map
}
