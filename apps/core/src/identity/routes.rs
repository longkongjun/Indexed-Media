use crate::bootstrap::config::AppConfig;
use crate::identity::IdentityUseCases;
use crate::identity::model::{
    BootstrapCommand, BootstrapResponse, BootstrapStatusResponse, LoginCommand, LoginRequest,
    SessionResponse,
};
use crate::identity::service::IdentityService;
use crate::platform::db::Db;
use crate::platform::request_security::{
    CsrfGuard, SessionGuard, source_key, validate_same_origin,
};
use crate::shared::error::{AppError, ErrorCode};
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Extension, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone)]
struct IdentityHttpState {
    config: AppConfig,
    db: Db,
    service: IdentityService,
}

impl IdentityHttpState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            ));
        }
        match tokio::time::timeout(
            std::time::Duration::from_millis(250),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
        )
        .await
        {
            Ok(Ok(1)) => Ok(()),
            _ => Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            )),
        }
    }
}

/// 构建系统引导和管理员会话 HTTP 路由。
///
/// 路由器强制执行启动就绪状态、来源/CSRF 规则及安全会话 Cookie，并将凭据/会话状态变更委托给
/// [`IdentityService`]。
pub fn router(config: AppConfig, db: &Db) -> Router {
    let service = IdentityService::new(db.pool().clone(), config.config_dir.clone());
    Router::new()
        .route(
            "/api/v1/family-members",
            get(list_family_members).post(create_family_member),
        )
        .route(
            "/api/v1/family-members/{memberId}",
            axum::routing::delete(delete_family_member),
        )
        .route("/api/v1/system/bootstrap-status", get(bootstrap_status))
        .route("/api/v1/system/bootstrap", post(bootstrap))
        .route("/api/v1/sessions", post(create_session))
        .route("/api/v1/session", get(get_session).delete(delete_session))
        .route("/api/v1/device-pairing-codes", post(create_pairing_code))
        .route("/api/v1/devices/enrollment", post(enroll_device))
        .route("/api/v1/devices", get(list_devices))
        .route(
            "/api/v1/devices/{deviceId}",
            axum::routing::delete(delete_device),
        )
        .route(
            "/api/v1/devices/current/token-rotation",
            post(rotate_device_token),
        )
        .with_state(Arc::new(IdentityHttpState {
            config,
            db: db.clone(),
            service,
        }))
}
async fn bootstrap_status(
    State(state): State<Arc<IdentityHttpState>>,
) -> Result<Json<BootstrapStatusResponse>, AppError> {
    state.ensure_ready().await?;
    Ok(Json(BootstrapStatusResponse {
        requires_initialization: state.service.requires_initialization().await?,
        version: "v1",
    }))
}
async fn bootstrap(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<BootstrapCommand>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    state.ensure_ready().await?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.service.audit_denial("origin").await?;
        return Err(error);
    }
    let Json(command) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let account = state.service.bootstrap(command).await?;
    Ok((
        StatusCode::CREATED,
        Json(BootstrapResponse {
            account,
            version: "v1",
        }),
    ))
}
async fn create_session(
    State(state): State<Arc<IdentityHttpState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    payload: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, AppError> {
    state.ensure_ready().await?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.service.audit_denial("origin").await?;
        return Err(error);
    }
    let Json(request) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let session = state
        .service
        .create_session(LoginCommand {
            administrator_name: request.administrator_name,
            password: request.password,
            source_key: source_key(
                &headers,
                peer.map(|value| value.0.0),
                &state.config.trusted_proxy_cidrs,
            ),
        })
        .await?;
    let mut response = (
        StatusCode::CREATED,
        Json(SessionResponse {
            account: session.account,
            csrf_token: session.csrf_token,
            version: "v1",
        }),
    )
        .into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&session_cookie(&session.raw_token))
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
    );
    Ok(response)
}
async fn get_session(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<Json<SessionResponse>, AppError> {
    state.ensure_ready().await?;
    let session = SessionGuard::authenticate(&headers, &state.service).await?;
    let session_id = web_session_id(&session)?;
    let csrf_token = state.service.rotate_csrf(session_id).await?;
    Ok(Json(SessionResponse {
        account: session.account,
        csrf_token,
        version: "v1",
    }))
}
async fn delete_session(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    state.ensure_ready().await?;
    let session = SessionGuard::authenticate_without_sliding(&headers, &state.service).await?;
    let session_id = web_session_id(&session)?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.service.audit_denial("origin").await?;
        return Err(error);
    }
    if let Err(error) = CsrfGuard::validate(&headers, &session) {
        state.service.audit_denial("csrf").await?;
        return Err(error);
    }
    state.service.revoke_session(session_id).await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "__Host-mediaflow_session=; Max-Age=0; Secure; HttpOnly; SameSite=Strict; Path=/",
        ),
    );
    Ok(response)
}
fn session_cookie(token: &str) -> String {
    format!("__Host-mediaflow_session={token}; Secure; HttpOnly; SameSite=Strict; Path=/")
}

