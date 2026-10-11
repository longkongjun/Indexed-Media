//! `media-capability.v1` 网关集成测试：A1 闸门的四项自动化验证。
//!
//! 覆盖：只读能力往返、幂等重放无二次副作用、过期 revision 稳定拒绝、非法输入（绝对
//! 路径 / shell / SQL / 凭据）拒绝、令牌认证边界，以及事件按 cursor 无缺口补发。
//! 另含 SA-1-001 修复的 apply-plan 全路径验证（只交付不授权）。

#![allow(clippy::too_many_lines)]

mod common;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::organization::organization_fixture;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::organization::model::OrganizationOperation;
use mediaflow_core::organization::plan_store::{
    AuthorizationGranter, OrganizationPlanRecord, OrganizationPlanStore,
};
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::tasks::processing::model::{ProcessingReason, ProcessingStage};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const TOKEN: &str = "a1-gateway-token-0123456789abcdef";

struct Harness {
    #[allow(dead_code)]
    fixture: common::TestConfigDir,
    app: Router,
    task_id: Uuid,
}

async fn harness(file: &[u8]) -> Harness {
    let fixture = common::TestConfigDir::new(RunMode::Development).with_capability_token(TOKEN);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("database");
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision = common::seed_stable_revision(db.pool(), inbox, file, vec![1]).await;
    let store = ProcessingStore::new(db.pool().clone());
    let task = store
        .ensure_revision(revision, 90_000_001)
        .await
        .expect("task");
    let app = build_router(fixture.config().clone(), Some(db));
    Harness {
        fixture,
        app,
        task_id: task.id,
    }
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    serde_json::from_slice(&bytes).expect("json body")
}

// 测试辅助按值接收 input：调用点都是内联 json! 字面量，克隆成本无意义。
#[allow(clippy::needless_pass_by_value)]
fn call_envelope(
    request_id: Uuid,
    capability: &str,
    task_id: Option<Uuid>,
    input: Value,
    expected_revision: Option<&str>,
) -> Value {
    json!({
        "protocol_version": "media-capability.v1",
        "request_id": request_id.to_string(),
        "session_id": null,
        "task_id": task_id.map(|id| id.to_string()),
        "capability": capability,
        "actor": { "kind": "ai", "id": "dsh-test" },
        "input": input,
        "execution": {
            "mode": if capability == "media.create-plan" { "plan-only" } else { "read-only" },
            "idempotency_key": format!("idem-{request_id}"),
            "expected_revision": expected_revision,
            "deadline_ms": 30000
        }
    })
}

