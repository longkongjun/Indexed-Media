mod common;

use std::sync::Arc;

use async_trait::async_trait;
use mediaflow_core::automation::completion::DownloadCompletionDispatcher;
use mediaflow_core::automation::event_store::AutomationEventStore;
use mediaflow_core::automation::model::{AutomationEventStatus, AutomationSourceInput};
use mediaflow_core::automation::source_store::AutomationSourceStore;
use mediaflow_core::automation::worker::{
    AutomationActionError, AutomationWorker, DownloadTaskCreationPort,
};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::port::RemoteDownloadStatus;
use mediaflow_core::connectors::downloader::task_store::DownloadTaskStore;
use mediaflow_core::discovery::reconcile_service::ReconcileRequestService;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use uuid::Uuid;

struct NoDownloads;

#[async_trait]
impl DownloadTaskCreationPort for NoDownloads {
    async fn create_download(
        &self,
        _: Uuid,
        _: &[u8],
        _: &str,
        _: &str,
    ) -> Result<Uuid, AutomationActionError> {
        Err(AutomationActionError::unavailable())
    }
}

#[tokio::test]
async fn first_completion_dispatches_one_mapping_event_and_one_reconcile_request() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox_id = common::seed_inbox(db.pool()).await;
    let (connection_id, task_id) =
        complete_download(&db, &fixture, "mapped-completion", 1_000).await;
    let source_id = Uuid::now_v7();
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            source_id,
            AutomationSourceInput::DownloadCompletion {
                display_name: "Completed downloads".to_owned(),
                enabled: true,
                downloader_connection_id: connection_id,
                inbox_directory_id: inbox_id,
            },
            None,
            1_100,
        )
        .await
        .unwrap();
    let dispatcher = DownloadCompletionDispatcher::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        OutboxNotifier::new(),
    )
    .unwrap();

    let dispatch = dispatcher.run_once(1_200).await.unwrap().unwrap();
    assert_eq!(dispatch.download_task_id, task_id);
    let event_id = dispatch.event_id.unwrap();
    assert!(dispatcher.run_once(1_201).await.unwrap().is_none());
    let event = AutomationEventStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .get(event_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.source_id, source_id);
    assert_eq!(event.status, AutomationEventStatus::Pending);

    let reconciles = Arc::new(ReconcileRequestService::new(
        db.pool().clone(),
        OutboxNotifier::new(),
    ));
    let clock = Arc::new(ManualTaskClock::new(1_300));
    let worker = AutomationWorker::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(NoDownloads),
        reconciles.clone(),
        clock.clone(),
    )
    .unwrap();
    let lease = worker.store().claim_one(1_300, 50).await.unwrap().unwrap();
    assert_eq!(lease.id, event_id);
    let accepted_before_checkpoint = reconciles
        .accept(inbox_id, &format!("automation:{event_id}"), 1_301)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE tasks_scan_tasks SET status='completed',stage='finished',observed_files=7,
         errors=0,updated_at_us=1302 WHERE id=?",
    )
    .bind(
        accepted_before_checkpoint
            .scan_task_id
            .as_bytes()
            .as_slice(),
    )
    .execute(db.pool())
    .await
    .unwrap();
    drop(lease);
    clock.set(1_350);
    assert_eq!(worker.prepare().await.unwrap(), 1);
    assert_eq!(worker.run_once().await.unwrap(), Some(event_id));
    let completed = worker.store().get(event_id).await.unwrap().unwrap();
    assert_eq!(completed.status, AutomationEventStatus::Completed);
    assert_eq!(
        completed.downstream_kind.as_deref(),
        Some("reconcile-request")
    );
    assert_eq!(completed.downstream_id, Some(accepted_before_checkpoint.id));
    assert_eq!(completed.result_count, Some(7));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_reconcile_requests")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert_completion_schema_path_free(db.pool()).await;
}