/// 提取浏览器会话通道的行标识符；设备令牌主体不适用于会话端点（登出走设备端点）。
fn web_session_id(
    session: &crate::identity::model::AuthenticatedSession,
) -> Result<uuid::Uuid, AppError> {
    match session.principal {
        crate::identity::model::Principal::Session { session_id } => Ok(session_id),
        crate::identity::model::Principal::Device { .. } => Err(AppError::new(
            ErrorCode::SessionExpired,
            "device tokens do not manage browser sessions",
        )),
    }
}

async fn guard_admin(
    state: &IdentityHttpState,
    headers: &HeaderMap,
) -> Result<crate::identity::model::AuthenticatedSession, AppError> {
    state.ensure_ready().await?;
    SessionGuard::authenticate(headers, &state.service).await
}

/// 管理面守卫：仅浏览器会话通道（配对码签发/设备列表/设备管理为 Web 表面，设备令牌
/// 不得自延续——被盗设备令牌不能签发新配对码或吊销其他设备）。
async fn guard_web_admin(
    state: &IdentityHttpState,
    headers: &HeaderMap,
) -> Result<crate::identity::model::AuthenticatedSession, AppError> {
    let session = guard_admin(state, headers).await?;
    web_session_id(&session)?;
    Ok(session)
}

/// 家庭成员只有身份记录；管理端对他们完全关闭（`MVP-WEB-003`）。
async fn list_family_members(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    guard_web_admin(&state, &headers).await?;
    let members = state.service.list_family_members().await?;
    Ok(Json(serde_json::json!({ "items": members })))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
struct FamilyMemberInput {
    display_name: String,
    password: String,
}

async fn create_family_member(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
    payload: Result<Json<FamilyMemberInput>, JsonRejection>,
) -> Result<Json<serde_json::Value>, AppError> {
    guard_web_admin(&state, &headers).await?;
    validate_same_origin(&headers, &state.config.public_origin)?;
    let Json(input) = payload.map_err(|_| {
        AppError::new(
            ErrorCode::ValidationFailed,
            "family member payload is invalid",
        )
    })?;
    if input.password.len() < 8 || input.password.len() > 512 {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "password length is invalid",
        ));
    }
    let member = state
        .service
        .create_family_member(&input.display_name, &input.password)
        .await?;
    Ok(Json(serde_json::json!({ "member": member })))
}

