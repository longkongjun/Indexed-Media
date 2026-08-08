#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_routes::router_with_registry;
use mediaflow_core::connectors::downloader::connection_service::DownloaderRegistry;
use mediaflow_core::connectors::downloader::model::{
    DownloaderCapabilities, LoadedDownloaderCredentials,
};
use mediaflow_core::connectors::downloader::port::{
    AddDownloadRequest, DownloadSource, DownloadSourceError, DownloaderEndpoint,
    RemoteDownloadQuery, RemoteDownloadRef, RemoteDownloadSnapshot,
};
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "downloader-config-session-token";
const CSRF: &str = "downloader-config-csrf-token";
const ORIGIN: &str = "http://127.0.0.1:3000";
const USERNAME: &str = "downloader-user-must-not-leak";
const PASSWORD: &str = "downloader-password-must-not-leak";

struct FakeDownloadSource {
    product: &'static str,
    api: &'static str,
    probes: AtomicUsize,
}

#[async_trait]
impl DownloadSource for FakeDownloadSource {
    async fn probe(
        &self,
        endpoint: DownloaderEndpoint<'_>,
    ) -> Result<DownloaderCapabilities, DownloadSourceError> {
        assert_credentials(endpoint.credentials);
        self.probes.fetch_add(1, Ordering::SeqCst);
        if endpoint.base_url.contains("offline") {
            return Err(DownloadSourceError::Unavailable);
        }
        Ok(DownloaderCapabilities {
            manual_add: true,
            task_monitoring: true,
            product_version: self.product.to_owned(),
            api_version: self.api.to_owned(),
        })
    }

    async fn add(
        &self,
        _endpoint: DownloaderEndpoint<'_>,
        _request: AddDownloadRequest<'_>,
    ) -> Result<RemoteDownloadRef, DownloadSourceError> {
        Err(DownloadSourceError::InvalidResponse)
    }

    async fn fetch(
        &self,
        _endpoint: DownloaderEndpoint<'_>,
        _query: RemoteDownloadQuery,
    ) -> Result<Vec<RemoteDownloadSnapshot>, DownloadSourceError> {
        Err(DownloadSourceError::InvalidResponse)
    }
}

fn assert_credentials(credentials: &LoadedDownloaderCredentials) {
    assert_eq!(credentials.username().expose(), USERNAME.as_bytes());
    assert_eq!(credentials.password().expose(), PASSWORD.as_bytes());
}

