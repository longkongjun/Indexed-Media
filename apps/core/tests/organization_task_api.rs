#![allow(clippy::too_many_lines)]

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use common::organization::{
    OrganizationFixture, organization_fixture, organization_fixture_with_nfo,
};
use mediaflow_core::organization::executor::{ExecutionOutcome, OrganizationExecutor};
use mediaflow_core::organization::fs::ProcessingStopToken;
use mediaflow_core::organization::journal_store::JournalStore;
use mediaflow_core::organization::model::{
    ConfirmedNfoMetadata, OrganizationNfoPolicy, OrganizationOperation,
};
use mediaflow_core::organization::plan_service::{
    OrganizationPlanService, OrganizationPlanningPort,
};
use mediaflow_core::organization::plan_store::{OrganizationPlanRecord, OrganizationPlanStore};
use mediaflow_core::organization::planner::{PlanningIdentity, PlanningInput};
use mediaflow_core::organization::task_routes::{self, OrganizationTaskService};
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::random;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::shared::error::AppError;
use mediaflow_core::tasks::processing::model::{ProcessingReason, ProcessingStage};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "organization-task-api-session";
const CSRF: &str = "organization-task-api-csrf";
const ORIGIN: &str = "http://127.0.0.1:3000";

#[tokio::test]
async fn authentication_origin_and_csrf_precede_organization_command_parsing() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    seed_session(fixture.db.pool(), fixture.account_id).await;
    let app = build_router(fixture.config.config().clone(), Some(fixture.db.clone()));
    let task_id = fixture.lease.task.id;

    let unauthenticated = app
        .clone()
        .oneshot(get(
            &format!("/api/v1/processing-tasks/{task_id}/organization"),
            "",
        ))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let bad_origin = app
        .clone()
        .oneshot(post(
            &format!("/api/v1/processing-tasks/{task_id}/organization/executions"),
            cookie(),
            CSRF,
            "https://evil.test",
            "organization-execute",
            &json!({"unknown":true}),
        ))
        .await
        .unwrap();
    assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(bad_origin).await["error"]["code"],
        "origin.untrusted"
    );

    let bad_csrf = app
        .clone()
        .oneshot(post(
            &format!("/api/v1/processing-tasks/{task_id}/organization/executions"),
            cookie(),
            "bad",
            ORIGIN,
            "organization-execute",
            &json!({"unknown":true}),
        ))
        .await
        .unwrap();
    assert_eq!(bad_csrf.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(bad_csrf).await["error"]["code"], "csrf.invalid");

    let strict_body = app
        .oneshot(post(
            &format!("/api/v1/processing-tasks/{task_id}/organization/executions"),
            cookie(),
            CSRF,
            ORIGIN,
            "organization-execute",
            &json!({"unknown":true}),
        ))
        .await
        .unwrap();
    assert_eq!(strict_body.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn production_router_returns_safe_current_and_not_planned_details() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    seed_session(fixture.db.pool(), fixture.account_id).await;
    let app = build_router(fixture.config.config().clone(), Some(fixture.db.clone()));
    let path = format!(
        "/api/v1/processing-tasks/{}/organization",
        fixture.lease.task.id
    );

    let response = app.clone().oneshot(get(&path, cookie())).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["task_id"], fixture.lease.task.id.to_string());
    assert_eq!(body["state"], "planned");
    assert_eq!(body["plan"]["source"]["root_id"], "incoming");
    assert_eq!(
        body["plan"]["source"]["relative_path"],
        "ready/Arrival.2016.mkv"
    );
    assert_eq!(body["plan"]["destination"]["root_id"], "library");
    assert_eq!(body["journals"], json!([]));
    assert!(body["local_result"].is_null());
    let serialized = body.to_string();
    assert!(!serialized.contains(fixture.source_root.to_str().unwrap()));
    assert!(!serialized.contains(fixture.target_root.to_str().unwrap()));
    assert!(!serialized.contains("container_path"));
    assert!(!serialized.contains("nfo_metadata"));

    let unplanned_revision = common::seed_stable_revision(
        fixture.db.pool(),
        fixture.lease.task.inbox_directory_id,
        b"ready/not-planned.mkv",
        vec![9; 16],
    )
    .await;
    let unplanned = ProcessingStore::new(fixture.db.pool().clone())
        .ensure_revision(unplanned_revision, 140_000_000)
        .await
        .unwrap();
    let unplanned_path = format!("/api/v1/processing-tasks/{}/organization", unplanned.id);
    let response = app
        .clone()
        .oneshot(get(&unplanned_path, cookie()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["state"], "not-planned");
    assert!(body["plan"].is_null());
    assert_eq!(body["allowed_actions"], json!(["cancel"]));

    let missing = app
        .oneshot(get(
            &format!("/api/v1/processing-tasks/{}/organization", Uuid::now_v7()),
            cookie(),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(missing).await["error"]["code"], "task.not_found");
}

#[tokio::test]
async fn recalculation_is_receipted_once_and_stale_plan_execution_is_rejected() {
    let (fixture, plans, paused_plan) = paused_fixture().await;
    seed_session(fixture.db.pool(), fixture.account_id).await;
    let app = organization_router(&fixture, plans, 100_000_000);
    let base = format!(
        "/api/v1/processing-tasks/{}/organization",
        fixture.lease.task.id
    );

    let paused = app.clone().oneshot(get(&base, cookie())).await.unwrap();
    assert_eq!(paused.status(), StatusCode::OK);
    let paused = json_body(paused).await;
    assert_eq!(paused["state"], "paused");
    assert_eq!(paused["plan"]["version"], paused_plan.version);
    assert_eq!(
        paused["allowed_actions"],
        json!(["recalculate", "execute", "retry", "cancel"])
    );

    let path = format!("{base}/recalculations");
    let recalculate_key = "r".repeat(255);
    let first = app
        .clone()
        .oneshot(post_empty(&path, cookie(), CSRF, ORIGIN, &recalculate_key))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = json_body(first).await;
    assert_eq!(first["plan"]["version"], paused_plan.version + 1);
    let replay = app
        .clone()
        .oneshot(post_empty(&path, cookie(), CSRF, ORIGIN, &recalculate_key))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(replay).await, first);
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM organization_plan_recalculation_receipts WHERE task_id=?",
    )
    .bind(fixture.lease.task.id.as_bytes().as_slice())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(receipt_count, 1);

    let stale = app
        .oneshot(post(
            &format!("{base}/executions"),
            cookie(),
            CSRF,
            ORIGIN,
            "execute-stale-plan",
            &json!({"plan_version": paused_plan.version}),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "request.conflict");
}

