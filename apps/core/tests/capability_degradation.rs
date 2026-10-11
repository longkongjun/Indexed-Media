//! A4 降级矩阵系统测试：DSH 停机、坏输出、重复提交与 Core 重启下，确定性整理与已入库
//! 事实不受影响；AI 通道的全部故障都收敛为结构化拒绝，不污染任务状态或事件流。

mod common;

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::organization::organization_fixture;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::organization::executor::{ExecutionOutcome, OrganizationExecutor};
use mediaflow_core::organization::fs::ProcessingStopToken;
use mediaflow_core::organization::journal_store::JournalStore;
use mediaflow_core::organization::model::OrganizationOperation;
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::db::{Db, open_pool};
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const TOKEN: &str = "a4-degradation-token-0123456789abcdef";

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    serde_json::from_slice(&bytes).expect("json body")
}

#[allow(clippy::needless_pass_by_value)]
fn call(token: &str, body: Value) -> Request<Body> {
    Request::post("/api/capability/v1/calls")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

fn call_envelope(request_id: Uuid, capability: &str, mode: &str, revision: Option<&str>) -> Value {
    json!({
        "protocol_version": "media-capability.v1",
        "request_id": request_id.to_string(),
        "session_id": null,
        "task_id": null,
        "capability": capability,
        "actor": { "kind": "ai", "id": "dsh-degradation" },
        "input": {},
        "execution": {
            "mode": mode,
            "idempotency_key": format!("idem-{request_id}"),
            "expected_revision": revision,
            "deadline_ms": 30000
        }
    })
}

#[tokio::test]
async fn deterministic_organization_completes_while_gateway_is_unconfigured() {
    // DSH 通道完全关闭（未配置令牌）不影响确定性整理：同一数据库上，能力调用全部
    // 稳定拒绝的同时，move 计划由执行器正常推进到 verified。
    let fixture = organization_fixture(OrganizationOperation::Move).await;
    let app = build_router(fixture.config.config().clone(), Some(fixture.db.clone()));

    let rejected = json_body(
        app.clone()
            .oneshot(call(
                TOKEN,
                call_envelope(Uuid::now_v7(), "media.task-status", "read-only", None),
            ))
            .await
            .expect("response"),
    )
    .await;
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(
        rejected["error"]["category"], "provider",
        "unconfigured gateway must fail closed: {rejected}"
    );

    let executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(1_000)),
    );
    let outcome = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .expect("deterministic flow must not depend on the AI channel");
    assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
    assert_eq!(
        std::fs::read(fixture.destination_path()).unwrap(),
        b"arrival-media"
    );
    let journals = JournalStore::new(fixture.db.pool().clone())
        .for_task(fixture.account_id, fixture.lease.task.id)
        .await
        .unwrap();
    assert_eq!(
        journals[0].status,
        mediaflow_core::organization::journal_store::JournalStatus::Verified
    );
}

