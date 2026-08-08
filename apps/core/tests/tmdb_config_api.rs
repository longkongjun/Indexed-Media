#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::ProviderError;
use mediaflow_core::connectors::routes::router_with_tester;
use mediaflow_core::connectors::service::ConnectorService;
use mediaflow_core::connectors::service::TmdbConnectionTester;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use mediaflow_core::platform::secrets::SecretBytes;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const TOKEN: &str = "candidate-token-that-must-never-leak";
const COOKIE_TOKEN: &str = "tmdb-config-session-token";
const CSRF: &str = "tmdb-config-csrf-token";
const ORIGIN: &str = "http://127.0.0.1:3000";

#[tokio::test]
async fn connection_test_never_persists_candidate_and_all_responses_are_redacted() {
    let (fixture, db, app, cookie, tester) = authenticated_app().await;
    let composed = build_router(fixture.config().clone(), Some(db.clone()));
    let initial = composed
        .oneshot(get("/api/v1/integrations/tmdb", &cookie))
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    assert_eq!(json_body(initial).await["configured"], false);
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/integrations/tmdb/connection-tests",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            json!({"api_read_access_token":TOKEN,"locale":"zh-CN","region":"CN"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["health"],
        "healthy"
    );
    assert!(!body.contains(TOKEN));
    assert!(!body.contains("token"));
    assert_eq!(tester.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM connectors_integrations")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    let audit = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(GROUP_CONCAT(action || outcome || COALESCE(subject_id,'') || safe_details_json), '') FROM platform_audit_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!audit.contains(TOKEN));
    assert!(
        !fixture
            .config()
            .config_dir
            .join("instance.key")
            .to_string_lossy()
            .contains(TOKEN)
    );
}

#[tokio::test]
async fn put_get_replace_conflict_and_delete_keep_plaintext_out_of_storage_and_api() {
    let (fixture, db, app, cookie, tester) = authenticated_app().await;
    let create = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("0"),
            json!({"api_read_access_token":TOKEN,"locale":"zh-CN","region":"CN"}),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let created = body_text(create).await;
    assert!(!created.contains(TOKEN));
    assert_eq!(
        serde_json::from_str::<Value>(&created).unwrap()["config_version"],
        1
    );

    let stored: (Vec<u8>, Vec<u8>, i64) = sqlx::query_as(
        "SELECT secret_nonce,secret_ciphertext,config_version FROM connectors_integrations WHERE kind='tmdb'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(stored.0.len(), 24);
    assert!(
        !stored
            .1
            .windows(TOKEN.len())
            .any(|window| window == TOKEN.as_bytes())
    );
    assert_eq!(stored.2, 1);
    let service = ConnectorService::new(db.pool().clone(), &fixture.config().config_dir, tester);
    let (decrypted, locale, region) = service.load_tmdb_credential().await.unwrap();
    assert_eq!(decrypted.expose(), TOKEN.as_bytes());
    assert_eq!(locale, "zh-CN");
    assert_eq!(region.as_deref(), Some("CN"));

    let stale = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("0"),
            json!({"api_read_access_token":"replacement-token-must-not-win","locale":"en-US","region":null}),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "request.conflict");

    let read = app
        .clone()
        .oneshot(get("/api/v1/integrations/tmdb", &cookie))
        .await
        .unwrap();
    let read = body_text(read).await;
    assert!(!read.to_lowercase().contains("token"));
    assert!(!read.contains(TOKEN));
    assert_eq!(
        serde_json::from_str::<Value>(&read).unwrap()["locale"],
        "zh-CN"
    );

    let replacement_token = "replacement-token-that-must-not-leak";
    let replace = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("1"),
            json!({"api_read_access_token":replacement_token,"locale":"en-US","region":null}),
        ))
        .await
        .unwrap();
    assert_eq!(replace.status(), StatusCode::OK);
    let replaced = body_text(replace).await;
    assert!(!replaced.contains(replacement_token));
    assert_eq!(
        serde_json::from_str::<Value>(&replaced).unwrap()["config_version"],
        2
    );
    let new_ciphertext = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT secret_ciphertext FROM connectors_integrations WHERE kind='tmdb'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_ne!(new_ciphertext, stored.1);
    let (replacement, locale, region) = service.load_tmdb_credential().await.unwrap();
    assert_eq!(replacement.expose(), replacement_token.as_bytes());
    assert_eq!(locale, "en-US");
    assert_eq!(region, None);

    let delete = app
        .clone()
        .oneshot(empty_mutation(
            Method::DELETE,
            "/api/v1/integrations/tmdb",
            &cookie,
            CSRF,
            ORIGIN,
            Some("2"),
        ))
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);
    let after = app
        .oneshot(get("/api/v1/integrations/tmdb", &cookie))
        .await
        .unwrap();
    let after = json_body(after).await;
    assert_eq!(after["configured"], false);
    assert_eq!(after["config_version"], 3);
    let cleared: (Option<Vec<u8>>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT secret_nonce,secret_ciphertext FROM connectors_integrations WHERE kind='tmdb'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(cleared, (None, None));
    assert_eq!(
        service.load_tmdb_credential().await.unwrap_err().code(),
        mediaflow_core::shared::error::ErrorCode::IntegrationNotConfigured
    );
}

