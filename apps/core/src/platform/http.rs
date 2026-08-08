use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, Uri, header};
use axum::middleware::{Next, from_fn};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::automation::event_routes::router_with_notifier as automation_event_router;
use crate::automation::routes::router as automation_router;
use crate::automation::webhook::router_with_notifier as automation_webhook_router;
use crate::bootstrap::config::AppConfig;
use crate::catalog::routes::router_with_notifier as catalog_router;
use crate::connectors::downloader::connection_routes::router as downloader_connection_router;
use crate::connectors::downloader::task_routes::router_with_notifier as download_task_router;
use crate::connectors::enhancer::routes::router as enhancer_router;
use crate::connectors::routes::router_with_notifier as connector_router;
use crate::discovery::capability::DeploymentRootSet;
use crate::discovery::routes::router as discovery_router;
use crate::discovery::service::DiscoveryService;
use crate::identification::routes::router_with_notifier as identification_router;
use crate::identity::routes::router as identity_router;
use crate::organization::fs::OrganizationFs;
use crate::organization::target_routes::router as organization_target_router;
use crate::organization::target_service::OrganizationTargetService;
use crate::organization::task_routes::{
    OrganizationTaskService, router as organization_task_router,
};
use crate::platform::capability_fs::OsCapabilityFs;
use crate::platform::db::Db;
use crate::platform::outbox::OutboxNotifier;
use crate::platform::sse::{SseOptions, router_with_notifier as sse_router};
use crate::platform::static_web::static_or_spa_response;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::routes::router_with_notifier as scan_router;

#[derive(Clone)]
struct HttpState {
    config: AppConfig,
    db: Option<Db>,
    startup_issues: Arc<[&'static str]>,
    discovery: Option<DiscoveryService>,
}

impl HttpState {
    async fn is_ready(&self) -> bool {
        if !self.startup_issues.is_empty() {
            return false;
        }
        let Some(db) = &self.db else {
            return false;
        };
        if self
            .discovery
            .as_ref()
            .is_none_or(|service| !service.configuration_is_current())
        {
            return false;
        }
        matches!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(db.pool()),
            )
            .await,
            Ok(Ok(1))
        )
    }
}

#[derive(Serialize)]
struct HealthBody<'a> {
    status: &'a str,
}

/// 使用进程本地 outbox 通知器构建顶层路由器。
///
/// 当 `db` 缺失或启动验证失败时，存活性仍然可用，但业务路由保持未挂载或受就绪状态门控。
/// 每个响应都会获得 Core 安全响应头。
pub fn build_router(config: AppConfig, db: Option<Db>) -> Router {
    build_router_with_outbox(config, db, OutboxNotifier::new())
}

