use std::sync::Arc;

use axum::extract::{OriginalUri, State};
use axum::http::{HeaderMap, Uri};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use unicode_normalization::UnicodeNormalization as _;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::catalog::model::{LocalStatus, MediaItemFilter, MediaItemKind};
use crate::catalog::store::CatalogStore;
use crate::identity::model::AuthenticatedSession;
use crate::identity::service::IdentityService;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::request_security::SessionGuard;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::PageRequest;

#[derive(Clone)]
struct CatalogHttpState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    catalog: CatalogStore,
    artwork: Option<crate::catalog::artwork::ArtworkService>,
}

impl CatalogHttpState {
    async fn guard_get(&self, headers: &HeaderMap) -> Result<AuthenticatedSession, AppError> {
        if !self.config.readiness_issues().is_empty()
            || !matches!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(250),
                    sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
                )
                .await,
                Ok(Ok(1))
            )
        {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            ));
        }
        SessionGuard::authenticate(headers, &self.identity).await
    }
}

/// 组装目录列表与详情路由，并复用给定 outbox 通知器传播事务事件。
pub fn router_with_notifier(config: AppConfig, db: &Db, notifier: OutboxNotifier) -> Router {
    router_with_artwork_client(
        config,
        db,
        notifier,
        crate::connectors::tmdb::TmdbClient::production().ok(),
    )
}

/// 复用正式路由并显式注入固定生产或loopback客户端，供隔离提供方联调。
pub fn router_with_artwork_client(
    config: AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
    client: Option<crate::connectors::tmdb::TmdbClient>,
) -> Router {
    Router::new()
        .route("/api/v1/media-libraries", get(list_media_libraries))
        .route("/api/v1/media-items", get(list_media_items))
        .route("/api/v1/media-items/{mediaItemId}", get(get_media_item))
        .route(
            "/api/v1/media-items/{mediaItemId}/artwork",
            get(get_artwork),
        )
        .with_state(Arc::new(CatalogHttpState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            catalog: CatalogStore::new_with_notifier(db.pool().clone(), notifier),
            artwork: client.map(|client| {
                crate::catalog::artwork::ArtworkService::new(
                    db.pool().clone(),
                    config.config_dir.clone(),
                    client,
                )
            }),
            config,
            db: db.clone(),
        }))
}

async fn get_artwork(
    State(state): State<Arc<CatalogHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    use sha2::Digest as _;
    let session = state.guard_get(&headers).await?;
    let value = uri
        .path()
        .strip_prefix("/api/v1/media-items/")
        .and_then(|value| value.strip_suffix("/artwork"))
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| validation("invalid media item ID"))?;
    let media_id = Uuid::parse_str(value).map_err(|_| validation("invalid media item ID"))?;
    let image = state
        .artwork
        .as_ref()
        .ok_or_else(|| AppError::new(ErrorCode::ProviderUnavailable, "poster client unavailable"))?
        .poster(session.account.id, media_id)
        .await?;
    let etag = format!("\"{}\"", hex::encode(sha2::Sha256::digest(&image.bytes)));
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                image.content_type.to_owned(),
            ),
            (
                axum::http::header::CACHE_CONTROL,
                "private, max-age=3600".to_owned(),
            ),
            (axum::http::header::VARY, "Authorization, Cookie".to_owned()),
            (axum::http::header::ETAG, etag),
        ],
        image.bytes,
    ))
}

async fn list_media_libraries(
    State(state): State<Arc<CatalogHttpState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    Ok(Json(state.catalog.libraries(session.account.id).await?))
}

async fn list_media_items(
    State(state): State<Arc<CatalogHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let (filter, page) = list_query(&uri)?;
    Ok(Json(
        state
            .catalog
            .list(session.account.id, &filter, &page)
            .await?,
    ))
}

async fn get_media_item(
    State(state): State<Arc<CatalogHttpState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Result<impl IntoResponse, AppError> {
    let session = state.guard_get(&headers).await?;
    let value = uri
        .path()
        .strip_prefix("/api/v1/media-items/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| validation("invalid media item ID"))?;
    let id = Uuid::parse_str(value).map_err(|_| validation("invalid media item ID"))?;
    Ok(Json(state.catalog.detail(session.account.id, id).await?))
}

fn list_query(uri: &Uri) -> Result<(MediaItemFilter, PageRequest), AppError> {
    let mut cursor = None;
    let mut limit = None;
    let mut kind = None;
    let mut library_id = None;
    let mut local_status = None;
    let mut query_value = None;
    for (key, value) in url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes()) {
        match key.as_ref() {
            "cursor" if cursor.is_none() => cursor = Some(value.into_owned()),
            "limit" if limit.is_none() => {
                limit = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| validation("invalid catalog page limit"))?,
                );
            }
            "type" if kind.is_none() => {
                kind = Some(match value.as_ref() {
                    "movie" => MediaItemKind::Movie,
                    "series" => MediaItemKind::Series,
                    "generic-video" => MediaItemKind::GenericVideo,
                    _ => return Err(validation("invalid media item type")),
                });
            }
            "library_id" if library_id.is_none() => {
                library_id =
                    Some(Uuid::parse_str(&value).map_err(|_| validation("invalid library ID"))?);
            }
            "local_status" if local_status.is_none() => {
                local_status = Some(match value.as_ref() {
                    "complete" => LocalStatus::Complete,
                    "partial" => LocalStatus::Partial,
                    _ => return Err(validation("invalid local status")),
                });
            }
            "q" if query_value.is_none() => {
                let normalized = value.nfkc().collect::<String>().trim().to_owned();
                if normalized.is_empty()
                    || normalized.chars().count() > 200
                    || normalized.chars().any(char::is_control)
                {
                    return Err(validation("catalog query is outside bounds"));
                }
                query_value = Some(normalized);
            }
            _ => return Err(validation("invalid catalog query")),
        }
    }
    Ok((
        MediaItemFilter {
            kind,
            library_id,
            local_status,
            query: query_value,
        },
        PageRequest::new(cursor, limit).map_err(|error| validation(error.to_string()))?,
    ))
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}