#[tokio::test]
async fn candidate_test_is_guarded_redacted_and_never_persisted() {
    let (fixture, db, app, cookie, qbit, _transmission) = authenticated_app().await;
    let unauthenticated = app
        .clone()
        .oneshot(
            Request::get("/api/v1/downloader-connections")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let missing_csrf = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/downloader-connections/connection-tests",
            Some(&cookie),
            None,
            Some(ORIGIN),
            None,
            candidate("qbittorrent", "https://download.test/qbit"),
        ))
        .await
        .unwrap();
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let tested = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/downloader-connections/connection-tests",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            candidate("qbittorrent", "https://download.test/qbit"),
        ))
        .await
        .unwrap();
    assert_eq!(tested.status(), StatusCode::OK);
    let tested = body_text(tested).await;
    assert!(!tested.contains(USERNAME));
    assert!(!tested.contains(PASSWORD));
    let tested: Value = serde_json::from_str(&tested).unwrap();
    assert_eq!(tested["reachable"], true);
    assert_eq!(tested["health"], "healthy");
    assert_eq!(tested["capabilities"]["api_version"], "2.11.4");
    assert_eq!(qbit.probes.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM downloader_connections")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    let audit = audit_text(db.pool()).await;
    assert!(audit.contains("downloader.connection-test"));
    assert!(!audit.contains(USERNAME));
    assert!(!audit.contains(PASSWORD));

    let composed = build_router(fixture.config().clone(), Some(db));
    let composed_list = composed
        .oneshot(authenticated_get(
            "/api/v1/downloader-connections?limit=20",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(composed_list.status(), StatusCode::OK);
}

#[tokio::test]
async fn create_list_get_replace_and_delete_keep_connections_isolated_and_secret_free() {
    let (_fixture, db, app, cookie, qbit, transmission) = authenticated_app().await;
    let created = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/downloader-connections",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            candidate("qbittorrent", "https://download.test/qbit/"),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_text = body_text(created).await;
    assert!(!created_text.contains(USERNAME));
    assert!(!created_text.contains(PASSWORD));
    let created: Value = serde_json::from_str(&created_text).unwrap();
    let first_id = created["id"].as_str().unwrap();
    assert_eq!(created["config_version"], 1);
    assert_eq!(created["health"], "healthy");
    assert_eq!(created["base_url"], "https://download.test/qbit");
    assert_eq!(created["checked_at"].as_str().unwrap().len(), 27);
    assert!(created.get("checked_at_us").is_none());
    assert!(created.get("updated_at_us").is_none());

    let offline = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/downloader-connections",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            candidate("transmission", "https://offline.test/transmission"),
        ))
        .await
        .unwrap();
    assert_eq!(offline.status(), StatusCode::CREATED);
    let offline = json_body(offline).await;
    let second_id = offline["id"].as_str().unwrap();
    assert_eq!(offline["health"], "unavailable");
    assert_eq!(offline["failure_code"], "integration.unavailable");
    assert_eq!(offline["capabilities"], Value::Null);
    assert_eq!(qbit.probes.load(Ordering::SeqCst), 1);
    assert_eq!(transmission.probes.load(Ordering::SeqCst), 1);

    let list = app
        .clone()
        .oneshot(authenticated_get(
            "/api/v1/downloader-connections?limit=1",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list = json_body(list).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert!(list["next_cursor"].is_string());

    for id in [first_id, second_id] {
        let read = app
            .clone()
            .oneshot(authenticated_get(
                &format!("/api/v1/downloader-connections/{id}"),
                &cookie,
            ))
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
        let text = body_text(read).await;
        assert!(!text.contains(USERNAME));
        assert!(!text.contains(PASSWORD));
    }

    let stale = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/downloader-connections/{first_id}"),
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("0"),
            candidate("qbittorrent", "https://download.test/qbit-next"),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "request.conflict");

    let replaced = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/downloader-connections/{first_id}"),
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("1"),
            candidate("qbittorrent", "https://download.test/qbit-next/"),
        ))
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced = json_body(replaced).await;
    assert_eq!(replaced["config_version"], 2);
    assert_eq!(replaced["base_url"], "https://download.test/qbit-next");
    assert_eq!(qbit.probes.load(Ordering::SeqCst), 2);

    let stale_delete = app
        .clone()
        .oneshot(empty_mutation(
            Method::DELETE,
            &format!("/api/v1/downloader-connections/{first_id}"),
            &cookie,
            Some("1"),
        ))
        .await
        .unwrap();
    assert_eq!(stale_delete.status(), StatusCode::CONFLICT);
    let deleted = app
        .clone()
        .oneshot(empty_mutation(
            Method::DELETE,
            &format!("/api/v1/downloader-connections/{first_id}"),
            &cookie,
            Some("2"),
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let missing = app
        .clone()
        .oneshot(authenticated_get(
            &format!("/api/v1/downloader-connections/{first_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let stored = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
        "SELECT secret_nonce,secret_ciphertext FROM downloader_connections WHERE id=?",
    )
    .bind(Uuid::parse_str(second_id).unwrap().as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(stored.0.len(), 24);
    assert!(
        !stored
            .1
            .windows(PASSWORD.len())
            .any(|window| window == PASSWORD.as_bytes())
    );
    let audit = audit_text(db.pool()).await;
    assert!(audit.contains("downloader.connection-create"));
    assert!(audit.contains("downloader.connection-replace"));
    assert!(audit.contains("downloader.connection-delete"));
    assert!(!audit.contains(USERNAME));
    assert!(!audit.contains(PASSWORD));
}

#[tokio::test]
async fn write_guards_and_strict_json_precede_all_connection_side_effects() {
    let (_fixture, db, app, cookie, qbit, transmission) = authenticated_app().await;
    let requests = [
        json_request(
            Method::POST,
            "/api/v1/downloader-connections",
            None,
            None,
            None,
            None,
            candidate("qbittorrent", "https://download.test"),
        ),
        json_request(
            Method::POST,
            "/api/v1/downloader-connections",
            Some(&cookie),
            Some(CSRF),
            Some("https://evil.test"),
            None,
            candidate("qbittorrent", "https://download.test"),
        ),
        json_request(
            Method::POST,
            "/api/v1/downloader-connections",
            Some(&cookie),
            Some("bad-csrf"),
            Some(ORIGIN),
            None,
            candidate("qbittorrent", "https://download.test"),
        ),
        json_request(
            Method::POST,
            "/api/v1/downloader-connections",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            json!({
                "kind":"qbittorrent","display_name":"qBit",
                "base_url":"https://download.test","username":USERNAME,
                "password":PASSWORD,"enabled":true,"secret_echo":true
            }),
        ),
    ];
    let expected = [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
        StatusCode::UNPROCESSABLE_ENTITY,
    ];
    for (request, status) in requests.into_iter().zip(expected) {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        let body = body_text(response).await;
        assert!(!body.contains(USERNAME));
        assert!(!body.contains(PASSWORD));
    }
    assert_eq!(qbit.probes.load(Ordering::SeqCst), 0);
    assert_eq!(transmission.probes.load(Ordering::SeqCst), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM downloader_connections")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

async fn authenticated_app() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    Arc<FakeDownloadSource>,
    Arc<FakeDownloadSource>,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','not-used',0,0)",
    )
    .bind(account.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO identity_sessions
         (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,
          credential_version,created_at_us,last_used_at_us,revoked_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,NULL)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account.as_bytes().as_slice())
    .bind(random::sha256(COOKIE_TOKEN.as_bytes()).as_slice())
    .bind(random::sha256(CSRF.as_bytes()).as_slice())
    .bind(i64::MAX - 1)
    .bind(i64::MAX)
    .bind(1_i64)
    .bind(0_i64)
    .bind(0_i64)
    .execute(db.pool())
    .await
    .unwrap();
    let qbit = Arc::new(FakeDownloadSource {
        product: "5.1.2",
        api: "2.11.4",
        probes: AtomicUsize::new(0),
    });
    let transmission = Arc::new(FakeDownloadSource {
        product: "4.2.0",
        api: "6.1.0",
        probes: AtomicUsize::new(0),
    });
    let registry = DownloaderRegistry::new(qbit.clone(), transmission.clone());
    let app = router_with_registry(fixture.config().clone(), &db, registry);
    (
        fixture,
        db,
        app,
        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        qbit,
        transmission,
    )
}

fn candidate(kind: &str, base_url: &str) -> Value {
    json!({
        "kind":kind,
        "display_name":if kind == "qbittorrent" { "Primary qBit" } else { "Transmission" },
        "base_url":base_url,
        "username":USERNAME,
        "password":PASSWORD,
        "enabled":true
    })
}

fn authenticated_get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn json_request(
    method: Method,
    path: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
    if_match: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("sec-fetch-site", "same-origin");
    if let Some(value) = cookie {
        builder = builder.header(header::COOKIE, value);
    }
    if let Some(value) = csrf {
        builder = builder.header("x-csrf-token", value);
    }
    if let Some(value) = origin {
        builder = builder.header(header::ORIGIN, value);
    }
    if let Some(value) = if_match {
        builder = builder.header(header::IF_MATCH, value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn empty_mutation(
    method: Method,
    path: &str,
    cookie: &str,
    if_match: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin");
    if let Some(value) = if_match {
        builder = builder.header(header::IF_MATCH, value);
    }
    builder.body(Body::empty()).unwrap()
}

async fn audit_text(pool: &sqlx::SqlitePool) -> String {
    sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(GROUP_CONCAT(action || outcome || COALESCE(subject_id,'') || safe_details_json), '')
         FROM platform_audit_events",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