#[tokio::test]
async fn read_only_task_status_round_trips_with_revision() {
    let harness = harness(b"a1-status.mkv").await;
    let request_id = Uuid::now_v7();
    let response = harness
        .app
        .clone()
        .oneshot(
            Request::post("/api/capability/v1/calls")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(
                    call_envelope(
                        request_id,
                        "media.task-status",
                        Some(harness.task_id),
                        json!({ "task_id": harness.task_id.to_string() }),
                        None,
                    )
                    .to_string(),
                )
                .expect("request"),
        )
        .await
        .expect("gateway response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["status"], "succeeded");
    assert_eq!(body["task_id"], harness.task_id.to_string());
    assert_eq!(body["result"]["status"], "queued");
    assert!(
        body["result"]["revision"]
            .as_str()
            .unwrap()
            .starts_with("task.version:")
    );
    assert!(body["error"].is_null());
}

#[tokio::test]
async fn replayed_request_returns_cached_result_without_second_side_effects() {
    let harness = harness(b"a1-replay.mkv").await;
    let request_id = Uuid::now_v7();
    let envelope = call_envelope(
        request_id,
        "media.task-status",
        Some(harness.task_id),
        json!({ "task_id": harness.task_id.to_string() }),
        None,
    );
    let send = |body: String| {
        Request::post("/api/capability/v1/calls")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("request")
    };
    let first = json_body(
        harness
            .app
            .clone()
            .oneshot(send(envelope.to_string()))
            .await
            .expect("first response"),
    )
    .await;
    let second = json_body(
        harness
            .app
            .clone()
            .oneshot(send(envelope.to_string()))
            .await
            .expect("replay response"),
    )
    .await;
    assert_eq!(
        first, second,
        "replay must return the cached result verbatim"
    );

    let events = json_body(
        harness
            .app
            .clone()
            .oneshot(
                Request::get("/api/capability/v1/events?limit=50")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("events request"),
            )
            .await
            .expect("events response"),
    )
    .await;
    assert_eq!(
        events["items"].as_array().map(Vec::len),
        Some(1),
        "replay must not append a second event: {events}"
    );
}

#[tokio::test]
async fn idempotency_key_bound_to_another_request_is_a_stable_conflict() {
    let harness = harness(b"a1-idem-conflict.mkv").await;
    let input = json!({ "task_id": harness.task_id.to_string() });
    let first_body = call_envelope(
        Uuid::now_v7(),
        "media.task-status",
        Some(harness.task_id),
        input.clone(),
        None,
    );
    let mut second_body = call_envelope(
        Uuid::now_v7(),
        "media.task-status",
        Some(harness.task_id),
        input,
        None,
    );
    second_body["execution"]["idempotency_key"] =
        first_body["execution"]["idempotency_key"].clone();

    let send = |body: Value| {
        Request::post("/api/capability/v1/calls")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .expect("request")
    };
    let _first = json_body(
        harness
            .app
            .clone()
            .oneshot(send(first_body))
            .await
            .expect("first"),
    )
    .await;
    let second = json_body(
        harness
            .app
            .clone()
            .oneshot(send(second_body))
            .await
            .expect("second"),
    )
    .await;
    assert_eq!(second["status"], "rejected");
    assert_eq!(second["error"]["category"], "conflict");
    assert_eq!(second["error"]["error_code"], "capability.request.conflict");
}

#[tokio::test]
async fn stale_expected_revision_is_a_stable_conflict() {
    let harness = harness(b"a1-stale-revision.mkv").await;
    let response = harness
        .app
        .clone()
        .oneshot(
            Request::post("/api/capability/v1/calls")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(
                    call_envelope(
                        Uuid::now_v7(),
                        "media.task-status",
                        Some(harness.task_id),
                        json!({ "task_id": harness.task_id.to_string() }),
                        Some("task.version:999999"),
                    )
                    .to_string(),
                )
                .expect("request"),
        )
        .await
        .expect("response");
    let body = json_body(response).await;
    assert_eq!(body["status"], "rejected");
    assert_eq!(body["error"]["category"], "conflict");
    assert_eq!(body["error"]["retryable"], false);
    assert_eq!(body["error"]["required_action"], "replan");
}

#[tokio::test]
async fn unsafe_inputs_are_rejected_with_validation_category() {
    let harness = harness(b"a1-unsafe.mkv").await;
    let cases: Vec<(&str, Value)> = vec![
        (
            "host path",
            json!({ "task_id": harness.task_id.to_string(), "path": "/etc/passwd" }),
        ),
        ("path escape", json!({ "task_id": "../../etc/passwd" })),
        (
            "shell control",
            json!({ "task_id": format!("{} && rm -rf x", harness.task_id) }),
        ),
        (
            "sql statement",
            json!({ "task_id": format!("{}; DROP TABLE tasks", harness.task_id) }),
        ),
        (
            "credential key",
            json!({ "task_id": harness.task_id.to_string(), "api_key": "x" }),
        ),
        (
            "unregistered field",
            json!({ "task_id": harness.task_id.to_string(), "extra": "x" }),
        ),
        (
            "unregistered field",
            json!({ "task_id": harness.task_id.to_string(), "extra": "x" }),
        ),
    ];
    for (index, (label, input)) in cases.into_iter().enumerate() {
        let envelope = call_envelope(
            Uuid::now_v7(),
            "media.task-status",
            Some(harness.task_id),
            input,
            None,
        );
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::post("/api/capability/v1/calls")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(envelope.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = json_body(response).await;
        assert_eq!(
            body["status"], "rejected",
            "case {index} ({label}) must be rejected"
        );
        assert_eq!(
            body["error"]["category"], "validation",
            "case {index} ({label}) must be a validation rejection, got {body}"
        );
    }

    // 只读能力不接受写入模式：稳定拒绝且分类为 precondition（执行许可，非输入校验）。
    let mut write_mode = call_envelope(
        Uuid::now_v7(),
        "media.task-status",
        Some(harness.task_id),
        json!({ "task_id": harness.task_id.to_string() }),
        None,
    );
    write_mode["execution"]["mode"] = json!("approval-required");
    let body = json_body(
        harness
            .app
            .clone()
            .oneshot(
                Request::post("/api/capability/v1/calls")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(write_mode.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response"),
    )
    .await;
    assert_eq!(body["status"], "rejected");
    assert_eq!(body["error"]["category"], "precondition");
}

#[tokio::test]
async fn wrong_or_missing_token_is_rejected_before_any_capability_work() {
    let harness = harness(b"a1-token.mkv").await;
    for header_value in ["Bearer wrong-token-value-000000000", "Bearer", ""] {
        let request = Request::post("/api/capability/v1/calls")
            .header(header::AUTHORIZATION, header_value)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                call_envelope(
                    Uuid::now_v7(),
                    "media.task-status",
                    Some(harness.task_id),
                    json!({ "task_id": harness.task_id.to_string() }),
                    None,
                )
                .to_string(),
            ))
            .expect("request");
        let body = json_body(
            harness
                .app
                .clone()
                .oneshot(request)
                .await
                .expect("response"),
        )
        .await;
        assert_eq!(body["status"], "rejected");
        assert_eq!(body["error"]["category"], "permission");
    }
}

#[tokio::test]
async fn unconfigured_gateway_stably_reports_missing_configuration() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("database");
    let app = build_router(fixture.config().clone(), Some(db));
    let body = json_body(
        app.oneshot(
            Request::post("/api/capability/v1/calls")
                .header(header::AUTHORIZATION, "Bearer whatever-token-value-0000")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .expect("request"),
        )
        .await
        .expect("response"),
    )
    .await;
    assert_eq!(body["error"]["category"], "provider");
    assert_eq!(
        body["error"]["error_code"],
        "capability.integration.not_configured"
    );
}

#[tokio::test]
async fn events_replay_by_cursor_without_gaps_or_duplicates() {
    let harness = harness(b"a1-cursor.mkv").await;
    // 五个成功调用各留一个 completed 事件；预检拒绝不留事件（未产生任务事实）。
    for _ in 0..5 {
        let response = harness
            .app
            .clone()
            .oneshot(
                Request::post("/api/capability/v1/calls")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        call_envelope(
                            Uuid::now_v7(),
                            "media.task-status",
                            Some(harness.task_id),
                            json!({ "task_id": harness.task_id.to_string() }),
                            None,
                        )
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        let body = json_body(response).await;
        assert_eq!(body["status"], "succeeded");
    }

    // 断线模拟：以 limit=2 逐页拉取并携带 cursor。
    let mut cursor: Option<String> = None;
    let mut sequences: Vec<i64> = Vec::new();
    loop {
        let path = match &cursor {
            Some(cursor) => format!("/api/capability/v1/events?limit=2&cursor={cursor}"),
            None => "/api/capability/v1/events?limit=2".to_owned(),
        };
        let page = json_body(
            harness
                .app
                .clone()
                .oneshot(
                    Request::get(path)
                        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("page"),
        )
        .await;
        for item in page["items"].as_array().expect("items") {
            sequences.push(item["sequence"].as_i64().expect("sequence"));
        }
        match page["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    assert_eq!(sequences.len(), 5, "five calls persisted events");
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "cursor pages must be gapless and ordered"
    );
}

#[tokio::test]
async fn plan_preview_without_persisted_plan_reports_needs_input() {
    let harness = harness(b"a1-plan-preview.mkv").await;
    let body = json_body(
        harness
            .app
            .clone()
            .oneshot(
                Request::post("/api/capability/v1/calls")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        call_envelope(
                            Uuid::now_v7(),
                            "media.create-plan",
                            Some(harness.task_id),
                            json!({ "task_id": harness.task_id.to_string() }),
                            None,
                        )
                        .to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response"),
    )
    .await;
    assert_eq!(body["status"], "needs_input");
    assert!(body["result"]["detail"].is_string());
}

#[tokio::test]
async fn unregistered_and_unimplemented_capabilities_are_rejected() {
    let harness = harness(b"a1-unregistered.mkv").await;
    for capability in ["media.retry", "media.apply-plan", "media.scan"] {
        let body = json_body(
            harness
                .app
                .clone()
                .oneshot(
                    Request::post("/api/capability/v1/calls")
                        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            call_envelope(Uuid::now_v7(), capability, None, json!({}), None)
                                .to_string(),
                        ))
                        .expect("request"),
                )
                .await
                .expect("response"),
        )
        .await;
        assert_eq!(
            body["status"], "rejected",
            "{capability} must be rejected in A1: {body}"
        );
    }
}

#[tokio::test]
async fn write_capabilities_refuse_when_organization_commands_are_unavailable() {
    let harness = harness(b"a2-write-unavailable.mkv").await;
    for (capability, revision) in [
        ("media.apply-plan", "plan.version:1"),
        ("media.rollback", "result.version:1"),
    ] {
        let mut envelope = call_envelope(
            Uuid::now_v7(),
            capability,
            Some(harness.task_id),
            json!({ "task_id": harness.task_id.to_string() }),
            Some(revision),
        );
        envelope["execution"]["mode"] = json!("reversible-write");
        let body = json_body(
            harness
                .app
                .clone()
                .oneshot(
                    Request::post("/api/capability/v1/calls")
                        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(envelope.to_string()))
                        .expect("request"),
                )
                .await
                .expect("response"),
        )
        .await;
        assert_eq!(body["status"], "failed", "{capability}: {body}");
        let expected_category = if capability == "media.rollback" {
            "not_found"
        } else {
            "precondition"
        };
        assert_eq!(
            body["error"]["category"], expected_category,
            "{capability} must report its missing precondition structurally: {body}"
        );
    }
}

#[tokio::test]
async fn approval_capability_reports_missing_task_as_structured_not_found() {
    let harness = harness(b"a2-approval-missing.mkv").await;
    let mut envelope = call_envelope(
        Uuid::now_v7(),
        "media.request-approval",
        None,
        json!({ "task_id": "019f8a30-0000-7000-8000-00000000dead" }),
        None,
    );
    envelope["execution"]["mode"] = json!("approval-required");
    let body = json_body(
        harness
            .app
            .clone()
            .oneshot(
                Request::post("/api/capability/v1/calls")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(envelope.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response"),
    )
    .await;
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error"]["category"], "not_found");
}

#[tokio::test]
async fn approval_capability_rejects_wrong_execution_mode() {
    let harness = harness(b"a2-approval-mode.mkv").await;
    // request-approval 只接受 approval-required；read-only 模式是 precondition 拒绝。
    let envelope = call_envelope(
        Uuid::now_v7(),
        "media.request-approval",
        None,
        json!({ "task_id": harness.task_id.to_string() }),
        None,
    );
    let body = json_body(
        harness
            .app
            .clone()
            .oneshot(
                Request::post("/api/capability/v1/calls")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(envelope.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response"),
    )
    .await;
    assert_eq!(body["status"], "rejected");
    assert_eq!(body["error"]["category"], "precondition");
}

#[tokio::test]
async fn dry_run_recalculation_refuses_without_organization_commands() {
    let harness = harness(b"a2-dryrun-unavailable.mkv").await;
    let envelope = call_envelope(
        Uuid::now_v7(),
        "media.create-plan",
        Some(harness.task_id),
        json!({ "task_id": harness.task_id.to_string(), "dry_run": true }),
        None,
    );
    let body = json_body(
        harness
            .app
            .clone()
            .oneshot(
                Request::post("/api/capability/v1/calls")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(envelope.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response"),
    )
    .await;
    // 任务仍处 identification：重算的前置（planning 任务）不满足，是结构化 precondition 失败。
    assert_eq!(body["status"], "failed");
    assert_eq!(body["error"]["category"], "precondition", "body: {body}");
}

#[tokio::test]
async fn malformed_envelopes_are_rejected_with_structured_errors() {
    let harness = harness(b"a1-malformed.mkv").await;
    // 缺字段信封与未知协议版本由 serde 严格形状拒绝；未知能力名走 registry 拒绝。
    // 全部返回 200 + 结构化 rejected（协议错误不使用 HTTP 状态码承载语义）。
    let cases = [
        (json!({}), "validation"),
        (
            json!({
                "protocol_version": "media-capability.v2",
                "request_id": Uuid::now_v7().to_string(),
                "session_id": null, "task_id": null,
                "capability": "media.task-status",
                "actor": { "kind": "ai", "id": "dsh" },
                "input": {},
                "execution": { "mode": "read-only", "idempotency_key": "idem-aaaaaaaaaa", "expected_revision": null, "deadline_ms": 1 }
            }),
            "validation",
        ),
        (
            json!({
                "protocol_version": "media-capability.v1",
                "request_id": Uuid::now_v7().to_string(),
                "session_id": null, "task_id": null,
                "capability": "media.explode",
                "actor": { "kind": "ai", "id": "dsh" },
                "input": {},
                "execution": { "mode": "read-only", "idempotency_key": "idem-bbbbbbbbbb", "expected_revision": null, "deadline_ms": 1 }
            }),
            "validation",
        ),
    ];
    for (payload, expected_category) in cases {
        let body = json_body(
            harness
                .app
                .clone()
                .oneshot(
                    Request::post("/api/capability/v1/calls")
                        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(payload.to_string()))
                        .expect("request"),
                )
                .await
                .expect("response"),
        )
        .await;
        assert_eq!(body["status"], "rejected", "payload {payload}");
        assert_eq!(
            body["error"]["category"], expected_category,
            "payload {payload}"
        );
    }
}

// ---- SA-1-001 修复：media.apply-plan 只交付不授权（网关全路径）----

/// 组装带 organization 部署根与 capability 令牌的网关环境。
struct AuthorizedHarness {
    fixture: common::organization::OrganizationFixture,
    app: Router,
}

async fn authorized_harness() -> AuthorizedHarness {
    let mut fixture = organization_fixture(OrganizationOperation::Move).await;
    fixture
        .config
        .config_mut()
        .capability_token
        .replace(format!("{TOKEN}-org"));
    // 经真实状态机推进到"计划已落定、等待人工"形态（SQL 直改会留下不一致的
    // checkpoint/attempt 事实）：识别完成 → 领取 planning → 计划落定不自动执行。
    let tasks = ProcessingStore::new(fixture.db.pool().clone());
    tasks
        .finish_identification_complete(
            &fixture.lease,
            ProcessingReason::IdentificationConfirmedTitleYear,
            90_000_000,
        )
        .await
        .expect("finish identification");
    let planning_lease = tasks
        .claim_next(
            "capability-apply-planning",
            &[ProcessingStage::Planning],
            91_000_000,
        )
        .await
        .expect("claim planning")
        .expect("planning lease");
    tasks
        .finish_organization_plan(&planning_lease, fixture.plan.id, false, 92_000_000)
        .await
        .expect("finish paused plan");
    let app = build_router(fixture.config.config().clone(), Some(fixture.db.clone()));
    AuthorizedHarness { fixture, app }
}

#[allow(clippy::needless_pass_by_value)]
fn apply_envelope(request_id: Uuid, task_id: Uuid, revision: &str) -> Value {
    json!({
        "protocol_version": "media-capability.v1",
        "request_id": request_id.to_string(),
        "session_id": null,
        "task_id": null,
        "capability": "media.apply-plan",
        "actor": { "kind": "ai", "id": "dsh-apply" },
        "input": { "task_id": task_id.to_string() },
        "execution": {
            "mode": "reversible-write",
            "idempotency_key": format!("idem-{request_id}"),
            "expected_revision": revision,
            "deadline_ms": 30000
        }
    })
}

async fn apply_call(app: &Router, task_id: Uuid, revision: &str) -> Value {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/capability/v1/calls")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}-org"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    apply_envelope(Uuid::now_v7(), task_id, revision).to_string(),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    json_body(response).await
}

async fn authorization_receipt_count(fixture: &common::organization::OrganizationFixture) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM organization_plan_authorization_receipts WHERE task_id=?",
    )
    .bind(fixture.lease.task.id.as_bytes().to_vec())
    .fetch_one(fixture.db.pool())
    .await
    .expect("receipt count")
}

#[tokio::test]
async fn apply_plan_without_web_receipt_waits_for_approval_with_zero_side_effects() {
    // SA-1-001 核心：无 Web 人工授权回执时 AI 通道只得到 needs_approval——
    // 不执行、不写回执、任务留在审批队列、源文件不动。
    let harness = authorized_harness().await;
    let body = apply_call(
        &harness.app,
        harness.fixture.lease.task.id,
        "plan.version:1",
    )
    .await;

    assert_eq!(body["status"], "needs_approval", "body: {body}");
    assert_eq!(body["result"]["plan_version"], "1");
    assert_eq!(body["evidence"][0]["kind"], "plan");
    assert!(
        body["next_actions"]
            .as_array()
            .expect("next actions")
            .iter()
            .any(|action| action == "request_approval")
    );
    assert_eq!(
        authorization_receipt_count(&harness.fixture).await,
        0,
        "capability 通道必须不产生授权回执"
    );
    let status =
        sqlx::query_scalar::<_, String>("SELECT status FROM tasks_processing_tasks WHERE id=?")
            .bind(harness.fixture.lease.task.id.as_bytes().to_vec())
            .fetch_one(harness.fixture.db.pool())
            .await
            .expect("task status");
    assert_eq!(status, "paused", "任务必须留在等待人工的暂停形态");
    assert!(
        harness.fixture.source_path().exists(),
        "源文件必须保持原位（零文件副作用）"
    );
}

#[tokio::test]
async fn apply_plan_delivers_plan_authorized_by_web() {
    // Web 人工授权回执存在时，AI 通道交付执行（不再需要自行授权）。
    let harness = authorized_harness().await;
    let plan: &OrganizationPlanRecord = &harness.fixture.plan;
    OrganizationPlanStore::new(harness.fixture.db.pool().clone())
        .authorize_once(
            harness.fixture.account_id,
            harness.fixture.lease.task.id,
            plan.version,
            "web-manual-execute",
            90_000_100,
            AuthorizationGranter::Web(harness.fixture.account_id),
        )
        .await
        .expect("web authorization");

    let body = apply_call(
        &harness.app,
        harness.fixture.lease.task.id,
        "plan.version:1",
    )
    .await;
    assert_eq!(body["status"], "accepted", "body: {body}");
    assert_eq!(body["result"]["plan_version"], "1");

    let granter = sqlx::query_scalar::<_, Option<String>>(
        "SELECT granted_by_kind FROM organization_plan_authorization_receipts WHERE task_id=?",
    )
    .bind(harness.fixture.lease.task.id.as_bytes().to_vec())
    .fetch_one(harness.fixture.db.pool())
    .await
    .expect("receipt granter");
    assert_eq!(
        authorization_receipt_count(&harness.fixture).await,
        1,
        "交付本身必须不追加第二份回执"
    );
    assert_eq!(granter.as_deref(), Some("web"));
}

#[tokio::test]
async fn apply_plan_with_stale_plan_version_is_rejected_as_conflict() {
    let harness = authorized_harness().await;
    let body = apply_call(
        &harness.app,
        harness.fixture.lease.task.id,
        "plan.version:99",
    )
    .await;
    assert_eq!(body["status"], "failed", "body: {body}");
    assert_eq!(body["error"]["category"], "conflict");
    assert_eq!(body["error"]["required_action"], "replan");
    assert_eq!(
        authorization_receipt_count(&harness.fixture).await,
        0,
        "版本拒绝同样不得产生回执"
    );
}

#[tokio::test]
async fn apply_plan_after_recalculation_awaits_a_fresh_web_receipt() {
    // 旧版本的人工回执不能授权新版本计划：重算后 AI 必须再次等待人工审批。
    let harness = authorized_harness().await;
    let store = OrganizationPlanStore::new(harness.fixture.db.pool().clone());
    let plan = harness.fixture.plan.clone();
    store
        .authorize_once(
            harness.fixture.account_id,
            harness.fixture.lease.task.id,
            plan.version,
            "web-manual-execute-v1",
            90_000_100,
            AuthorizationGranter::Web(harness.fixture.account_id),
        )
        .await
        .expect("web authorization v1");

    let mut changed = plan.draft.clone();
    changed.target.config_version += 1;
    let next = store
        .persist_next(harness.fixture.account_id, changed, 90_000_200)
        .await
        .expect("recalculated plan");
    assert_eq!(next.version, 2);

    let body = apply_call(
        &harness.app,
        harness.fixture.lease.task.id,
        "plan.version:2",
    )
    .await;
    assert_eq!(body["status"], "needs_approval", "body: {body}");
    assert_eq!(body["result"]["plan_version"], "2");
    assert_eq!(
        authorization_receipt_count(&harness.fixture).await,
        1,
        "v1 回执保留，v2 不得被 AI 授权"
    );
}