#[tokio::test]
async fn hostile_envelopes_never_mutate_core_state() {
    let fixture = common::TestConfigDir::new(RunMode::Development).with_capability_token(TOKEN);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"a4-hostile.mkv", vec![1]).await;
    let tasks = mediaflow_core::tasks::processing::store::ProcessingStore::new(db.pool().clone());
    let task = tasks.ensure_revision(revision, 90_000_001).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));

    let hostile: Vec<Value> = vec![
        json!("a bare string"),
        json!([1, 2, 3]),
        json!({}),
        json!({
            "protocol_version": "media-capability.v9",
            "request_id": Uuid::now_v7().to_string(),
            "session_id": null, "task_id": null, "capability": "media.task-status",
            "actor": { "kind": "ai", "id": "dsh" }, "input": {},
            "execution": { "mode": "read-only", "idempotency_key": "idem-aaaaaaaaaa", "expected_revision": null, "deadline_ms": 1 }
        }),
        json!({
            "protocol_version": "media-capability.v1",
            "request_id": Uuid::now_v7().to_string(),
            "session_id": null, "task_id": null, "capability": "media.task-status",
            "actor": { "kind": "ai", "id": "dsh" }, "input": {},
            "execution": { "mode": "reversible-write", "idempotency_key": "idem-bbbbbbbbbb", "expected_revision": null, "deadline_ms": 1 }
        }),
    ];
    for payload in hostile {
        let response = app
            .clone()
            .oneshot(call(TOKEN, payload))
            .await
            .expect("response");
        let status = response.status();
        let body = json_body(response).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "protocol errors travel in envelopes"
        );
        assert_eq!(
            body["status"], "rejected",
            "hostile payload must be rejected: {body}"
        );
    }
    // 物理层截断的 JSON 同样走结构化拒绝。
    let truncated = app
        .clone()
        .oneshot(
            Request::post("/api/capability/v1/calls")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{\"protocol_version\": \"media-cap"))
                .expect("request"),
        )
        .await
        .expect("response");
    let body = json_body(truncated).await;
    assert_eq!(
        body["status"], "rejected",
        "truncated payload must be rejected: {body}"
    );

    // 恶意流量后：任务事实零变化、事件流零留痕（拒绝不产生任务事实）。
    let task_after = tasks.get_by_task_id(task.id).await.unwrap();
    assert_eq!(task_after.status, task.status);
    assert_eq!(task_after.stage, task.stage);
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM capability_events")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(events, 0, "rejections must not append capability events");
}

