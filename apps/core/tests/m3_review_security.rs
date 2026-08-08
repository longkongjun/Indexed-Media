mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{
    EpisodeIdentity, MetadataCandidate, ProviderError, ProviderMediaKind, SecretString,
    TmdbConfigCommand, TmdbExternalIdRequest, TmdbSearchRequest,
};
use mediaflow_core::connectors::service::{
    ConnectorService, MetadataProvider, TmdbConnectionTester,
};
use mediaflow_core::identification::decision::{
    DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use mediaflow_core::identification::routes::router_with_services;
use mediaflow_core::identification::store::{IdentificationCommit, IdentificationStore};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::random;
use mediaflow_core::platform::secrets::SecretBytes;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "m3-review-security-session";
const CSRF: &str = "m3-review-security-csrf";
const ORIGIN: &str = "http://127.0.0.1:3000";
const TOKEN: &str = "never-return-this-server-token";

#[tokio::test]
async fn decision_mutation_guard_order_and_candidate_provider_errors_are_stable_and_redacted() {
    let provider = Arc::new(FailingProvider {
        error: ProviderError::Timeout,
        calls: AtomicUsize::new(0),
    });
    let harness = harness(provider.clone(), b"movies/security.mkv").await;
    let body = json!({
        "kind":"select-provider-candidate","provider":"tmdb","media_type":"movie",
        "provider_id":"1","save_feedback":false
    });

    let unauthenticated = harness
        .app
        .clone()
        .oneshot(write_request(harness.case_id, "", CSRF, ORIGIN, &body))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let bad_origin = harness
        .app
        .clone()
        .oneshot(write_request(
            harness.case_id,
            &harness.cookie,
            CSRF,
            "https://evil.test",
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
    let bad_csrf = harness
        .app
        .clone()
        .oneshot(write_request(
            harness.case_id,
            &harness.cookie,
            "wrong-csrf",
            ORIGIN,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(bad_csrf.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_task_decisions")
            .fetch_one(harness.db.pool())
            .await
            .unwrap(),
        0
    );

    let failed = harness
        .app
        .clone()
        .oneshot(search_request(harness.case_id, &harness.cookie))
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    let text = body_text(failed).await;
    assert!(text.contains("provider.unavailable"));
    assert!(!text.contains(TOKEN));
    assert!(!text.contains(&harness.config_path));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

    let invalid = harness
        .app
        .oneshot(
            Request::get(format!(
                "/api/v1/review-cases/{}/candidates?q=x&q=y&media_type=movie&locale=zh-CN",
                harness.case_id
            ))
            .header(header::COOKIE, &harness.cookie)
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_case_id_from_another_database_is_not_observable_and_never_calls_the_provider() {
    let left_provider = Arc::new(FailingProvider {
        error: ProviderError::RateLimited {
            retry_at_us: 200_000_000,
        },
        calls: AtomicUsize::new(0),
    });
    let left = harness(left_provider, b"movies/left.mkv").await;
    let right_provider = Arc::new(FailingProvider {
        error: ProviderError::RateLimited {
            retry_at_us: 200_000_000,
        },
        calls: AtomicUsize::new(0),
    });
    let right = harness(right_provider.clone(), b"movies/right.mkv").await;

    let limited = left
        .app
        .clone()
        .oneshot(search_request(left.case_id, &left.cookie))
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        body_text(limited)
            .await
            .contains("integration.rate_limited")
    );

    let response = right
        .app
        .oneshot(search_request(left.case_id, &right.cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(right_provider.calls.load(Ordering::SeqCst), 0);
}

struct Harness {
    _fixture: common::TestConfigDir,
    db: mediaflow_core::platform::db::Db,
    app: axum::Router,
    cookie: String,
    case_id: Uuid,
    config_path: String,
}

async fn harness(provider: Arc<dyn MetadataProvider>, path: &[u8]) -> Harness {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let config_path = fixture.config().config_dir.to_string_lossy().into_owned();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    seed_session(db.pool(), account).await;
    ConnectorService::new(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(AcceptTester),
    )
    .put_tmdb(
        TmdbConfigCommand {
            api_read_access_token: SecretString::new(TOKEN.to_owned()),
            locale: "zh-CN".to_owned(),
            region: None,
        },
        0,
    )
    .await
    .unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let lease =
        common::seed_processing_lease(db.pool(), inbox, path, path.to_vec(), "security").await;
    let store = IdentificationStore::new(db.pool().clone());
    let attempt = store
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    let case_id = store
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &[],
            candidates: &[],
            decision: &IdentificationDecisionDraft {
                level: DecisionLevel::Unidentified,
                selected_candidate: None,
                reasons: vec![DecisionReason::NoCandidate],
                retry_at_us: None,
                graph: None,
                rule_version: 1,
            },
            title_hint: Some("security"),
            now_us: 94_000_000,
        })
        .await
        .unwrap()
        .review_case_id
        .unwrap();
    let app = router_with_services(
        fixture.config().clone(),
        &db,
        OutboxNotifier::new(),
        provider,
    );
    Harness {
        _fixture: fixture,
        db,
        app,
        cookie: format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        case_id,
        config_path,
    }
}

async fn seed_session(pool: &sqlx::SqlitePool, account: Uuid) {
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
    .execute(pool)
    .await
    .unwrap();
}

fn write_request(
    case_id: Uuid,
    cookie: &str,
    csrf: &str,
    origin: &str,
    body: &Value,
) -> Request<Body> {
    Request::post(format!("/api/v1/review-cases/{case_id}/decisions"))
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(header::ORIGIN, origin)
        .header("sec-fetch-site", "same-origin")
        .header(header::IF_MATCH, "1")
        .header("idempotency-key", "security-decision")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn search_request(case_id: Uuid, cookie: &str) -> Request<Body> {
    Request::get(format!(
        "/api/v1/review-cases/{case_id}/candidates?q=security&media_type=movie&locale=zh-CN"
    ))
    .header(header::COOKIE, cookie)
    .body(Body::empty())
    .unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

struct AcceptTester;

#[async_trait]
impl TmdbConnectionTester for AcceptTester {
    async fn test(
        &self,
        _token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct FailingProvider {
    error: ProviderError,
    calls: AtomicUsize,
}

#[async_trait]
impl MetadataProvider for FailingProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Err(self.error)
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.error)
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Err(self.error)
    }

    async fn verify_episodes(
        &self,
        _token: &SecretBytes,
        _series_id: i64,
        _episodes: &[(u16, u16)],
        _locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        Err(self.error)
    }
}
