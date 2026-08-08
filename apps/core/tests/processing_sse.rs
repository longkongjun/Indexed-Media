mod common;

use std::sync::Arc;

use async_trait::async_trait;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{
    EpisodeIdentity, MetadataCandidate, ProviderError, ProviderMediaKind, SecretString,
    TmdbConfigCommand, TmdbExternalIdRequest, TmdbSearchRequest,
};
use mediaflow_core::connectors::service::{
    ConnectorService, MetadataProvider, TmdbConnectionTester,
};
use mediaflow_core::identification::parser::FilenameParser;
use mediaflow_core::identification::service::IdentificationService;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{OutboxNotifier, OutboxReader};
use mediaflow_core::platform::secrets::SecretBytes;
use mediaflow_core::tasks::events::TaskEventEnvelope;

const SECRET: &str = "processing-sse-secret-sentinel";

#[tokio::test]
async fn decision_and_integration_events_are_minimal_committed_and_replayable() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/unknown.mkv",
        vec![66],
        "sse-worker",
    )
    .await;
    let hint = FilenameParser::default()
        .parse("movies/unknown.mkv")
        .unwrap();
    let notifier = OutboxNotifier::new();
    let outcome = IdentificationService::new_with_notifier(db.pool().clone(), notifier.clone())
        .identify(
            &lease,
            &hint,
            &[],
            &EmptyProvider,
            &SecretBytes::new(SECRET.as_bytes().to_vec()),
            "zh-CN",
            None,
            93_000_000,
        )
        .await
        .unwrap();
    let connectors = ConnectorService::new_with_notifier(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(UnusedTester),
        notifier,
    );
    connectors
        .put_tmdb(
            TmdbConfigCommand {
                api_read_access_token: SecretString::new(SECRET.to_owned()),
                locale: "zh-CN".to_owned(),
                region: None,
            },
            0,
        )
        .await
        .unwrap();
    connectors
        .record_tmdb_health(None, 94_000_000)
        .await
        .unwrap();

    let reader = OutboxReader::new(db.pool().clone());
    let events = reader.after(0, 50).await.unwrap();
    let decision = events
        .iter()
        .find(|event| {
            matches!(
                event,
                TaskEventEnvelope::ProcessingTaskIdentificationDecided { task_id, .. }
                    if *task_id == lease.task.id
            )
        })
        .expect("identification decision event");
    assert!(matches!(
        decision,
        TaskEventEnvelope::ProcessingTaskIdentificationDecided { payload, .. }
            if Some(payload.decision_id) == outcome.decision_id
                && payload.level.as_str() == "unidentified"
    ));
    assert!(events.iter().any(|event| matches!(
        event,
        TaskEventEnvelope::IntegrationHealthChanged { payload, .. }
            if payload.kind == "tmdb" && payload.health == mediaflow_core::connectors::model::IntegrationHealth::Healthy
    )));

    let decision_id = decision.id();
    for _ in 0..2 {
        let replay = reader
            .replay(Some(decision_id - 1), 50, 95_000_000)
            .await
            .unwrap();
        assert!(replay.gap.is_none());
        assert_eq!(replay.events.first().unwrap().id(), decision_id);
    }
    let payloads = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(GROUP_CONCAT(payload_json, ''), '')
         FROM platform_outbox_events
         WHERE event_type IN ('processing-task.identification-decided',
                              'integration.health-changed')",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!payloads.contains(SECRET));
    for forbidden in ["evidence", "candidates", "source_hash", "absolute_path"] {
        assert!(
            !payloads.contains(forbidden),
            "forbidden event field: {forbidden}"
        );
    }
}

struct UnusedTester;

#[async_trait]
impl TmdbConnectionTester for UnusedTester {
    async fn test(
        &self,
        _token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
}

struct EmptyProvider;

#[async_trait]
impl MetadataProvider for EmptyProvider {
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
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(Vec::new())
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Err(ProviderError::InvalidResponse)
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