#[tokio::test]
async fn claimed_completion_recovers_after_a_restart_boundary() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox_id = common::seed_inbox(db.pool()).await;
    let (connection_id, task_id) =
        complete_download(&db, &fixture, "restart-completion", 3_000).await;
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            Uuid::now_v7(),
            AutomationSourceInput::DownloadCompletion {
                display_name: "Restart mapping".to_owned(),
                enabled: true,
                downloader_connection_id: connection_id,
                inbox_directory_id: inbox_id,
            },
            None,
            3_100,
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE download_completion_signals SET status='running',attempt_count=1,
         lease_token=randomblob(16),lease_expires_at_us=3200 WHERE download_task_id=?",
    )
    .bind(task_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let restarted = DownloadCompletionDispatcher::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        OutboxNotifier::new(),
    )
    .unwrap();

    assert_eq!(restarted.prepare(3_200).await.unwrap(), 1);
    let dispatch = restarted.run_once(3_201).await.unwrap().unwrap();
    assert_eq!(dispatch.download_task_id, task_id);
    assert!(dispatch.event_id.is_some());
    assert!(restarted.run_once(3_202).await.unwrap().is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM automation_events WHERE action='reconcile-inbox'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn inbox_disabled_after_event_acceptance_retries_without_scanning_another_directory() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox_id = common::seed_inbox(db.pool()).await;
    let (connection_id, _task_id) =
        complete_download(&db, &fixture, "disabled-target", 4_000).await;
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            Uuid::now_v7(),
            AutomationSourceInput::DownloadCompletion {
                display_name: "Disabled target mapping".to_owned(),
                enabled: true,
                downloader_connection_id: connection_id,
                inbox_directory_id: inbox_id,
            },
            None,
            4_100,
        )
        .await
        .unwrap();
    let dispatcher = DownloadCompletionDispatcher::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        OutboxNotifier::new(),
    )
    .unwrap();
    let event_id = dispatcher
        .run_once(4_200)
        .await
        .unwrap()
        .unwrap()
        .event_id
        .unwrap();
    sqlx::query("UPDATE discovery_inbox_directories SET health='unavailable' WHERE id=?")
        .bind(inbox_id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let worker = AutomationWorker::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(NoDownloads),
        Arc::new(ReconcileRequestService::new(
            db.pool().clone(),
            OutboxNotifier::new(),
        )),
        Arc::new(ManualTaskClock::new(4_300)),
    )
    .unwrap();

    assert_eq!(worker.run_once().await.unwrap(), Some(event_id));
    let event = worker.store().get(event_id).await.unwrap().unwrap();
    assert_eq!(event.status, AutomationEventStatus::RetryWait);
    assert_eq!(
        event.failure_code,
        Some(mediaflow_core::automation::model::AutomationFailureCode::IntegrationUnavailable)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_reconcile_requests")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_scan_tasks")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn failed_download_outbox_commit_rolls_back_completion_and_signal_together() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let connection_id = Uuid::now_v7();
    DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(connection_id, common_connection_input(), 5_000)
        .await
        .unwrap();
    let tasks = DownloadTaskStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let task = tasks
        .accept(
            common_task_command(connection_id),
            "atomic-completion",
            5_001,
        )
        .await
        .unwrap();
    tasks.claim("atomic-worker", 5_002).await.unwrap();
    tasks
        .commit_remote_ref(
            task.id,
            "atomic-worker",
            "ffffffffffffffffffffffffffffffffffffffff",
            5_003,
        )
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_completion_outbox BEFORE INSERT ON platform_outbox_events
         WHEN NEW.event_type='download-task.changed'
         BEGIN SELECT RAISE(ABORT,'injected completion outbox failure'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();
    assert!(
        tasks
            .commit_snapshot(
                task.id,
                "atomic-worker",
                RemoteDownloadStatus::Completed,
                10_000,
                5_004,
            )
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM download_tasks WHERE id=?")
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "monitoring"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM download_completion_signals")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER fail_completion_outbox")
        .execute(db.pool())
        .await
        .unwrap();
    tasks
        .commit_snapshot(
            task.id,
            "atomic-worker",
            RemoteDownloadStatus::Completed,
            10_000,
            5_005,
        )
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM download_completion_signals")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn no_mapping_finishes_the_signal_without_guessing_an_inbox() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let (_connection_id, task_id) =
        complete_download(&db, &fixture, "unmapped-completion", 2_000).await;
    let dispatcher = DownloadCompletionDispatcher::open(
        db.pool().clone(),
        &fixture.config().config_dir,
        OutboxNotifier::new(),
    )
    .unwrap();

    let result = dispatcher.run_once(2_100).await.unwrap().unwrap();
    assert_eq!(result.download_task_id, task_id);
    assert_eq!(result.event_id, None);
    assert_eq!(
        sqlx::query_as::<_, (String, String)>(
            "SELECT status,outcome FROM download_completion_signals WHERE download_task_id=?",
        )
        .bind(task_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        ("completed".to_owned(), "no-mapping".to_owned())
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM automation_events")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

async fn complete_download(
    db: &mediaflow_core::platform::db::Db,
    fixture: &common::TestConfigDir,
    key: &str,
    now_us: i64,
) -> (Uuid, Uuid) {
    let connection_id = Uuid::now_v7();
    DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(connection_id, common_connection_input(), now_us)
        .await
        .unwrap();
    let tasks = DownloadTaskStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let task = tasks
        .accept(common_task_command(connection_id), key, now_us + 1)
        .await
        .unwrap();
    tasks.claim("completion-worker", now_us + 2).await.unwrap();
    tasks
        .commit_remote_ref(
            task.id,
            "completion-worker",
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            now_us + 3,
        )
        .await
        .unwrap();
    tasks
        .commit_snapshot(
            task.id,
            "completion-worker",
            RemoteDownloadStatus::Completed,
            10_000,
            now_us + 4,
        )
        .await
        .unwrap();
    (connection_id, task.id)
}

fn common_connection_input()
-> mediaflow_core::connectors::downloader::model::DownloaderConnectionInput {
    mediaflow_core::connectors::downloader::model::DownloaderConnectionInput {
        kind: mediaflow_core::connectors::downloader::model::DownloaderKind::Qbittorrent,
        display_name: "Completion qBit".to_owned(),
        base_url: "https://download.test/qbit".to_owned(),
        username: mediaflow_core::connectors::model::SecretString::new("user".to_owned()),
        password: mediaflow_core::connectors::model::SecretString::new("password".to_owned()),
        enabled: true,
    }
}

fn common_task_command(
    connection_id: Uuid,
) -> mediaflow_core::connectors::downloader::model::CreateDownloadTaskCommand {
    mediaflow_core::connectors::downloader::model::CreateDownloadTaskCommand {
        connection_id,
        source: mediaflow_core::connectors::model::SecretString::new(
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_owned(),
        ),
        display_name: "Completion task".to_owned(),
    }
}

async fn assert_completion_schema_path_free(pool: &sqlx::SqlitePool) {
    let schema = sqlx::query_scalar::<_, String>(
        "SELECT sql FROM sqlite_schema WHERE type='table' AND name='download_completion_signals'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .to_ascii_lowercase();
    for forbidden in ["path", "source", "remote_id"] {
        assert!(!schema.contains(forbidden), "{forbidden}");
    }
}