#[tokio::test]
async fn session_origin_csrf_if_match_and_json_guards_precede_all_config_writes() {
    let (_fixture, db, app, cookie, _tester) = authenticated_app().await;
    let body = json!({"api_read_access_token":TOKEN,"locale":"zh-CN","region":"CN"});
    let requests = [
        json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            None,
            None,
            None,
            Some("0"),
            body.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some(CSRF),
            Some("https://evil.test"),
            Some("0"),
            body.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some("bad-csrf"),
            Some(ORIGIN),
            Some("0"),
            body.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("not-a-version"),
            body.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("0"),
            json!({"api_read_access_token":TOKEN,"locale":"zh-CN","region":"CN","secret_echo":true}),
        ),
    ];
    let expected = [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
        StatusCode::UNPROCESSABLE_ENTITY,
        StatusCode::UNPROCESSABLE_ENTITY,
    ];
    for (request, expected) in requests.into_iter().zip(expected) {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected);
        assert!(!body_text(response).await.contains(TOKEN));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM connectors_integrations")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[test]
fn request_debug_never_contains_candidate_credentials() {
    let command: mediaflow_core::connectors::model::TmdbConfigCommand = serde_json::from_value(
        json!({"api_read_access_token":TOKEN,"locale":"zh-CN","region":"CN"}),
    )
    .unwrap();
    let debug = format!("{command:?}");
    assert!(!debug.contains(TOKEN));
    assert!(debug.contains("[REDACTED]"));
}

#[tokio::test]
async fn provider_failures_are_stable_redacted_and_never_persist_candidate_tokens() {
    let (fixture, db, _app, cookie, _tester) = authenticated_app().await;
    for (failure, status, code) in [
        (
            ProviderError::CredentialsInvalid,
            StatusCode::UNAUTHORIZED,
            "integration.unauthorized",
        ),
        (
            ProviderError::RateLimited { retry_at_us: 1 },
            StatusCode::TOO_MANY_REQUESTS,
            "integration.rate_limited",
        ),
        (
            ProviderError::TemporarilyUnavailable { retry_at_us: 1 },
            StatusCode::SERVICE_UNAVAILABLE,
            "provider.unavailable",
        ),
    ] {
        let app = router_with_tester(
            fixture.config().clone(),
            &db,
            Arc::new(FailingTester(failure)),
        );
        let response = app
            .oneshot(json_request(
                Method::POST,
                "/api/v1/integrations/tmdb/connection-tests",
                Some(&cookie),
                Some(CSRF),
                Some(ORIGIN),
                None,
                json!({"api_read_access_token":TOKEN,"locale":"zh-CN","region":"CN"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        let body = body_text(response).await;
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()["error"]["code"],
            code
        );
        assert!(!body.contains(TOKEN));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM connectors_integrations")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn running_service_pins_its_key_and_restart_fails_closed_after_key_replacement() {
    let (fixture, db, app, cookie, tester) = authenticated_app().await;
    let response = app
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/integrations/tmdb",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("0"),
            json!({"api_read_access_token":TOKEN,"locale":"zh-CN","region":"CN"}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let pinned = ConnectorService::new(
        db.pool().clone(),
        &fixture.config().config_dir,
        tester.clone(),
    );
    let key_path = fixture.config().config_dir.join("instance.key");
    std::fs::rename(
        &key_path,
        fixture.config().config_dir.join("instance.key.previous"),
    )
    .unwrap();
    std::fs::write(&key_path, [9_u8; 32]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    assert_eq!(
        pinned.load_tmdb_credential().await.unwrap().0.expose(),
        TOKEN.as_bytes()
    );
    let restarted = ConnectorService::new(db.pool().clone(), &fixture.config().config_dir, tester);
    let error = restarted.load_tmdb_credential().await.unwrap_err();
    assert_eq!(
        error.code(),
        mediaflow_core::shared::error::ErrorCode::Internal
    );
    assert!(!format!("{error:?}").contains(TOKEN));
}

struct HealthyTester {
    calls: AtomicUsize,
}

struct FailingTester(ProviderError);

#[async_trait]
impl TmdbConnectionTester for FailingTester {
    async fn test(
        &self,
        _token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Err(self.0)
    }
}

#[async_trait]
impl TmdbConnectionTester for HealthyTester {
    async fn test(
        &self,
        token: &SecretBytes,
        locale: &str,
        region: Option<&str>,
    ) -> Result<(), ProviderError> {
        assert_eq!(token.expose(), TOKEN.as_bytes());
        assert_eq!(locale, "zh-CN");
        assert_eq!(region, Some("CN"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

async fn authenticated_app() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    Arc<HealthyTester>,
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
    let tester = Arc::new(HealthyTester {
        calls: AtomicUsize::new(0),
    });
    let app = router_with_tester(fixture.config().clone(), &db, tester.clone());
    (
        fixture,
        db,
        app,
        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        tester,
    )
}

fn get(path: &str, cookie: &str) -> Request<Body> {
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
    csrf: &str,
    origin: &str,
    if_match: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(header::ORIGIN, origin)
        .header("sec-fetch-site", "same-origin");
    if let Some(value) = if_match {
        builder = builder.header(header::IF_MATCH, value);
    }
    builder.body(Body::empty()).unwrap()
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