/// 使用 `notifier` 为事务性事件唤醒构建顶层路由器。
///
/// 调用方提供的通知器让任务变更和 SSE 订阅者共享同一唤醒通道。与 [`build_router`] 一样，仅当 `db` 存在时
/// 才挂载依赖数据库的路由；部署根目录验证失败会使发现和任务路由不可用。
pub fn build_router_with_outbox(
    config: AppConfig,
    db: Option<Db>,
    notifier: OutboxNotifier,
) -> Router {
    let mut startup_issues = config.readiness_issues();
    let services = db.as_ref().and_then(|db| {
        let initialized = DeploymentRootSet::load(&config.deployment_roots_file, config.mode)
            .and_then(|roots| {
                OsCapabilityFs::open(roots.declarations(), config.mode).map(|fs| {
                    let views = roots.view_map();
                    let fingerprint = roots.fingerprint();
                    let root_access = roots
                        .views()
                        .iter()
                        .map(|root| (root.id.clone(), root.access))
                        .collect();
                    let fs = Arc::new(fs);
                    let capability_fs: Arc<dyn crate::discovery::capability::CapabilityFs> =
                        fs.clone();
                    let organization_fs: Arc<dyn OrganizationFs> = fs;
                    let discovery = DiscoveryService::new(
                        views.clone(),
                        Arc::clone(&capability_fs),
                        db.pool().clone(),
                    )
                    .with_config_guard(config.deployment_roots_file.clone(), fingerprint.clone());
                    let organization = OrganizationTargetService::new_with_notifier(
                        views,
                        Arc::clone(&capability_fs),
                        db.pool().clone(),
                        notifier.clone(),
                    )
                    .with_config_guard(config.deployment_roots_file.clone(), fingerprint.clone());
                    let tasks = OrganizationTaskService::production(
                        db.pool().clone(),
                        notifier.clone(),
                        organization_fs,
                        root_access,
                    )
                    .with_config_guard(config.deployment_roots_file.clone(), fingerprint);
                    (discovery, organization, tasks)
                })
            });
        if let Ok(services) = initialized {
            Some(services)
        } else {
            if !startup_issues.contains(&"deployment roots are unavailable") {
                startup_issues.push("deployment roots are unavailable");
            }
            None
        }
    });
    let discovery = services.as_ref().map(|(service, _, _)| service.clone());
    let organization = services.map(|(_, targets, tasks)| (targets, tasks));
    let startup_issues = Arc::from(startup_issues);
    let state = Arc::new(HttpState {
        config: config.clone(),
        db: db.clone(),
        startup_issues,
        discovery: discovery.clone(),
    });
    let mut router = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .fallback(fallback)
        .with_state(state);
    if let Some(db) = db {
        router = router.merge(identity_router(config.clone(), &db));
        router = merge_connector_routes(router, &config, &db, &notifier);
        router = router.merge(automation_router(config.clone(), &db));
        router = router.merge(automation_event_router(
            config.clone(),
            &db,
            notifier.clone(),
        ));
        router = router.merge(automation_webhook_router(
            config.clone(),
            &db,
            notifier.clone(),
        ));
        router = router.merge(catalog_router(config.clone(), &db, notifier.clone()));
        router = router.merge(identification_router(config.clone(), &db, notifier.clone()));
        router = router.merge(sse_router(
            config.clone(),
            &db,
            notifier.clone(),
            SseOptions::production(),
        ));
        if let Some((target_service, task_service)) = organization {
            router = router.merge(organization_target_router(
                config.clone(),
                &db,
                target_service,
            ));
            router = router.merge(organization_task_router(config.clone(), &db, task_service));
        }
        if let Some(service) = discovery {
            router = router.merge(discovery_router(config.clone(), &db, service.clone()));
            router = router.merge(scan_router(config, &db, service, notifier));
        }
    }
    router.layer(from_fn(security_headers))
}

fn merge_connector_routes(
    router: Router,
    config: &AppConfig,
    db: &Db,
    notifier: &OutboxNotifier,
) -> Router {
    router
        .merge(connector_router(config.clone(), db, notifier.clone()))
        .merge(downloader_connection_router(config.clone(), db))
        .merge(download_task_router(config.clone(), db, notifier.clone()))
        .merge(enhancer_router(config.clone(), db, notifier.clone()))
}

async fn security_headers(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    if !headers.contains_key(header::CACHE_CONTROL) {
        headers.insert(
            header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
    }
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        axum::http::HeaderValue::from_static("same-origin"),
    );
    response
}

async fn live() -> impl IntoResponse {
    (StatusCode::OK, Json(HealthBody { status: "live" }))
}

async fn ready(State(state): State<Arc<HttpState>>) -> Response {
    if state.is_ready().await {
        (StatusCode::OK, Json(HealthBody { status: "ready" })).into_response()
    } else {
        AppError::new(ErrorCode::NotReady, "startup safety checks have not passed").into_response()
    }
}

async fn fallback(State(state): State<Arc<HttpState>>, uri: Uri) -> Response {
    if !state.is_ready().await {
        return AppError::new(ErrorCode::NotReady, "business routes are closed").into_response();
    }

    let path = uri.path();
    if path == "/api" || path.starts_with("/api/") {
        return AppError::new(ErrorCode::NotFound, "API route does not exist").into_response();
    }

    match static_or_spa_response(&state.config.web_dist, path).await {
        Ok(Some(response)) => response,
        Ok(None) => {
            AppError::new(ErrorCode::NotFound, "static resource does not exist").into_response()
        }
        Err(error) => error.into_response(),
    }
}
