#![allow(clippy::needless_pass_by_value)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::rejection::JsonRejection;
use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Datelike as _;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization as _;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::connectors::model::{
    MetadataCandidate, ProviderError, ProviderMediaKind, TmdbSearchRequest,
};
use crate::connectors::service::{ConnectorService, MetadataProvider, TmdbConnectionTester};
use crate::connectors::tmdb::{TmdbCachePolicy, TmdbClient, TmdbProvider};
use crate::identification::decision::DecisionLevel;
use crate::identification::manual::model::{ManualDecisionInput, ManualIdentityHint};
use crate::identification::manual::store::ManualDecisionStore;
use crate::identification::model::MediaKind;
use crate::identification::review::{ReviewCaseFilter, ReviewCaseStore};
use crate::identification::store::IdentificationStore;
use crate::identity::model::AuthenticatedSession;
use crate::identity::service::IdentityService;
use crate::platform::audit;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::PageRequest;
use crate::tasks::processing::model::{
    ProcessingStage, ProcessingStatus, ProcessingTaskFilter, TaskCenterView,
};
use crate::tasks::processing::service::ProcessingTaskService;

#[derive(Clone)]
struct IdentificationHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    processing: ProcessingTaskService,
    identification: IdentificationStore,
    reviews: ReviewCaseStore,
    manual: ManualDecisionStore,
    connectors: ConnectorService,
    provider: Arc<dyn MetadataProvider>,
}

impl IdentificationHttpState {
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

    async fn guard_get(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await
    }

    async fn guard_post(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        self.ensure_ready().await?;
        let session = SessionGuard::authenticate_without_sliding(headers, &self.identity).await?;
        if let Err(error) = validate_same_origin(headers, &self.config.public_origin) {
            self.identity.audit_denial("origin").await?;
            return Err(error);
        }
        if let Err(error) = CsrfGuard::validate(headers, &session) {
            self.identity.audit_denial("csrf").await?;
            return Err(error);
        }
        Ok(session)
    }
}

/// 构建已认证处理控制及只读识别/`ReviewCase` 路由。
pub fn router_with_notifier(config: AppConfig, db: &Db, notifier: OutboxNotifier) -> Router {
    let (provider, tester): (Arc<dyn MetadataProvider>, Arc<dyn TmdbConnectionTester>) =
        match TmdbClient::production() {
            Ok(client) => (
                Arc::new(TmdbProvider::new(
                    db.pool().clone(),
                    client.clone(),
                    TmdbCachePolicy::default(),
                )),
                Arc::new(client),
            ),
            Err(_) => (Arc::new(UnavailableProvider), Arc::new(UnavailableProvider)),
        };
    router_with_dyn_services(config, db, notifier, provider, tester)
}

/// 使用注入的有界提供方构建识别路由，供 HTTP 契约测试使用。
pub fn router_with_services(
    config: AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
    provider: Arc<dyn MetadataProvider>,
) -> Router {
    router_with_dyn_services(
        config,
        db,
        notifier,
        provider,
        Arc::new(UnavailableProvider),
    )
}

fn router_with_dyn_services(
    config: AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
    provider: Arc<dyn MetadataProvider>,
    tester: Arc<dyn TmdbConnectionTester>,
) -> Router {
    let connectors = ConnectorService::new_with_notifier(
        db.pool().clone(),
        &config.config_dir,
        tester,
        notifier.clone(),
    );
    Router::new()
        .route("/api/v1/processing-tasks", get(list_processing_tasks))
        .route(
            "/api/v1/processing-tasks/{processingTaskId}",
            get(get_processing_task),
        )
        .route(
            "/api/v1/processing-tasks/{processingTaskId}/identification",
            get(get_identification),
        )
        .route(
            "/api/v1/processing-tasks/{processingTaskId}/attempts",
            post(retry_processing_task),
        )
        .route(
            "/api/v1/processing-tasks/{processingTaskId}/cancel",
            post(cancel_processing_task),
        )
        .route("/api/v1/review-cases", get(list_review_cases))
        .route("/api/v1/review-cases/{reviewCaseId}", get(get_review_case))
        .route(
            "/api/v1/review-cases/{reviewCaseId}/decisions",
            post(submit_review_decision),
        )
        .route(
            "/api/v1/review-cases/{reviewCaseId}/candidates",
            get(search_review_candidates),
        )
        .with_state(Arc::new(IdentificationHttpState {
            config: config.clone(),
            db: db.clone(),
            identity: IdentityService::new(db.pool().clone(), config.config_dir),
            processing: ProcessingTaskService::new_with_notifier(
                db.pool().clone(),
                notifier.clone(),
            ),
            identification: IdentificationStore::new_with_notifier(db.pool().clone(), notifier),
            reviews: ReviewCaseStore::new(db.pool().clone()),
            manual: ManualDecisionStore::new(db.pool().clone()),
            connectors,
            provider,
        }))
}

