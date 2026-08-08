#![allow(clippy::too_many_lines)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{
    CacheStatus, CandidateIdentity, EpisodeIdentity, FieldLanguageSource, LocalizedField,
    MetadataCandidate, ProviderError, ProviderMediaKind, SecretString, TmdbConfigCommand,
    TmdbExternalIdRequest, TmdbSearchRequest,
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

const COOKIE_TOKEN: &str = "manual-decision-api-session";
const CSRF: &str = "manual-decision-api-csrf";
const ORIGIN: &str = "http://127.0.0.1:3000";

#[tokio::test]
async fn accepts_all_three_exact_unions_and_replays_the_same_receipt() {
    for (index, (body, expected_kind)) in [
        (
            json!({
                "kind":"select-provider-candidate","provider":"tmdb","media_type":"movie",
                "provider_id":"438631","save_feedback":false
            }),
            "select-provider-candidate",
        ),
        (
            json!({
                "kind":"rematch-with-hints","media_type":"tv","title":"Three Body",
                "year":2023,"season":1,"episodes":[1,2],"save_feedback":true
            }),
            "rematch-with-hints",
        ),
        (
            json!({
                "kind":"select-generic-video","display_title":"Family Trip 2026",
                "group_hint":"Family Trip","save_grouping_feedback":false
            }),
            "select-generic-video",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let harness = harness(Arc::new(SearchProvider::successful(1)), index).await;
        let key = format!("decision-key-{index}");
        let first = harness
            .app
            .clone()
            .oneshot(decision_request(
                harness.case_id,
                &harness.cookie,
                Some("1"),
                Some(&key),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::ACCEPTED);
        let first = json_body(first).await;
        assert_eq!(first["review_case_id"], harness.case_id.to_string());
        assert_eq!(first["case_version"], 2);
        assert_eq!(first["kind"], expected_kind);
        assert_eq!(first["state"], "accepted");

        let replay = harness
            .app
            .clone()
            .oneshot(decision_request(
                harness.case_id,
                &harness.cookie,
                Some("1"),
                Some(&key),
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        assert_eq!(json_body(replay).await, first);
        let second_pending = harness
            .app
            .clone()
            .oneshot(decision_request(
                harness.case_id,
                &harness.cookie,
                Some("2"),
                Some("another-decision-key"),
                &json!({
                    "kind":"select-generic-video","display_title":"Second decision",
                    "group_hint":null,"save_grouping_feedback":false
                }),
            ))
            .await
            .unwrap();
        assert_eq!(second_pending.status(), StatusCode::CONFLICT);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_task_decisions")
                .fetch_one(harness.db.pool())
                .await
                .unwrap(),
            1
        );
    }
}

#[tokio::test]
async fn rejects_missing_or_invalid_headers_stale_versions_and_non_exact_json_without_writes() {
    let harness = harness(Arc::new(SearchProvider::successful(1)), 20).await;
    let valid = json!({
        "kind":"select-provider-candidate","provider":"tmdb","media_type":"movie",
        "provider_id":"438631","save_feedback":false
    });
    for request in [
        decision_request(
            harness.case_id,
            &harness.cookie,
            None,
            Some("decision-key"),
            &valid,
        ),
        decision_request(
            harness.case_id,
            &harness.cookie,
            Some("\"1\""),
            Some("decision-key"),
            &valid,
        ),
        decision_request(
            harness.case_id,
            &harness.cookie,
            Some("1"),
            Some("short"),
            &valid,
        ),
        decision_request(
            harness.case_id,
            &harness.cookie,
            Some("1"),
            Some("decision-key"),
            &json!({
                "kind":"select-provider-candidate","provider":"tmdb","media_type":"movie",
                "provider_id":"438631","save_feedback":false,"raw_path":"/private/media"
            }),
        ),
        decision_request(
            harness.case_id,
            &harness.cookie,
            Some("1"),
            Some("decision-key"),
            &json!({
                "kind":"rematch-with-hints","media_type":"tv","title":"Series","year":2026,
                "season":1,"episodes":[1,1],"save_feedback":false
            }),
        ),
    ] {
        let response = harness.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "validation.failed"
        );
    }

    let stale = harness
        .app
        .clone()
        .oneshot(decision_request(
            harness.case_id,
            &harness.cookie,
            Some("9"),
            Some("decision-key"),
            &valid,
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "request.conflict");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_task_decisions")
            .fetch_one(harness.db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn candidate_search_is_bounded_safe_and_does_not_persist_identification_history() {
    let provider = Arc::new(SearchProvider::successful(25));
    let harness = harness(provider.clone(), 30).await;
    let before = history_counts(harness.db.pool()).await;
    let response = harness
        .app
        .clone()
        .oneshot(get(
            &format!(
                "/api/v1/review-cases/{}/candidates?q=%20Dune%20&media_type=movie&locale=zh-CN&limit=20",
                harness.case_id
            ),
            &harness.cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 20);
    assert_eq!(provider.search_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.last_query.lock().unwrap().as_deref(), Some("Dune"));
    for item in items {
        let keys = item
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "locale",
                "media_type",
                "original_title",
                "provider",
                "provider_id",
                "title",
                "year",
            ]
        );
        let serialized = item.to_string();
        assert!(!serialized.contains("artwork"));
        assert!(!serialized.contains("raw"));
    }
    assert_eq!(history_counts(harness.db.pool()).await, before);

    let missing = harness
        .app
        .oneshot(get(
            &format!(
                "/api/v1/review-cases/{}/candidates?q=Dune&media_type=movie&locale=zh-CN",
                Uuid::now_v7()
            ),
            &harness.cookie,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(provider.search_calls.load(Ordering::SeqCst), 1);
}

struct Harness {
    _fixture: common::TestConfigDir,
    db: mediaflow_core::platform::db::Db,
    app: axum::Router,
    cookie: String,
    case_id: Uuid,
}

async fn harness(provider: Arc<dyn MetadataProvider>, discriminator: usize) -> Harness {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    seed_session(db.pool(), account).await;
    configure_tmdb(&fixture, &db).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let path = format!("movies/review-{discriminator}.mkv");
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        path.as_bytes(),
        vec![u8::try_from(discriminator % 250).unwrap()],
        "decision-api",
    )
    .await;
    let identification = IdentificationStore::new(db.pool().clone());
    let attempt = identification
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    let case_id = identification
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
            title_hint: Some("Dune"),
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
    }
}

async fn configure_tmdb(fixture: &common::TestConfigDir, db: &mediaflow_core::platform::db::Db) {
    ConnectorService::new(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(AcceptTester),
    )
    .put_tmdb(
        TmdbConfigCommand {
            api_read_access_token: SecretString::new("server-only-token-value".to_owned()),
            locale: "zh-CN".to_owned(),
            region: Some("CN".to_owned()),
        },
        0,
    )
    .await
    .unwrap();
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

fn decision_request(
    case_id: Uuid,
    cookie: &str,
    version: Option<&str>,
    key: Option<&str>,
    body: &Value,
) -> Request<Body> {
    let mut builder = Request::post(format!("/api/v1/review-cases/{case_id}/decisions"))
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(version) = version {
        builder = builder.header(header::IF_MATCH, version);
    }
    if let Some(key) = key {
        builder = builder.header("idempotency-key", key);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap()).unwrap()
}

async fn history_counts(pool: &sqlx::SqlitePool) -> (i64, i64, i64) {
    (
        sqlx::query_scalar("SELECT COUNT(*) FROM identification_evidence")
            .fetch_one(pool)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT COUNT(*) FROM identification_candidates")
            .fetch_one(pool)
            .await
            .unwrap(),
        sqlx::query_scalar("SELECT COUNT(*) FROM identification_decisions")
            .fetch_one(pool)
            .await
            .unwrap(),
    )
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

struct SearchProvider {
    candidates: Vec<MetadataCandidate>,
    search_calls: AtomicUsize,
    last_query: std::sync::Mutex<Option<String>>,
}

impl SearchProvider {
    fn successful(count: usize) -> Self {
        Self {
            candidates: (1..=count)
                .map(|index| candidate(i64::try_from(index).unwrap()))
                .collect(),
            search_calls: AtomicUsize::new(0),
            last_query: std::sync::Mutex::new(None),
        }
    }
}

#[async_trait]
impl MetadataProvider for SearchProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(Vec::new())
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.search_calls.fetch_add(1, Ordering::SeqCst);
        *self.last_query.lock().unwrap() = Some(request.title.clone());
        Ok(self.candidates.clone())
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Ok(candidate(provider_id))
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

fn candidate(provider_id: i64) -> MetadataCandidate {
    MetadataCandidate {
        identity: CandidateIdentity {
            provider_id,
            media_kind: ProviderMediaKind::Movie,
        },
        titles: vec![LocalizedField {
            value: format!("Dune {provider_id}"),
            language: "zh-CN".to_owned(),
            source: FieldLanguageSource::Preferred,
        }],
        summaries: Vec::new(),
        release_dates: vec![LocalizedField {
            value: NaiveDate::from_ymd_opt(2021, 10, 22).unwrap(),
            language: "zh-CN".to_owned(),
            source: FieldLanguageSource::Preferred,
        }],
        aliases: Vec::new(),
        episodes: Vec::new(),
        provider_version: 1,
        cache_status: CacheStatus::Miss,
        retrieved_at_us: 90_000_000,
    }
}