#[tokio::test]
async fn execution_receipt_repairs_response_loss_without_an_extra_attempt() {
    let (fixture, plans, paused_plan) = paused_fixture().await;
    seed_session(fixture.db.pool(), fixture.account_id).await;
    plans
        .authorize_once(
            fixture.account_id,
            fixture.lease.task.id,
            paused_plan.version,
            "execute-after-response-loss",
        )
        .await
        .unwrap();
    let attempts_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks_processing_attempts WHERE task_id=?")
            .bind(fixture.lease.task.id.as_bytes().as_slice())
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
    let app = organization_router(&fixture, plans, 110_000_000);
    let path = format!(
        "/api/v1/processing-tasks/{}/organization/executions",
        fixture.lease.task.id
    );
    let request = || {
        post(
            &path,
            cookie(),
            CSRF,
            ORIGIN,
            "execute-after-response-loss",
            &json!({"plan_version": paused_plan.version}),
        )
    };

    let first = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    let first = json_body(first).await;
    assert_eq!(first["plan"]["authorization"], "one-time");
    assert_eq!(first["allowed_actions"], json!(["recalculate", "cancel"]));
    let replay = app.oneshot(request()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(replay).await, first);
    let attempts_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks_processing_attempts WHERE task_id=?")
            .bind(fixture.lease.task.id.as_bytes().as_slice())
            .fetch_one(fixture.db.pool())
            .await
            .unwrap();
    assert_eq!(attempts_after, attempts_before + 1);
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM organization_plan_authorization_receipts WHERE plan_id=?",
    )
    .bind(paused_plan.id.as_bytes().as_slice())
    .fetch_one(fixture.db.pool())
    .await
    .unwrap();
    assert_eq!(receipts, 1);
}