async fn list_processing_tasks(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let (filter, page) = processing_center_query(&uri)?;
    Ok(Json(
        state
            .processing
            .list_center(session.account.id, &filter, &page)
            .await?,
    ))
}

async fn get_processing_task(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    Ok(Json(
        state
            .processing
            .get(session.account.id, processing_task_id(&uri, None)?)
            .await?,
    ))
}

async fn get_identification(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    Ok(Json(
        state
            .identification
            .detail(
                session.account.id,
                processing_task_id(&uri, Some("identification"))?,
            )
            .await?,
    ))
}

async fn retry_processing_task(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_post(&headers).await?;
    let task_id = processing_task_id(&uri, Some("attempts"))?;
    let task = state
        .processing
        .retry(
            session.account.id,
            task_id,
            idempotency_key(&headers)?,
            chrono::Utc::now().timestamp_micros(),
        )
        .await?;
    audit::record(
        state.db.pool(),
        "processing.retry",
        "success",
        Some(&task_id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

async fn cancel_processing_task(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_post(&headers).await?;
    let task_id = processing_task_id(&uri, Some("cancel"))?;
    let task = state
        .processing
        .cancel(
            session.account.id,
            task_id,
            idempotency_key(&headers)?,
            chrono::Utc::now().timestamp_micros(),
        )
        .await?;
    audit::record(
        state.db.pool(),
        "processing.cancel",
        "success",
        Some(&task_id.to_string()),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(task)))
}

async fn list_review_cases(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let (filter, page) = review_query(&uri)?;
    Ok(Json(
        state
            .reviews
            .list_active(session.account.id, &filter, &page)
            .await?,
    ))
}

async fn get_review_case(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let case_id = resource_id(&uri, "/api/v1/review-cases/", None, "review case")?;
    Ok(Json(
        state
            .reviews
            .get_active(session.account.id, case_id)
            .await?,
    ))
}

async fn submit_review_decision(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    payload: Result<Json<ReviewDecisionRequest>, JsonRejection>,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_post(&headers).await?;
    let case_id = resource_id(
        &uri,
        "/api/v1/review-cases/",
        Some("decisions"),
        "review case",
    )?;
    let expected_version = if_match(&headers)?;
    let key = decision_idempotency_key(&headers)?;
    let Json(request) = payload.map_err(|_| validation("invalid review decision JSON"))?;
    let accepted = state
        .manual
        .accept(
            session.account.id,
            case_id,
            expected_version,
            key,
            &request.into_manual(),
            chrono::Utc::now().timestamp_micros(),
        )
        .await?;
    Ok((StatusCode::ACCEPTED, Json(accepted)))
}

async fn search_review_candidates(
    State(state): State<Arc<IdentificationHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let case_id = resource_id(
        &uri,
        "/api/v1/review-cases/",
        Some("candidates"),
        "review case",
    )?;
    state
        .reviews
        .get_active(session.account.id, case_id)
        .await?;
    let request = candidate_query(&uri)?;
    let (token, _, region) = state.connectors.load_tmdb_credential().await?;
    let mut candidates = state
        .provider
        .search(
            &token,
            &TmdbSearchRequest {
                media_kind: request.media_type.provider_kind(),
                title: request.query,
                year: None,
                locale: request.locale.clone(),
                region,
            },
        )
        .await
        .map_err(provider_error)?;
    candidates.truncate(request.limit);
    let items = candidates
        .into_iter()
        .map(|candidate| review_candidate(candidate, request.media_type, &request.locale))
        .collect::<Result<Vec<_>, AppError>>()?;
    Ok(Json(ReviewCandidatePage { items }))
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ReviewMediaType {
    Movie,
    Tv,
}

impl ReviewMediaType {
    const fn media_kind(self) -> MediaKind {
        match self {
            Self::Movie => MediaKind::Movie,
            Self::Tv => MediaKind::Episode,
        }
    }

    const fn provider_kind(self) -> ProviderMediaKind {
        match self {
            Self::Movie => ProviderMediaKind::Movie,
            Self::Tv => ProviderMediaKind::Tv,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Tv => "tv",
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum TmdbProviderName {
    Tmdb,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum ReviewDecisionRequest {
    SelectProviderCandidate {
        provider: TmdbProviderName,
        media_type: ReviewMediaType,
        provider_id: String,
        save_feedback: bool,
    },
    RematchWithHints {
        media_type: ReviewMediaType,
        title: String,
        year: Option<u16>,
        season: Option<u16>,
        episodes: Vec<u16>,
        save_feedback: bool,
    },
    SelectGenericVideo {
        display_title: String,
        group_hint: Option<String>,
        save_grouping_feedback: bool,
    },
}

impl ReviewDecisionRequest {
    fn into_manual(self) -> ManualDecisionInput {
        match self {
            Self::SelectProviderCandidate {
                provider,
                media_type,
                provider_id,
                save_feedback,
            } => {
                let TmdbProviderName::Tmdb = provider;
                ManualDecisionInput::SelectProviderCandidate {
                    media_kind: media_type.media_kind(),
                    provider_id,
                    save_feedback,
                }
            }
            Self::RematchWithHints {
                media_type,
                title,
                year,
                season,
                episodes,
                save_feedback,
            } => ManualDecisionInput::RematchWithHints {
                hint: ManualIdentityHint {
                    media_kind: media_type.media_kind(),
                    normalized_title: title,
                    year,
                    season,
                    episodes,
                },
                save_feedback,
            },
            Self::SelectGenericVideo {
                display_title,
                group_hint,
                save_grouping_feedback,
            } => ManualDecisionInput::SelectGenericVideo {
                display_title,
                group_hint,
                save_grouping_feedback,
            },
        }
    }
}

struct CandidateSearch {
    query: String,
    media_type: ReviewMediaType,
    locale: String,
    limit: usize,
}

#[derive(Serialize)]
struct ReviewCandidatePage {
    items: Vec<ReviewCandidate>,
}

#[derive(Serialize)]
struct ReviewCandidate {
    provider: &'static str,
    media_type: &'static str,
    provider_id: String,
    title: String,
    original_title: Option<String>,
    year: Option<u16>,
    locale: String,
}

fn candidate_query(uri: &Uri) -> Result<CandidateSearch, AppError> {
    let mut query_value = None;
    let mut media_type = None;
    let mut locale = None;
    let mut limit = None;
    for (key, value) in query(uri) {
        match key.as_ref() {
            "q" if query_value.is_none() => query_value = Some(normalize_query(&value)?),
            "media_type" if media_type.is_none() => {
                media_type = Some(match value.as_ref() {
                    "movie" => ReviewMediaType::Movie,
                    "tv" => ReviewMediaType::Tv,
                    _ => return Err(validation("invalid candidate media type")),
                });
            }
            "locale" if locale.is_none() && valid_locale(&value) => {
                locale = Some(value.into_owned());
            }
            "limit" if limit.is_none() => {
                let value = value
                    .parse::<usize>()
                    .map_err(|_| validation("invalid candidate limit"))?;
                if !(1..=20).contains(&value) {
                    return Err(validation("invalid candidate limit"));
                }
                limit = Some(value);
            }
            _ => return Err(validation("invalid candidate query")),
        }
    }
    Ok(CandidateSearch {
        query: query_value.ok_or_else(|| validation("candidate query missing"))?,
        media_type: media_type.ok_or_else(|| validation("candidate media type missing"))?,
        locale: locale.ok_or_else(|| validation("candidate locale missing"))?,
        limit: limit.unwrap_or(20),
    })
}

fn normalize_query(value: &str) -> Result<String, AppError> {
    let normalized = value
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty()
        || normalized.chars().count() > 200
        || normalized.chars().any(char::is_control)
    {
        return Err(validation("candidate query is outside bounds"));
    }
    Ok(normalized)
}

fn review_candidate(
    candidate: MetadataCandidate,
    requested_type: ReviewMediaType,
    requested_locale: &str,
) -> Result<ReviewCandidate, AppError> {
    if candidate.identity.provider_id <= 0
        || candidate.identity.media_kind != requested_type.provider_kind()
        || candidate.titles.is_empty()
    {
        return Err(provider_error(ProviderError::InvalidResponse));
    }
    let title = &candidate.titles[0];
    let original_title = candidate
        .titles
        .iter()
        .find(|field| field.source == crate::connectors::model::FieldLanguageSource::Original)
        .map(|field| field.value.clone());
    let year = candidate
        .release_dates
        .first()
        .and_then(|field| u16::try_from(field.value.year()).ok());
    if title.value.is_empty()
        || title.value.chars().count() > 512
        || original_title
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.chars().count() > 512)
        || year.is_some_and(|value| !(1870..=9999).contains(&value))
    {
        return Err(provider_error(ProviderError::InvalidResponse));
    }
    Ok(ReviewCandidate {
        provider: "tmdb",
        media_type: requested_type.as_str(),
        provider_id: candidate.identity.provider_id.to_string(),
        title: title.value.clone(),
        original_title,
        year,
        locale: requested_locale.to_owned(),
    })
}

fn valid_locale(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2] == b'-'
        && bytes[3..].iter().all(u8::is_ascii_uppercase)
}

fn if_match(headers: &HeaderMap) -> Result<i64, AppError> {
    let value = headers
        .get(header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("If-Match missing"))?;
    if value.is_empty() || value.len() > 19 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(validation("If-Match is invalid"));
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| validation("If-Match is invalid"))
}

fn decision_idempotency_key(headers: &HeaderMap) -> Result<&str, AppError> {
    let key = idempotency_key(headers)?;
    if !(8..=128).contains(&key.len()) || key.chars().any(char::is_control) {
        return Err(validation("idempotency key is outside bounds"));
    }
    Ok(key)
}

fn provider_error(error: ProviderError) -> AppError {
    let code = match error {
        ProviderError::CredentialsInvalid => ErrorCode::IntegrationUnauthorized,
        ProviderError::RateLimited { .. } => ErrorCode::IntegrationRateLimited,
        ProviderError::NotConfigured
        | ProviderError::TemporarilyUnavailable { .. }
        | ProviderError::Timeout
        | ProviderError::ResponseTooLarge
        | ProviderError::InvalidResponse => ErrorCode::ProviderUnavailable,
    };
    AppError::new(code, "candidate provider request failed")
}

struct UnavailableProvider;

#[async_trait]
impl TmdbConnectionTester for UnavailableProvider {
    async fn test(
        &self,
        _token: &crate::platform::secrets::SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::TemporarilyUnavailable {
            retry_at_us: chrono::Utc::now().timestamp_micros() + 60_000_000,
        })
    }
}

#[async_trait]
impl MetadataProvider for UnavailableProvider {
    async fn find_external(
        &self,
        _token: &crate::platform::secrets::SecretBytes,
        _request: &crate::connectors::model::TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Err(unavailable())
    }

    async fn search(
        &self,
        _token: &crate::platform::secrets::SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Err(unavailable())
    }

    async fn details(
        &self,
        _token: &crate::platform::secrets::SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Err(unavailable())
    }

    async fn verify_episodes(
        &self,
        _token: &crate::platform::secrets::SecretBytes,
        _series_id: i64,
        _episodes: &[(u16, u16)],
        _locale: &str,
    ) -> Result<Vec<crate::connectors::model::EpisodeIdentity>, ProviderError> {
        Err(unavailable())
    }
}

fn unavailable() -> ProviderError {
    ProviderError::TemporarilyUnavailable {
        retry_at_us: chrono::Utc::now().timestamp_micros() + 60_000_000,
    }
}

fn processing_task_id(uri: &Uri, suffix: Option<&str>) -> Result<Uuid, AppError> {
    resource_id(uri, "/api/v1/processing-tasks/", suffix, "processing task")
}

fn resource_id(
    uri: &Uri,
    prefix: &str,
    suffix: Option<&str>,
    kind: &str,
) -> Result<Uuid, AppError> {
    let value = uri.path().strip_prefix(prefix).and_then(|value| {
        suffix.map_or_else(
            || (!value.is_empty() && !value.contains('/')).then_some(value),
            |suffix| value.strip_suffix(&format!("/{suffix}")),
        )
    });
    value
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| validation(format!("invalid {kind} ID")))
        .and_then(|value| {
            Uuid::parse_str(value).map_err(|_| validation(format!("invalid {kind} ID")))
        })
}

fn idempotency_key(headers: &HeaderMap) -> Result<&str, AppError> {
    headers
        .get("idempotency-key")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| validation("idempotency key missing"))
}

fn processing_center_query(uri: &Uri) -> Result<(ProcessingTaskFilter, PageRequest), AppError> {
    let mut cursor = None;
    let mut limit = None;
    let mut view = None;
    let mut stage = None;
    let mut status = None;
    let mut inbox = None;
    let mut query_value = None;
    for (key, value) in query(uri) {
        match key.as_ref() {
            "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
            "limit" if limit.is_none() => limit = Some(parse_limit(&value)?),
            "view" if view.is_none() => {
                view = Some(match value.as_ref() {
                    "pending" => TaskCenterView::Pending,
                    "running" => TaskCenterView::Running,
                    "all" => TaskCenterView::All,
                    "completed" => TaskCenterView::Completed,
                    _ => return Err(validation("invalid processing task view")),
                });
            }
            "stage" if stage.is_none() => stage = Some(parse_processing_stage(&value)?),
            "status" if status.is_none() => status = Some(parse_processing_status(&value)?),
            "inbox_directory_id" if inbox.is_none() => {
                inbox = Some(Uuid::parse_str(&value).map_err(|_| validation("invalid inbox"))?);
            }
            "q" if query_value.is_none() => {
                query_value = Some(normalize_task_query(&value)?);
            }
            _ => return Err(validation("invalid processing task query")),
        }
    }
    Ok((
        ProcessingTaskFilter {
            view: view.unwrap_or_default(),
            stage,
            status,
            inbox_directory_id: inbox,
            query: query_value,
        },
        PageRequest::new(cursor, limit).map_err(|error| validation(error.to_string()))?,
    ))
}

fn normalize_task_query(value: &str) -> Result<String, AppError> {
    let normalized = value.nfkc().collect::<String>().trim().to_owned();
    if normalized.is_empty()
        || normalized.chars().count() > 200
        || normalized.chars().any(char::is_control)
    {
        return Err(validation("processing task query is outside bounds"));
    }
    Ok(normalized)
}

fn parse_processing_stage(value: &str) -> Result<ProcessingStage, AppError> {
    match value {
        "identification" => Ok(ProcessingStage::Identification),
        "planning" => Ok(ProcessingStage::Planning),
        "file-operation" => Ok(ProcessingStage::FileOperation),
        "nfo" => Ok(ProcessingStage::Nfo),
        "completion" => Ok(ProcessingStage::Completion),
        _ => Err(validation("invalid processing stage")),
    }
}

fn parse_processing_status(value: &str) -> Result<ProcessingStatus, AppError> {
    match value {
        "queued" => Ok(ProcessingStatus::Queued),
        "running" => Ok(ProcessingStatus::Running),
        "waiting-confirmation" => Ok(ProcessingStatus::WaitingConfirmation),
        "paused" => Ok(ProcessingStatus::Paused),
        "cancelled" => Ok(ProcessingStatus::Cancelled),
        "partial-success" => Ok(ProcessingStatus::PartialSuccess),
        "completed" => Ok(ProcessingStatus::Completed),
        "failed" => Ok(ProcessingStatus::Failed),
        _ => Err(validation("invalid processing status")),
    }
}

fn review_query(uri: &Uri) -> Result<(ReviewCaseFilter, PageRequest), AppError> {
    let mut cursor = None;
    let mut limit = None;
    let mut level = None;
    let mut inbox = None;
    let mut updated_before_us = None;
    for (key, value) in query(uri) {
        match key.as_ref() {
            "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
            "limit" if limit.is_none() => limit = Some(parse_limit(&value)?),
            "decision_level" if level.is_none() => level = Some(parse_level(&value)?),
            "inbox_directory_id" if inbox.is_none() => {
                inbox = Some(Uuid::parse_str(&value).map_err(|_| validation("invalid inbox"))?);
            }
            "updated_before" if updated_before_us.is_none() => {
                updated_before_us = Some(
                    chrono::DateTime::parse_from_rfc3339(&value)
                        .map_err(|_| validation("invalid updated_before"))?
                        .timestamp_micros(),
                );
            }
            _ => return Err(validation("invalid review query")),
        }
    }
    Ok((
        ReviewCaseFilter {
            level,
            inbox_directory_id: inbox,
            updated_before_us,
        },
        PageRequest::new(cursor, limit).map_err(|error| validation(error.to_string()))?,
    ))
}

fn query(uri: &Uri) -> url::form_urlencoded::Parse<'_> {
    url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
}

fn parse_limit(value: &str) -> Result<u32, AppError> {
    value.parse().map_err(|_| validation("invalid page limit"))
}

fn parse_level(value: &str) -> Result<DecisionLevel, AppError> {
    match value {
        "probable" => Ok(DecisionLevel::Probable),
        "ambiguous" => Ok(DecisionLevel::Ambiguous),
        "unidentified" => Ok(DecisionLevel::Unidentified),
        _ => Err(validation("invalid review decision level")),
    }
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}
