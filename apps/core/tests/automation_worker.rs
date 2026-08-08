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
use mediaflow_core::platform::task_runtime::{ManualTaskClock, TaskClock};
use uuid::Uuid;

#[derive(Default)]
struct FakeDownloads {
    keys: Mutex<Vec<String>>,
    id: Mutex<Option<Uuid>>,
}

#[async_trait]
impl DownloadTaskCreationPort for FakeDownloads {
    async fn create_download(
        &self,
        _connection_id: Uuid,
        _source: &[u8],
        _display_name: &str,
        idempotency_key: &str,
    ) -> Result<Uuid, AutomationActionError> {
        self.keys.lock().unwrap().push(idempotency_key.to_owned());
        let mut id = self.id.lock().unwrap();
        Ok(*id.get_or_insert_with(Uuid::now_v7))
    }
}

struct NoReconcile;

#[async_trait]
impl InboxReconcilePort for NoReconcile {
    async fn reconcile_inbox(
        &self,
        _inbox_id: Uuid,
        _idempotency_key: &str,
    ) -> Result<InboxReconcileResult, AutomationActionError> {
        Err(AutomationActionError::unavailable())
    }
}

#[derive(Default)]
struct NeverCalledDownloads {
    calls: Mutex<usize>,
}

#[async_trait]
impl DownloadTaskCreationPort for NeverCalledDownloads {
    async fn create_download(
        &self,
        _: Uuid,
        _: &[u8],
        _: &str,
        _: &str,
    ) -> Result<Uuid, AutomationActionError> {
        *self.calls.lock().unwrap() += 1;
        Err(AutomationActionError::unavailable())
    }
}

#[tokio::test]
async fn worker_executes_a_fixed_download_action_and_commits_the_downstream_relation() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let event_id = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let downloads = Arc::new(FakeDownloads::default());
    let worker = AutomationWorker::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        downloads.clone(),
        Arc::new(NoReconcile),
        Arc::new(ManualTaskClock::new(100)),
    )
    .unwrap();

    assert_eq!(worker.run_once().await.unwrap(), Some(event_id));
    let event = worker.store().get(event_id).await.unwrap().unwrap();
    assert_eq!(event.status, AutomationEventStatus::Completed);
    assert_eq!(event.downstream_kind.as_deref(), Some("download-task"));
    assert_eq!(
        downloads.keys.lock().unwrap().as_slice(),
        [format!("automation:{event_id}")]
    );
}

#[tokio::test]
async fn unknown_action_is_terminally_quarantined_without_arbitrary_dispatch() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let event_id = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let mut connection = db.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA ignore_check_constraints=ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("UPDATE automation_events SET action='run-script' WHERE id=?")
        .bind(event_id.as_bytes().as_slice())
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    let downloads = Arc::new(NeverCalledDownloads::default());
    let worker = AutomationWorker::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        downloads.clone(),
        Arc::new(NoReconcile),
        Arc::new(ManualTaskClock::new(100)),
    )
    .unwrap();

    assert_eq!(worker.run_once().await.unwrap(), None);
    assert_eq!(*downloads.calls.lock().unwrap(), 0);
    let row = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT status,failure_code,attempt_count FROM automation_events WHERE id=?",
    )
    .bind(event_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        row,
        (
            "failed".to_owned(),
            "automation.action-invalid".to_owned(),
            1
        )
    );
}

#[tokio::test]
async fn recoverable_failures_stop_after_ten_bounded_attempts() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let event_id = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let downloads = Arc::new(NeverCalledDownloads::default());
    let clock = Arc::new(ManualTaskClock::new(1_000_000_000_000));
    let worker = AutomationWorker::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        downloads.clone(),
        Arc::new(NoReconcile),
        clock.clone(),
    )
    .unwrap();

    for attempt in 1..=10 {
        assert_eq!(worker.run_once().await.unwrap(), Some(event_id));
        let event = worker.store().get(event_id).await.unwrap().unwrap();
        assert_eq!(event.attempt_count, attempt);
        if attempt < 10 {
            assert_eq!(event.status, AutomationEventStatus::RetryWait);
            clock.set(clock.now_us() + 86_400_000_000);
        } else {
            assert_eq!(event.status, AutomationEventStatus::Failed);
        }
    }
    assert_eq!(*downloads.calls.lock().unwrap(), 10);
    assert_eq!(worker.run_once().await.unwrap(), None);
}
