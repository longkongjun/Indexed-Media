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
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::capability::handlers::{self, CapabilityOutcome};
use crate::capability::model::{
    CapabilityCall, CapabilityError, CapabilityErrorCategory, CapabilityErrorPhase,
    CapabilityEvent, CapabilityEventType, CapabilityName, CapabilityResult, CapabilityResultStatus,
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
    clock: Arc<dyn TaskClock>,
}

/// 组装能力协议路由；依赖部署时注入的 capability 令牌。
pub fn router(config: AppConfig, db: &Db) -> Router {
    Router::new()
        .route("/api/capability/v1/calls", post(invoke_capability))
        .route("/api/capability/v1/events", get(list_events))
        .with_state(Arc::new(CapabilityHttpState {
            config,
            store: CapabilityStore::new(db.pool().clone()),
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
        CapabilityName::MediaCreatePlan => {
            handlers::plan_preview(state.store.pool(), &call.input).await
        }
        _other => {
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
            let error = app_error_to_capability(&error);
            let status = CapabilityResultStatus::Failed;
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
    if let (Some(task_id), Some(expected)) =
        (call.task_id, call.execution.expected_revision.as_deref())
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
        | ErrorCode::InboxOverlap => (Category::Precondition, false, Action::AskUser),
        ErrorCode::RootUnavailable
        | ErrorCode::BackupFailed
        | ErrorCode::ConfigInvalid
        | ErrorCode::DatabaseInvalid
        | ErrorCode::RestoreTargetNotEmpty
        | ErrorCode::NotReady
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
