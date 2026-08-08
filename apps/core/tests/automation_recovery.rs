mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mediaflow_core::automation::model::AutomationEventStatus;
use mediaflow_core::automation::worker::{
    AutomationActionError, AutomationWorker, DownloadTaskCreationPort, InboxReconcilePort,
    InboxReconcileResult,
};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use std::time::Instant;
use uuid::Uuid;

#[derive(Default)]
struct IdempotentDownloads {
    calls: Mutex<usize>,
    id: Mutex<Option<Uuid>>,
}

#[async_trait]
impl DownloadTaskCreationPort for IdempotentDownloads {
    async fn create_download(
        &self,
        _connection_id: Uuid,
        _source: &[u8],
        _display_name: &str,
        _idempotency_key: &str,
    ) -> Result<Uuid, AutomationActionError> {
        *self.calls.lock().unwrap() += 1;
        let mut id = self.id.lock().unwrap();
        Ok(*id.get_or_insert_with(Uuid::now_v7))
    }
}

struct NoReconcile;

#[async_trait]
impl InboxReconcilePort for NoReconcile {
    async fn reconcile_inbox(
        &self,
        _: Uuid,
        _: &str,
    ) -> Result<InboxReconcileResult, AutomationActionError> {
        Err(AutomationActionError::unavailable())
    }
}

#[tokio::test]
async fn expired_lease_replays_the_same_idempotency_key_and_converges_to_one_downstream_fact() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let event_id = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let downloads = Arc::new(IdempotentDownloads::default());
    let clock = Arc::new(ManualTaskClock::new(100));
    let worker = AutomationWorker::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        downloads.clone(),
        Arc::new(NoReconcile),
        clock.clone(),
    )
    .unwrap();

    let lease = worker.store().claim_one(100, 50).await.unwrap().unwrap();
    let first_downstream = downloads
        .create_download(
            lease.payload.connection_id().unwrap(),
            lease.payload.source().unwrap(),
            lease.payload.display_name().unwrap(),
            &format!("automation:{event_id}"),
        )
        .await
        .unwrap();
    drop(lease);
    clock.set(150);
    assert_eq!(worker.prepare().await.unwrap(), 1);
    assert_eq!(worker.run_once().await.unwrap(), Some(event_id));
    let event = worker.store().get(event_id).await.unwrap().unwrap();
    assert_eq!(event.status, AutomationEventStatus::Completed);
    assert_eq!(event.downstream_id, Some(first_downstream));
    assert_eq!(*downloads.calls.lock().unwrap(), 2);
}

#[tokio::test]
async fn startup_reorders_one_thousand_expired_events_without_external_waits() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let template = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    sqlx::query(
        "WITH RECURSIVE sequence(value) AS (
           SELECT 1 UNION ALL SELECT value+1 FROM sequence WHERE value<1000
         )
         INSERT INTO automation_events
         (id,source_id,source_display_name,source_config_version,action,dedup_key_sha256,
          request_sha256,payload_schema_version,payload_nonce,payload_ciphertext,status,
          downstream_kind,downstream_id,result_count,failure_code,attempt_count,retry_at_us,
          lease_token,lease_expires_at_us,projection_version,created_at_us,updated_at_us)
         SELECT randomblob(16),source_id,source_display_name,source_config_version,action,
                randomblob(32),request_sha256,payload_schema_version,payload_nonce,payload_ciphertext,
                'running',NULL,NULL,NULL,NULL,1,NULL,randomblob(16),99,2,value+10,value+10
         FROM automation_events,sequence WHERE id=?",
    )
    .bind(template.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let worker = AutomationWorker::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(IdempotentDownloads::default()),
        Arc::new(NoReconcile),
        Arc::new(ManualTaskClock::new(100)),
    )
    .unwrap();

    let started = Instant::now();
    assert_eq!(worker.prepare().await.unwrap(), 1000);
    let elapsed = started.elapsed();
    eprintln!(
        "automation_recovery_1000_elapsed_ms={}",
        elapsed.as_millis()
    );
    assert!(elapsed < std::time::Duration::from_secs(10), "{elapsed:?}");
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM automation_events WHERE status='retry-wait'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1000
    );
    let outbox_before = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM platform_outbox_events WHERE event_type='automation-event.changed'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(outbox_before, 1000);
    assert_eq!(worker.prepare().await.unwrap(), 0);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM platform_outbox_events WHERE event_type='automation-event.changed'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        outbox_before
    );
}
