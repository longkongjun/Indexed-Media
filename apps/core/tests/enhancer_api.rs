#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::enhancer::model::{EnhancementInput, EnhancerConfigInput};
use mediaflow_core::connectors::enhancer::port::{
    EnhancementHints, EnhancerEndpoint, EnhancerError, EnhancerProbe, IdentificationEnhancer,
};
use mediaflow_core::connectors::enhancer::routes::router_with_adapter;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "enhancer-config-session-token";
const CSRF: &str = "enhancer-config-csrf-token";
const ORIGIN: &str = "http://127.0.0.1:3000";

#[derive(Default)]
struct FakeEnhancer {
    probes: AtomicUsize,
}

#[async_trait]
impl IdentificationEnhancer for FakeEnhancer {
    async fn probe(&self, endpoint: EnhancerEndpoint<'_>) -> Result<EnhancerProbe, EnhancerError> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        if endpoint.model == "timeout:test" {
            return Err(EnhancerError::Timeout);
        }
        Ok(EnhancerProbe {
            adapter_version: "ollama-v1".to_owned(),
            model_available: endpoint.model != "missing:latest",
        })
    }

    async fn enhance(
        &self,
        _endpoint: EnhancerEndpoint<'_>,
        _input: &EnhancementInput,
    ) -> Result<EnhancementHints, EnhancerError> {
        Err(EnhancerError::Unavailable)
    }
}

#[tokio::test]
async fn default_get_candidate_test_and_versioned_put_are_guarded_and_redacted() {
    let (fixture, db, app, cookie, adapter) = authenticated_app().await;
    let default = app
        .clone()
        .oneshot(authenticated_get(
            "/api/v1/identification-enhancer",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(default.status(), StatusCode::OK);
    let default = json_body(default).await;
    assert_eq!(default["kind"], "ollama");
    assert_eq!(default["enabled"], false);
    assert_eq!(default["config_version"], 1);
    assert_eq!(default["fallback_code"], "automation.source-disabled");

    let candidate_test = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/identification-enhancer/connection-tests",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            candidate(true, "http://127.0.0.1:11434", "missing:latest"),
        ))
        .await
        .unwrap();
    assert_eq!(candidate_test.status(), StatusCode::OK);
    let candidate_result = json_body(candidate_test).await;
    assert_eq!(candidate_result["reachable"], true);
    assert_eq!(candidate_result["model_available"], false);
    assert_eq!(
        candidate_result["fallback_code"],
        "integration.not-configured"
    );
    assert_eq!(adapter.probes.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT config_version FROM identification_enhancer WHERE singleton_key=1"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );

    let timeout = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/identification-enhancer/connection-tests",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            candidate(true, "http://127.0.0.1:11434", "timeout:test"),
        ))
        .await
        .unwrap();
    assert_eq!(timeout.status(), StatusCode::OK);
    let timeout = json_body(timeout).await;
    assert_eq!(timeout["reachable"], false);
    assert_eq!(timeout["health"], "unavailable");
    assert_eq!(timeout["fallback_code"], "provider.timeout");

    let saved = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("1"),
            candidate(true, "http://localhost:11434", "qwen3:4b"),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved_text = body_text(saved).await;
    assert!(!saved_text.contains("prompt"));
    assert!(!saved_text.contains("response"));
    let saved: Value = serde_json::from_str(&saved_text).unwrap();
    assert_eq!(saved["config_version"], 2);
    assert_eq!(saved["health"], "healthy");
    assert_eq!(adapter.probes.load(Ordering::SeqCst), 3);

    let disabled = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("2"),
            candidate(false, "http://127.0.0.1:11434", "qwen3:4b"),
        ))
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::OK);
    let disabled = json_body(disabled).await;
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["fallback_code"], "automation.source-disabled");
    assert_eq!(adapter.probes.load(Ordering::SeqCst), 3);

    let stale = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("1"),
            candidate(false, "http://127.0.0.1:11434", "qwen3:4b"),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let composed = build_router(fixture.config().clone(), Some(db.clone()));
    assert_eq!(
        composed
            .oneshot(authenticated_get(
                "/api/v1/identification-enhancer",
                &cookie
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let audit = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(GROUP_CONCAT(action || safe_details_json), '') FROM platform_audit_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(audit.contains("identification-enhancer.connection-test"));
    assert!(audit.contains("identification-enhancer.config-save"));
    assert!(!audit.contains("qwen3:4b"));
    assert!(!audit.contains("localhost"));
}

#[tokio::test]
async fn session_origin_csrf_if_match_strict_json_and_local_endpoint_checks_precede_writes() {
    let (_fixture, db, app, cookie, adapter) = authenticated_app().await;
    let valid = candidate(true, "http://127.0.0.1:11434", "qwen3:4b");
    let requests = [
        json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            None,
            None,
            None,
            Some("1"),
            valid.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some(CSRF),
            Some("https://evil.test"),
            Some("1"),
            valid.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some("bad"),
            Some(ORIGIN),
            Some("1"),
            valid.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("bad"),
            valid.clone(),
        ),
        json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("1"),
            json!({"enabled":true,"base_url":"http://127.0.0.1:11434","model":"qwen3:4b","timeout_ms":3000,"prompt":"leak"}),
        ),
        json_request(
            Method::PUT,
            "/api/v1/identification-enhancer",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("1"),
            candidate(true, "https://models.example.com", "qwen3:4b"),
        ),
    ];
    let expected = [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::FORBIDDEN,
        StatusCode::UNPROCESSABLE_ENTITY,
        StatusCode::UNPROCESSABLE_ENTITY,
        StatusCode::UNPROCESSABLE_ENTITY,
    ];
    for (request, expected) in requests.into_iter().zip(expected) {
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            expected
        );
    }
    assert_eq!(adapter.probes.load(Ordering::SeqCst), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT config_version FROM identification_enhancer WHERE singleton_key=1"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
}

async fn authenticated_app() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    Arc<FakeEnhancer>,
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
    let adapter = Arc::new(FakeEnhancer::default());
    let app = router_with_adapter(fixture.config().clone(), &db, adapter.clone());
    (fixture, db, app, cookie(), adapter)
}

fn candidate(enabled: bool, base_url: &str, model: &str) -> Value {
    json!({"enabled":enabled,"base_url":base_url,"model":model,"timeout_ms":3000})
}

fn authenticated_get(uri: &str, cookie: &str) -> Request<Body> {
    Request::get(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn json_request(
    method: Method,
    uri: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
    version: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    if let Some(csrf) = csrf {
        builder = builder.header("x-csrf-token", csrf);
    }
    if let Some(origin) = origin {
        builder = builder
            .header(header::ORIGIN, origin)
            .header("sec-fetch-site", "same-origin");
    }
    if let Some(version) = version {
        builder = builder.header(header::IF_MATCH, version);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn cookie() -> String {
    format!("__Host-mediaflow_session={COOKIE_TOKEN}")
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn _assert_input(_: EnhancerConfigInput) {}