async fn delete_family_member(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
    axum::extract::Path(member_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Response, AppError> {
    guard_web_admin(&state, &headers).await?;
    validate_same_origin(&headers, &state.config.public_origin)?;
    state.service.delete_family_member(member_id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// 签发一次性配对码（10 分钟有效）：Web 管理面展示给移动端输入/扫码。
async fn create_pairing_code(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    state.ensure_ready().await?;
    let session = guard_web_admin(&state, &headers).await?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.service.audit_denial("origin").await?;
        return Err(error);
    }
    if let Err(error) = CsrfGuard::validate(&headers, &session) {
        state.service.audit_denial("csrf").await?;
        return Err(error);
    }
    let (code, expires_at_us) = state.service.issue_pairing_code(session.account.id).await?;
    Ok((
        StatusCode::CREATED,
        Json(crate::identity::model::PairingCodeResponse {
            pairing_code: code,
            expires_at: micros_to_rfc3339(expires_at_us),
            version: "v1",
        }),
    ))
}

/// 设备 enrollment：移动端入口端点，**不做同源校验**（非浏览器客户端无 Fetch Metadata
/// 语义，与登录端点同为预认证表面）。门禁二选一：一次性配对码，或管理员凭据（走
/// 登录限流）。成功返回设备令牌原文（仅此一次）。
async fn enroll_device(
    State(state): State<Arc<IdentityHttpState>>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    payload: Result<Json<crate::identity::model::DeviceEnrollmentRequest>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    state.ensure_ready().await?;
    let Json(request) =
        payload.map_err(|_| AppError::new(ErrorCode::ValidationFailed, "invalid JSON request"))?;
    let source = source_key(
        &headers,
        peer.map(|value| value.0.0),
        &state.config.trusted_proxy_cidrs,
    );
    let enrolled = if request.pairing_code.is_some() && request.administrator_name.is_some() {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "provide either a pairing code or credentials, not both",
        ));
    } else if let Some(code) = request.pairing_code.as_deref() {
        state
            .service
            .enroll_with_pairing_code(code, &request.device_name, &source)
            .await?
    } else if let (Some(name), Some(password)) = (
        request.administrator_name.as_deref(),
        request.password.as_deref(),
    ) {
        state
            .service
            .enroll_with_credentials(name, password, &request.device_name, &source)
            .await?
    } else {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "a pairing code or administrator credentials are required",
        ));
    };
    Ok((
        StatusCode::CREATED,
        Json(crate::identity::model::DeviceEnrollmentResponse {
            device: enrolled.device,
            token: enrolled.raw_token,
            version: "v1",
        }),
    ))
}

/// 设备列表：Web 管理面（仅浏览器会话通道）。
async fn list_devices(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    state.ensure_ready().await?;
    guard_web_admin(&state, &headers).await?;
    let devices = state.service.list_devices().await?;
    Ok(Json(serde_json::json!({ "items": devices })))
}

/// 吊销设备：管理员（Web）可吊销任意设备；设备令牌只能吊销自己（移动端登出）。
async fn delete_device(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
    axum::extract::Path(device_id): axum::extract::Path<uuid::Uuid>,
) -> Result<Response, AppError> {
    state.ensure_ready().await?;
    let session = SessionGuard::authenticate_without_sliding(&headers, &state.service).await?;
    match session.principal {
        crate::identity::model::Principal::Session { .. } => {
            if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
                state.service.audit_denial("origin").await?;
                return Err(error);
            }
            if let Err(error) = CsrfGuard::validate(&headers, &session) {
                state.service.audit_denial("csrf").await?;
                return Err(error);
            }
            // 未知设备对管理员返回 404；幂等撤销只保留给重复点击同一台设备。
            state.service.device(device_id).await?;
            state
                .service
                .revoke_device(device_id, "administrator")
                .await?;
        }
        crate::identity::model::Principal::Device { device_id: self_id } => {
            if self_id != device_id {
                return Err(AppError::new(
                    ErrorCode::PermissionDenied,
                    "devices may only revoke their own token",
                ));
            }
            state.service.revoke_device(device_id, "logout").await?;
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// 设备令牌自轮换：旧令牌立即失效，新令牌仅此一次返回。
async fn rotate_device_token(
    State(state): State<Arc<IdentityHttpState>>,
    headers: HeaderMap,
) -> Result<Json<crate::identity::model::DeviceTokenRotationResponse>, AppError> {
    state.ensure_ready().await?;
    let session = SessionGuard::authenticate_without_sliding(&headers, &state.service).await?;
    let device_id = match session.principal {
        crate::identity::model::Principal::Device { device_id } => device_id,
        crate::identity::model::Principal::Session { .. } => {
            return Err(AppError::new(
                ErrorCode::PermissionDenied,
                "token rotation requires a device token",
            ));
        }
    };
    let (token, expires_at_us) = state.service.rotate_device_token(device_id).await?;
    Ok(Json(crate::identity::model::DeviceTokenRotationResponse {
        token,
        expires_at: micros_to_rfc3339(expires_at_us),
        version: "v1",
    }))
}

fn micros_to_rfc3339(micros: i64) -> String {
    chrono::TimeZone::timestamp_micros(&chrono::Utc, micros)
        .single()
        .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .unwrap_or_default()
}
