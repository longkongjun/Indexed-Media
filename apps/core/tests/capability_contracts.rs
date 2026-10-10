//! `media-capability.v1` 契约回放测试：Rust 信封类型与 JSON Schema golden 示例逐字段
//! 对齐。JSON Schema 是唯一事实源；本测试防止类型层与契约漂移。

use mediaflow_core::capability::model::{
    CapabilityCall, CapabilityError, CapabilityErrorCategory, CapabilityEvent, CapabilityEventPage,
    CapabilityEventType, CapabilityExecutionMode, CapabilityNextAction, CapabilityResult,
    CapabilityResultStatus, PROTOCOL_VERSION,
};
use serde_json::json;

fn example(name: &str) -> serde_json::Value {
    let path = format!("../../contracts/examples/v1/capability/{name}.json");
    let raw = std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{name}: {error}"));
    serde_json::from_str(&raw).unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn read_only_call_round_trips_through_the_rust_envelope() {
    let raw = example("capability-call.read-only");
    let call: CapabilityCall = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(call.protocol_version, PROTOCOL_VERSION);
    assert_eq!(
        call.capability.as_str(),
        raw["capability"].as_str().unwrap()
    );
    assert_eq!(call.execution.mode, CapabilityExecutionMode::ReadOnly);
    assert!(call.execution.expected_revision.is_some());
    let serialized = serde_json::to_value(&call).unwrap();
    assert_eq!(serialized, raw);
}

#[test]
fn plan_only_call_keeps_mode_and_idempotency_key() {
    let raw = example("capability-call.plan-only");
    let call: CapabilityCall = serde_json::from_value(raw).unwrap();
    assert_eq!(call.execution.mode, CapabilityExecutionMode::PlanOnly);
    assert_eq!(call.input["dry_run"], json!(true));
}

#[test]
fn succeeded_and_needs_approval_results_round_trip() {
    for name in [
        "capability-result.succeeded",
        "capability-result.needs-approval",
    ] {
        let raw = example(name);
        let result: CapabilityResult = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(result.protocol_version, PROTOCOL_VERSION);
        let serialized = serde_json::to_value(&result).unwrap();
        assert_eq!(serialized, raw);
    }
    let needs: CapabilityResult =
        serde_json::from_value(example("capability-result.needs-approval")).unwrap();
    assert_eq!(needs.status, CapabilityResultStatus::NeedsApproval);
    assert_eq!(
        needs.next_actions,
        vec![
            CapabilityNextAction::RequestApproval,
            CapabilityNextAction::Cancel
        ]
    );
}

#[test]
fn stale_revision_rejection_carries_structured_error() {
    let raw = example("capability-result.rejected.stale-revision");
    let result: CapabilityResult = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(result.status, CapabilityResultStatus::Rejected);
    let error = result.error.as_ref().unwrap();
    assert_eq!(error.error_code, "capability.revision-stale");
    assert_eq!(error.category, CapabilityErrorCategory::Conflict);
    assert!(!error.retryable);
    assert_eq!(serialized_error(error), raw["error"]);
}

#[test]
fn capability_events_round_trip_and_keep_sequence_ordering() {
    for name in [
        "capability-event.waiting-approval",
        "capability-event.external-sync-retryable",
    ] {
        let raw = example(name);
        let event: CapabilityEvent = serde_json::from_value(raw.clone()).unwrap();
        assert!(event.sequence >= 1);
        assert_eq!(serde_json::to_value(&event).unwrap(), raw);
    }
    let retryable: CapabilityEvent =
        serde_json::from_value(example("capability-event.external-sync-retryable")).unwrap();
    assert_eq!(
        retryable.event_type,
        CapabilityEventType::ExternalSyncRetryable
    );
    assert!(retryable.error.as_ref().unwrap().retryable);
}

#[test]
fn event_page_round_trips_with_monotonic_sequences() {
    let raw = example("capability-event-page");
    let page: CapabilityEventPage = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(page.items[0].sequence < page.items[1].sequence);
    assert_eq!(page.next_cursor.as_deref(), Some("capability-events:43"));
    assert_eq!(serde_json::to_value(&page).unwrap(), raw);
}

#[test]
fn unknown_fields_and_protocol_versions_are_rejected() {
    let raw = example("capability-call.read-only");
    let mut unknown_field = raw.clone();
    unknown_field["unexpected"] = json!("field");
    assert!(serde_json::from_value::<CapabilityCall>(unknown_field).is_err());

    let mut wrong_version = raw;
    wrong_version["protocol_version"] = json!("media-capability.v2");
    assert!(serde_json::from_value::<CapabilityCall>(wrong_version).is_err());

    let mut unknown_event_type = example("capability-event.waiting-approval");
    unknown_event_type["event_type"] = json!("task.exploded");
    assert!(serde_json::from_value::<CapabilityEvent>(unknown_event_type).is_err());
}

#[test]
fn absolute_paths_and_credentials_cannot_hide_in_free_form_error_details() {
    // 契约允许 safe_details 为自由对象，但脱敏是网关与序列化层职责：
    // 这里钉住错误信封不携带专门的路径/凭据字段，脱敏断言随 A1 网关落地。
    let raw = example("capability-result.rejected.stale-revision");
    let result: CapabilityResult = serde_json::from_value(raw).unwrap();
    let error: CapabilityError = result.error.unwrap();
    let fields = serde_json::to_value(&error).unwrap();
    for forbidden in ["path", "secret", "token", "password", "credential"] {
        assert!(
            !fields.as_object().unwrap().contains_key(forbidden),
            "capability error envelope must not expose {forbidden} fields"
        );
    }
}

fn serialized_error(error: &CapabilityError) -> serde_json::Value {
    serde_json::to_value(error).unwrap()
}