#[tokio::test]
async fn verified_journal_without_result_is_reported_as_recovery_pending() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    seed_session(fixture.db.pool(), fixture.account_id).await;
    let executor = executor(&fixture, 115_000_000);
    let ExecutionOutcome::Completed(result) = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("copy must reach a verified result");
    };
    sqlx::query("DELETE FROM organization_local_results WHERE id=?")
        .bind(result.id.as_bytes().as_slice())
        .execute(fixture.db.pool())
        .await
        .unwrap();
    let app = organization_router(
        &fixture,
        OrganizationPlanService::new(
            Arc::new(FixedPlanningInput(movie_planning_input(&fixture))),
            fixture.db.pool().clone(),
        ),
        116_000_000,
    );
    let response = app
        .oneshot(get(
            &format!(
                "/api/v1/processing-tasks/{}/organization",
                fixture.lease.task.id
            ),
            cookie(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["state"], "recovery-pending");
    assert!(body["local_result"].is_null());
    assert_eq!(body["journals"][0]["status"], "verified");
    assert_eq!(body["journals"][0]["destination"]["root_id"], "library");
    assert_eq!(body["allowed_actions"], json!(["cancel"]));
}

#[tokio::test]
async fn rollback_accepts_partial_result_and_external_change_requires_manual_review() {
    let partial = organization_fixture_with_nfo(
        OrganizationOperation::Copy,
        OrganizationNfoPolicy::GenerateMissing,
    )
    .await;
    seed_session(partial.db.pool(), partial.account_id).await;
    let partial_executor = executor(&partial, 120_000_000);
    let ExecutionOutcome::Completed(partial_result) = partial_executor
        .run_file_stage(&partial.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("file-only execution must persist a partial result");
    };
    assert_eq!(partial_result.status.as_str(), "partial-success");
    let partial_app = organization_router(
        &partial,
        OrganizationPlanService::new(
            Arc::new(FixedPlanningInput(movie_planning_input(&partial))),
            partial.db.pool().clone(),
        ),
        121_000_000,
    );
    let partial_path = format!(
        "/api/v1/processing-tasks/{}/organization/rollbacks",
        partial.lease.task.id
    );
    let response = partial_app
        .oneshot(post(
            &partial_path,
            cookie(),
            CSRF,
            ORIGIN,
            "rollback-partial-result",
            &json!({"result_version": partial_result.version}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let body = json_body(response).await;
    assert_eq!(body["state"], "completed");
    assert_eq!(body["local_result"]["status"], "compensated");
    assert!(!partial.destination_path().exists());

    let changed = organization_fixture(OrganizationOperation::Copy).await;
    seed_session(changed.db.pool(), changed.account_id).await;
    let changed_executor = executor(&changed, 130_000_000);
    let ExecutionOutcome::Completed(result) = changed_executor
        .run_next(&changed.lease, ProcessingStopToken::default())
        .await
        .unwrap()
    else {
        panic!("copy must finish before rollback");
    };
    let changed_app = organization_router(
        &changed,
        OrganizationPlanService::new(
            Arc::new(FixedPlanningInput(movie_planning_input(&changed))),
            changed.db.pool().clone(),
        ),
        131_000_000,
    );
    let changed_path = format!(
        "/api/v1/processing-tasks/{}/organization/rollbacks",
        changed.lease.task.id
    );
    let stale = changed_app
        .clone()
        .oneshot(post(
            &changed_path,
            cookie(),
            CSRF,
            ORIGIN,
            "rollback-stale-result",
            &json!({"result_version": result.version + 1}),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read(changed.destination_path()).unwrap(),
        b"arrival-media"
    );

    std::fs::write(changed.destination_path(), b"external-change").unwrap();
    let rollback = || {
        post(
            &changed_path,
            cookie(),
            CSRF,
            ORIGIN,
            "rollback-external-change",
            &json!({"result_version": result.version}),
        )
    };
    let response = changed_app.clone().oneshot(rollback()).await.unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let response = json_body(response).await;
    assert_eq!(response["state"], "manual-review");
    assert_eq!(response["local_result"]["status"], "manual-review");
    let replay = changed_app.oneshot(rollback()).await.unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(replay).await, response);
    assert_eq!(
        std::fs::read(changed.destination_path()).unwrap(),
        b"external-change"
    );
    let audit: String = sqlx::query_scalar(
        "SELECT COALESCE(GROUP_CONCAT(action || outcome || COALESCE(subject_id,'') || safe_details_json), '')
         FROM platform_audit_events",
    )
    .fetch_one(changed.db.pool())
    .await
    .unwrap();
    assert!(audit.contains("organization.rollbacksuccess"));
    assert!(audit.contains(&changed.lease.task.id.to_string()));
    assert!(!audit.contains(changed.source_root.to_str().unwrap()));
    assert!(!audit.contains(changed.target_root.to_str().unwrap()));
    assert!(!audit.contains("external-change"));
}

#[derive(Clone)]
struct FixedPlanningInput(PlanningInput);

#[async_trait]
impl OrganizationPlanningPort for FixedPlanningInput {
    async fn load(&self, _account_id: Uuid, _task_id: Uuid) -> Result<PlanningInput, AppError> {
        Ok(self.0.clone())
    }
}

async fn paused_fixture() -> (
    OrganizationFixture,
    OrganizationPlanService,
    OrganizationPlanRecord,
) {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let mut input = movie_planning_input(&fixture);
    input.target.automatic = false;
    input.target.rules.clear();
    let plans = OrganizationPlanService::new(
        Arc::new(FixedPlanningInput(input)),
        fixture.db.pool().clone(),
    );
    let paused_plan = plans
        .ensure_current(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    tasks
        .finish_identification_complete(
            &fixture.lease,
            ProcessingReason::IdentificationConfirmedTitleYear,
            90_000_000,
        )
        .await
        .unwrap();
    let planning_lease = tasks
        .claim_next(
            "organization-api-planning",
            &[ProcessingStage::Planning],
            91_000_000,
        )
        .await
        .unwrap()
        .unwrap();
    tasks
        .finish_organization_plan(&planning_lease, paused_plan.id, false, 92_000_000)
        .await
        .unwrap();
    (fixture, plans, paused_plan)
}

fn movie_planning_input(fixture: &OrganizationFixture) -> PlanningInput {
    PlanningInput {
        task_id: fixture.lease.task.id,
        file_revision_id: fixture.lease.task.file_revision_id,
        selected_identity_id: fixture.plan.draft.selected_identity_id,
        source: fixture.plan.draft.source.clone(),
        source_inbox_id: fixture.lease.task.inbox_directory_id,
        source_writable: false,
        source_unchanged: true,
        destination_exists: false,
        same_filesystem: true,
        explicit_tags: BTreeSet::new(),
        one_time_authorized: false,
        identity: PlanningIdentity::Movie {
            title: "Arrival".to_owned(),
            year: Some(2016),
            version_label: None,
        },
        nfo_metadata: ConfirmedNfoMetadata::default(),
        target: fixture.plan.draft.target.clone(),
    }
}

fn executor(fixture: &OrganizationFixture, now_us: i64) -> OrganizationExecutor {
    OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(now_us)),
    )
}

fn organization_router(
    fixture: &OrganizationFixture,
    plans: OrganizationPlanService,
    now_us: i64,
) -> Router {
    let service = OrganizationTaskService::new(
        fixture.db.pool().clone(),
        plans,
        executor(fixture, now_us),
        OutboxNotifier::new(),
    );
    task_routes::router(fixture.config.config().clone(), &fixture.db, service)
}

async fn seed_session(pool: &sqlx::SqlitePool, account_id: Uuid) {
    sqlx::query(
        "INSERT INTO identity_sessions
         (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,
          credential_version,created_at_us,last_used_at_us,revoked_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,NULL)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
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

fn cookie() -> &'static str {
    "__Host-mediaflow_session=organization-task-api-session"
}

fn get(path: &str, cookie: &str) -> Request<Body> {
    let mut request = Request::get(path);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::empty()).unwrap()
}

fn post(
    path: &str,
    cookie: &str,
    csrf: &str,
    origin: &str,
    key: &str,
    body: &Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", csrf)
        .header(header::ORIGIN, origin)
        .header("sec-fetch-site", "same-origin")
        .header("idempotency-key", key)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_empty(path: &str, cookie: &str, csrf: &str, origin: &str, key: &str) -> Request<Body> {
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

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
