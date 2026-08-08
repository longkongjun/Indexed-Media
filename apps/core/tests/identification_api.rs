#![allow(clippy::too_many_lines)]

mod common;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use chrono::NaiveDate;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{
    CacheStatus, CandidateIdentity, EpisodeIdentity, FieldLanguageSource, LocalizedField,
    MetadataCandidate, ProviderError, ProviderMediaKind, TmdbExternalIdRequest, TmdbSearchRequest,
};
use mediaflow_core::connectors::service::MetadataProvider;
use mediaflow_core::identification::parser::FilenameParser;
use mediaflow_core::identification::service::IdentificationService;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use mediaflow_core::platform::secrets::SecretBytes;
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "identification-api-session";
const CSRF: &str = "identification-api-csrf";
const ORIGIN: &str = "http://127.0.0.1:3000";
const SECRET: &str = "identification-api-secret-sentinel";

#[tokio::test]
async fn authenticated_processing_identification_and_review_reads_are_bounded_and_redacted() {
    let (fixture, db, app, cookie, task_id, case_id, inbox) = authenticated_case().await;

    let list = app
        .clone()
        .oneshot(get("/api/v1/processing-tasks?limit=1", &cookie))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list = json_body(list).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    assert_eq!(list["items"][0]["id"], task_id.to_string());
    assert_eq!(list["items"][0]["allowed_actions"][0], "review");
    assert_eq!(list["summary"]["pending"], 1);
    assert_eq!(list["summary"]["all"], 1);

    let filtered = app
        .clone()
        .oneshot(get(
            &format!(
                "/api/v1/processing-tasks?view=pending&stage=identification&status=waiting-confirmation&inbox_directory_id={inbox}&q=dune&limit=1"
            ),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(filtered.status(), StatusCode::OK);
    assert_eq!(
        json_body(filtered).await["items"][0]["id"],
        task_id.to_string()
    );

    for path in [
        "/api/v1/processing-tasks?view=pending&view=all",
        "/api/v1/processing-tasks?unknown=value",
        "/api/v1/processing-tasks?limit=201",
    ] {
        let invalid = app.clone().oneshot(get(path, &cookie)).await.unwrap();
        assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(invalid).await["error"]["code"],
            "validation.failed"
        );
    }

    let task = app
        .clone()
        .oneshot(get(&format!("/api/v1/processing-tasks/{task_id}"), &cookie))
        .await
        .unwrap();
    assert_eq!(task.status(), StatusCode::OK);
    assert_eq!(json_body(task).await["status"], "waiting-confirmation");

    let detail = app
        .clone()
        .oneshot(get(
            &format!("/api/v1/processing-tasks/{task_id}/identification"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let detail_text = body_text(detail).await;
    assert!(!detail_text.contains(SECRET));
    assert!(!detail_text.contains(&fixture.config().config_dir.to_string_lossy().to_string()));
    assert!(!detail_text.contains("source_hash"));
    let detail: Value = serde_json::from_str(&detail_text).unwrap();
    assert_eq!(detail["review_case_id"], case_id.to_string());
    assert_eq!(detail["decision"]["level"], "probable");
    assert_eq!(detail["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(detail["evidence_truncated"], false);

    let reviews = app
        .clone()
        .oneshot(get(
            &format!(
                "/api/v1/review-cases?decision_level=probable&inbox_directory_id={inbox}&updated_before=9999-01-01T00%3A00%3A00Z&limit=1"
            ),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(reviews.status(), StatusCode::OK);
    assert_eq!(
        json_body(reviews).await["items"][0]["id"],
        case_id.to_string()
    );

    let review = app
        .clone()
        .oneshot(get(&format!("/api/v1/review-cases/{case_id}"), &cookie))
        .await
        .unwrap();
    assert_eq!(review.status(), StatusCode::OK);
    assert_eq!(json_body(review).await["task_id"], task_id.to_string());

    let oversized = app
        .oneshot(get("/api/v1/review-cases?limit=201", &cookie))
        .await
        .unwrap();
    assert_eq!(oversized.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json_body(oversized).await["error"]["code"],
        "validation.failed"
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_review_cases")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn write_guards_idempotency_and_absent_manual_decision_paths_preserve_state() {
    let (_fixture, db, app, cookie, task_id, case_id, _inbox) = authenticated_case().await;

    let unauthenticated = app
        .clone()
        .oneshot(get("/api/v1/processing-tasks/not-a-uuid", ""))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let untrusted = app
        .clone()
        .oneshot(post(
            &format!("/api/v1/processing-tasks/{task_id}/attempts"),
            &cookie,
            CSRF,
            "https://evil.test",
            "retry-one",
        ))
        .await
        .unwrap();
    assert_eq!(untrusted.status(), StatusCode::FORBIDDEN);

    let retry_path = format!("/api/v1/processing-tasks/{task_id}/attempts");
    let first = app
        .clone()
        .oneshot(post(&retry_path, &cookie, CSRF, ORIGIN, "retry-one"))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = json_body(first).await;
    let second = app
        .clone()
        .oneshot(post(&retry_path, &cookie, CSRF, ORIGIN, "retry-one"))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(second).await, first);
    assert_eq!(first["attempt_count"], 2);

    let cancel_path = format!("/api/v1/processing-tasks/{task_id}/cancel");
    for _ in 0..2 {
        let cancelled = app
            .clone()
            .oneshot(post(&cancel_path, &cookie, CSRF, ORIGIN, "cancel-one"))
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::ACCEPTED);
        assert_eq!(json_body(cancelled).await["status"], "cancelled");
    }

    let decisions_before =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_decisions")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let absent = app
        .oneshot(post(
            &format!("/api/v1/review-cases/{case_id}/decision"),
            &cookie,
            CSRF,
            ORIGIN,
            "manual-decision",
        ))
        .await
        .unwrap();
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_decisions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        decisions_before
    );
}

async fn authenticated_case() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    Uuid,
    Uuid,
    Uuid,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    seed_session(db.pool(), account).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease =
        common::seed_processing_lease(db.pool(), inbox, b"movies/dune.mkv", vec![55], "api-worker")
            .await;
    let hint = FilenameParser::default().parse("movies/dune.mkv").unwrap();
    let outcome = IdentificationService::new(db.pool().clone())
        .identify(
            &lease,
            &hint,
            &[],
            &ProbableProvider,
            &SecretBytes::new(SECRET.as_bytes().to_vec()),
            "zh-CN",
            Some("CN"),
            93_000_000,
        )
        .await
        .unwrap();
    let case_id = outcome.review_case_id.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    (
        fixture,
        db,
        app,
        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        lease.task.id,
        case_id,
        inbox,
    )
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

fn get(path: &str, cookie: &str) -> Request<Body> {
    let mut request = Request::get(path);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::empty()).unwrap()
}

fn post(path: &str, cookie: &str, csrf: &str, origin: &str, key: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(header::ORIGIN, origin)
        .header("sec-fetch-site", "same-origin")
        .header("idempotency-key", key)
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

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).unwrap()
}

struct ProbableProvider;

#[async_trait]
impl MetadataProvider for ProbableProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(vec![candidate()])
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(vec![candidate()])
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Ok(candidate())
    }

    async fn verify_episodes(
        &self,
        _token: &SecretBytes,
        _series_id: i64,
        _episodes: &[(u16, u16)],
        _locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        Ok(Vec::new())
    }
}

fn candidate() -> MetadataCandidate {
    MetadataCandidate {
        identity: CandidateIdentity {
            provider_id: 438_631,
            media_kind: ProviderMediaKind::Movie,
        },
        titles: vec![LocalizedField {
            value: "dune".to_owned(),
            language: "en-US".to_owned(),
            source: FieldLanguageSource::English,
        }],
        summaries: Vec::new(),
        release_dates: vec![LocalizedField {
            value: NaiveDate::from_ymd_opt(2021, 10, 22).unwrap(),
            language: "en-US".to_owned(),
            source: FieldLanguageSource::English,
        }],
        aliases: Vec::new(),
        episodes: Vec::new(),
        provider_version: 1,
        cache_status: CacheStatus::Miss,
        retrieved_at_us: 90_000_000,
    }
}