#[tokio::test]
async fn idempotency_conflict_does_not_poison_subsequent_calls() {
    let fixture = common::TestConfigDir::new(RunMode::Development).with_capability_token(TOKEN);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision =
        common::seed_stable_revision(db.pool(), inbox, b"a4-idempotency.mkv", vec![1]).await;
    let tasks = mediaflow_core::tasks::processing::store::ProcessingStore::new(db.pool().clone());
    let task = tasks.ensure_revision(revision, 90_000_001).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let input = json!({ "task_id": task.id.to_string() });

    let envelope = |request_id: Uuid, key: &str| {
        json!({
            "protocol_version": "media-capability.v1",
            "request_id": request_id.to_string(),
            "session_id": null,
            "task_id": task.id.to_string(),
            "capability": "media.task-status",
            "actor": { "kind": "ai", "id": "dsh" },
            "input": input,
            "execution": { "mode": "read-only", "idempotency_key": key, "expected_revision": null, "deadline_ms": 30000 }
        })
    };

    let first = json_body(
        app.clone()
            .oneshot(call(TOKEN, envelope(Uuid::now_v7(), "idem-shared-key")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first["status"], "succeeded");
    let conflict = json_body(
        app.clone()
            .oneshot(call(TOKEN, envelope(Uuid::now_v7(), "idem-shared-key")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(conflict["status"], "rejected");
    assert_eq!(conflict["error"]["category"], "conflict");

    // 冲突不卡死通道：新键新请求照常成功，任务仍只读不改。
    let after = json_body(
        app.clone()
            .oneshot(call(TOKEN, envelope(Uuid::now_v7(), "idem-fresh-key")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(after["status"], "succeeded");
    let task_after = tasks.get_by_task_id(task.id).await.unwrap();
    assert_eq!(task_after.status, task.status);
}

#[tokio::test]
async fn cursor_replay_survives_database_reopen() {
    // Core 重启等价：连接关闭后按同一数据库重开，事件按 cursor 完整补发。
    let fixture = common::TestConfigDir::new(RunMode::Development).with_capability_token(TOKEN);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, b"a4-reopen.mkv", vec![1]).await;
    let tasks = mediaflow_core::tasks::processing::store::ProcessingStore::new(db.pool().clone());
    let task = tasks.ensure_revision(revision, 90_000_001).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let input = json!({ "task_id": task.id.to_string() });

    for _ in 0..3 {
        let envelope = json!({
            "protocol_version": "media-capability.v1",
            "request_id": Uuid::now_v7().to_string(),
            "session_id": null,
            "task_id": task.id.to_string(),
            "capability": "media.task-status",
            "actor": { "kind": "ai", "id": "dsh" },
            "input": input,
            "execution": { "mode": "read-only", "idempotency_key": format!("idem-{}", Uuid::now_v7()), "expected_revision": null, "deadline_ms": 30000 }
        });
        let body = json_body(app.clone().oneshot(call(TOKEN, envelope)).await.unwrap()).await;
        assert_eq!(body["status"], "succeeded");
    }
    let database_path = fixture.database_path();
    let db2: Db = {
        let pool = open_pool(&database_path).await.unwrap();
        Db::new(pool, database_path.clone())
    };
    let events: Vec<i64> =
        sqlx::query_scalar("SELECT sequence FROM capability_events ORDER BY sequence")
            .fetch_all(db2.pool())
            .await
            .unwrap();
    assert_eq!(
        events.len(),
        3,
        "three persisted calls must survive the reopen"
    );
    assert!(events.windows(2).all(|pair| pair[0] < pair[1]));

    let app2 = build_router(fixture.config().clone(), Some(db2));
    let page = json_body(
        app2.oneshot(
            Request::get("/api/capability/v1/events?limit=10")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .unwrap(),
    )
    .await;
    let sequences: Vec<i64> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["sequence"].as_i64().unwrap())
        .collect();
    assert_eq!(
        sequences, events,
        "cursor replay must be gapless after reopen"
    );
}

#[tokio::test]
async fn gateway_responses_and_events_leak_no_credentials_or_host_paths() {
    let fixture = common::TestConfigDir::new(RunMode::Development).with_capability_token(TOKEN);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision =
        common::seed_stable_revision(db.pool(), inbox, b"a4-redaction.mkv", vec![1]).await;
    let tasks = mediaflow_core::tasks::processing::store::ProcessingStore::new(db.pool().clone());
    let task = tasks.ensure_revision(revision, 90_000_001).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));

    // 成功调用 + 过期 revision 拒绝（两类信封都进入响应与事件流）。
    let attempts = vec![
        json!({
            "protocol_version": "media-capability.v1",
            "request_id": Uuid::now_v7().to_string(),
            "session_id": null, "task_id": task.id.to_string(),
            "capability": "media.task-status",
            "actor": { "kind": "ai", "id": "dsh" },
            "input": { "task_id": task.id.to_string() },
            "execution": { "mode": "read-only", "idempotency_key": format!("idem-{}", Uuid::now_v7()), "expected_revision": null, "deadline_ms": 30000 }
        }),
        json!({
            "protocol_version": "media-capability.v1",
            "request_id": Uuid::now_v7().to_string(),
            "session_id": null, "task_id": task.id.to_string(),
            "capability": "media.task-status",
            "actor": { "kind": "ai", "id": "dsh" },
            "input": { "task_id": task.id.to_string() },
            "execution": { "mode": "read-only", "idempotency_key": format!("idem-{}", Uuid::now_v7()), "expected_revision": "task.version:999999", "deadline_ms": 30000 }
        }),
    ];
    let mut response_texts = Vec::new();
    for envelope in attempts {
        let response = app.clone().oneshot(call(TOKEN, envelope)).await.unwrap();
        response_texts.push(json_body(response).await.to_string());
    }
    let events_page = json_body(
        app.clone()
            .oneshot(
                Request::get("/api/capability/v1/events?limit=10")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .unwrap(),
    )
    .await
    .to_string();
    let stored_events: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(progress_json,'') || COALESCE(evidence_json,'') || COALESCE(error_json,'') FROM capability_events",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();

    let forbidden_markers: &[&str] = &[
        TOKEN,
        "password",
        "secret",
        "api_key",
        "/private/tmp",
        "/Users/",
        "sqlite",
    ];
    for text in response_texts
        .iter()
        .chain(std::iter::once(&events_page))
        .chain(stored_events.iter())
    {
        let lowered = text.to_ascii_lowercase();
        for marker in forbidden_markers {
            assert!(
                !lowered.contains(marker),
                "capability output must not leak {marker}: {text}"
            );
        }
    }
    assert!(!response_texts.is_empty());
    assert!(
        !stored_events.is_empty(),
        "calls must have persisted events to scan"
    );
}
