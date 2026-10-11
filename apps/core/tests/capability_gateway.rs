//! `media-capability.v1` 网关集成测试：A1 闸门的四项自动化验证。
//!
//! 覆盖：只读能力往返、幂等重放无二次副作用、过期 revision 稳定拒绝、非法输入（绝对
//! 路径 / shell / SQL / 凭据）拒绝、令牌认证边界，以及事件按 cursor 无缺口补发。

#![allow(clippy::too_many_lines)]

mod common;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
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
                            json!({ "task_id": harness.task_id.to_string(), "dry_run": true }),
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
