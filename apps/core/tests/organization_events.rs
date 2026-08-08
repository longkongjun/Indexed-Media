mod common;

use common::organization::organization_fixture;
use mediaflow_core::discovery::model::{DeploymentRootView, RootAccess};
use mediaflow_core::organization::executor::{ExecutionOutcome, OrganizationExecutor};
use mediaflow_core::organization::fs::ProcessingStopToken;
use mediaflow_core::organization::journal_store::JournalStore;
use mediaflow_core::organization::model::{OrganizationOperation, OrganizationTargetInput};
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::organization::target_store::{
    OrganizationTargetBoundary, OrganizationTargetStore,
};
use mediaflow_core::platform::outbox::OutboxReader;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::tasks::events::TaskEventEnvelope;
use std::sync::Arc;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn target_and_result_events_are_minimal_and_equal_result_replay_is_silent() {
    let fixture = organization_fixture(OrganizationOperation::Copy).await;
    let before = OutboxReader::new(fixture.db.pool().clone())
        .after(0, 200)
        .await
        .unwrap();
    assert!(before.iter().any(|event| matches!(
        event,
        TaskEventEnvelope::OrganizationTargetChanged { payload, .. }
            if payload.target_id == fixture.plan.draft.target.id
                && payload.config_version == 1
    )));
    let target = fixture.plan.draft.target.clone();
    let unchanged = OrganizationTargetStore::new(fixture.db.pool().clone())
        .replace(
            fixture.account_id,
            target.id,
            target.config_version,
            OrganizationTargetInput {
                kind: target.kind,
                display_name: target.display_name.clone(),
                root_id: target.root_id.clone(),
                relative_path: target.relative_path.clone(),
                operation: target.operation,
                naming_pattern: target.naming_pattern,
                nfo_policy: target.nfo_policy,
                automatic: target.automatic,
                enabled: target.enabled,
                rules: target.rules.clone(),
            },
            &OrganizationTargetBoundary {
                root: DeploymentRootView {
                    id: target.root_id.clone(),
                    label: "Library".to_owned(),
                    access: RootAccess::ReadWrite,
                },
                overlaps_inbox: false,
            },
            101,
        )
        .await
        .unwrap();
    assert_eq!(unchanged.config_version, 1);
    assert_eq!(
        OutboxReader::new(fixture.db.pool().clone())
            .after(0, 200)
            .await
            .unwrap()
            .iter()
            .filter(|event| matches!(event, TaskEventEnvelope::OrganizationTargetChanged { .. }))
            .count(),
        1
    );

    let executor = OrganizationExecutor::new(
        fixture.account_id,
        OrganizationPlanStore::new(fixture.db.pool().clone()),
        JournalStore::new(fixture.db.pool().clone()),
        fixture.fs.clone(),
        Arc::new(ManualTaskClock::new(100_000_000)),
    );
    let outcome = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(outcome, ExecutionOutcome::Completed(_)));
    let after_first = OutboxReader::new(fixture.db.pool().clone())
        .after(0, 200)
        .await
        .unwrap();
    let result_events = after_first
        .iter()
        .filter(|event| matches!(event, TaskEventEnvelope::OrganizationResultChanged { .. }))
        .count();
    assert_eq!(result_events, 1);

    let replay = executor
        .run_next(&fixture.lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert!(matches!(replay, ExecutionOutcome::Completed(_)));
    let after_replay = OutboxReader::new(fixture.db.pool().clone())
        .after(0, 200)
        .await
        .unwrap();
    assert_eq!(
        after_replay
            .iter()
            .filter(|event| matches!(event, TaskEventEnvelope::OrganizationResultChanged { .. }))
            .count(),
        result_events
    );

    let raw = sqlx::query_scalar::<_, String>(
        "SELECT payload_json FROM platform_outbox_events
         WHERE event_type IN ('organization-target.changed','organization-result.changed')
         ORDER BY id",
    )
    .fetch_all(fixture.db.pool())
    .await
    .unwrap()
    .join("\n");
    assert!(!raw.contains("Arrival"));
    assert!(!raw.contains("ready/"));
    assert!(!raw.contains("<movie>"));
    assert!(!raw.contains(fixture.source_root.to_string_lossy().as_ref()));
}
