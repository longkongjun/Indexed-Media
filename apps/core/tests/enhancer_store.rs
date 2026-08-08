mod common;

use mediaflow_core::automation::model::AutomationFailureCode;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::enhancer::model::{EnhancerConfigInput, EnhancerHealthProjection};
use mediaflow_core::connectors::enhancer::store::EnhancerStore;
use mediaflow_core::connectors::model::IntegrationHealth;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{OutboxNotifier, OutboxReader};
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::tasks::events::TaskEventEnvelope;

#[tokio::test]
async fn migration_seeds_one_disabled_strict_path_and_prompt_free_configuration() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store = EnhancerStore::new(db.pool().clone(), OutboxNotifier::new());

    let current = store.get().await.unwrap();
    assert!(!current.enabled);
    assert_eq!(current.base_url, "http://127.0.0.1:11434");
    assert_eq!(current.endpoint_summary, "http://127.0.0.1:11434");
    assert_eq!(current.model, "qwen3:4b");
    assert_eq!(current.timeout_ms, 3_000);
    assert_eq!(current.config_version, 1);
    assert_eq!(current.health, IntegrationHealth::Degraded);
    assert_eq!(
        current.fallback_code,
        Some(AutomationFailureCode::SourceDisabled)
    );

    let sql = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='identification_enhancer'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(sql.contains("STRICT"));
    let columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('identification_enhancer')",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    for forbidden in [
        "prompt",
        "response",
        "basename",
        "parent",
        "provider_id",
        "path",
    ] {
        assert!(
            columns.iter().all(|column| !column.contains(forbidden)),
            "field={forbidden}"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        30
    );
}

#[tokio::test]
async fn replacement_and_probe_health_are_versioned_and_emit_only_safe_refresh_events() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store = EnhancerStore::new(db.pool().clone(), OutboxNotifier::new());
    let input = candidate(true, "http://localhost:11434", "qwen3:8b");

    let saved = store.replace(1, input, 100).await.unwrap();
    assert!(saved.enabled);
    assert_eq!(saved.config_version, 2);
    assert_eq!(saved.projection_version, 2);
    assert_eq!(saved.health, IntegrationHealth::Degraded);
    assert_eq!(
        saved.fallback_code,
        Some(AutomationFailureCode::IntegrationUnavailable)
    );
    assert_eq!(
        store
            .replace(1, candidate(true, "http://127.0.0.1:11434", "stale"), 101)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigVersionConflict
    );

    let healthy = store
        .commit_probe(
            2,
            EnhancerHealthProjection {
                health: IntegrationHealth::Healthy,
                fallback_code: None,
            },
            200,
        )
        .await
        .unwrap();
    assert_eq!(healthy.projection_version, 3);
    assert_eq!(healthy.checked_at_us, Some(200));
    assert_eq!(healthy.health, IntegrationHealth::Healthy);
    let event_text = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(GROUP_CONCAT(event_type || payload_json), '')
         FROM platform_outbox_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(event_text.contains("identification-enhancer.changed"));
    for forbidden in ["qwen3:8b", "localhost", "prompt", "response", "path"] {
        assert!(!event_text.contains(forbidden), "value={forbidden}");
    }
    let events = OutboxReader::new(db.pool().clone())
        .after(0, 10)
        .await
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TaskEventEnvelope::IdentificationEnhancerChanged { payload, .. }
            if payload.projection_version == 3
                && payload.enabled
                && payload.health == IntegrationHealth::Healthy
                && payload.fallback_code.is_none()
    )));
}

fn candidate(enabled: bool, base_url: &str, model: &str) -> EnhancerConfigInput {
    EnhancerConfigInput {
        enabled,
        base_url: base_url.to_owned(),
        model: model.to_owned(),
        timeout_ms: 3_000,
    }
}
