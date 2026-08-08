mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::port::RemoteDownloadStatus;
use mediaflow_core::connectors::downloader::task_store::DownloadTaskStore;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{OutboxNotifier, OutboxReader};
use mediaflow_core::tasks::events::TaskEventEnvelope;
use uuid::Uuid;

use common::TestConfigDir;

#[tokio::test]
async fn equivalent_remote_snapshots_keep_projection_version_and_outbox_count_stable() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let notifier = OutboxNotifier::new();
    let connections =
        DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let tasks = DownloadTaskStore::open_with_notifier(
        db.pool().clone(),
        &fixture.config().config_dir,
        notifier,
    )
    .unwrap();
    let connection_id = Uuid::now_v7();
    connections
        .insert(connection_id, crate::common_connection_input(), 100)
        .await
        .unwrap();
    let task = tasks
        .accept(crate::common_task_command(connection_id), "events-key", 200)
        .await
        .unwrap();
    tasks.claim("event-worker", 300).await.unwrap().unwrap();
    tasks
        .commit_remote_ref(
            task.id,
            "event-worker",
            "dddddddddddddddddddddddddddddddddddddddd",
            400,
        )
        .await
        .unwrap();
    let first = tasks
        .commit_snapshot(
            task.id,
            "event-worker",
            RemoteDownloadStatus::Downloading,
            4_000,
            500,
        )
        .await
        .unwrap();
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM platform_outbox_events WHERE event_type='download-task.changed'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let reclaimed = tasks
        .claim("event-recovered", 31_000_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reclaimed.task_id, task.id);
    let second = tasks
        .commit_snapshot(
            task.id,
            "event-recovered",
            RemoteDownloadStatus::Downloading,
            4_000,
            31_000_100,
        )
        .await
        .unwrap();
    assert_eq!(second.projection_version, first.projection_version);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM platform_outbox_events WHERE event_type='download-task.changed'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        count
    );
    let events = OutboxReader::new(db.pool().clone())
        .after(0, 20)
        .await
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TaskEventEnvelope::DownloadTaskChanged { task_id, payload, .. }
            if *task_id == task.id && payload.progress_basis_points == 4_000
    )));
}

fn common_connection_input()
-> mediaflow_core::connectors::downloader::model::DownloaderConnectionInput {
    mediaflow_core::connectors::downloader::model::DownloaderConnectionInput {
        kind: mediaflow_core::connectors::downloader::model::DownloaderKind::Qbittorrent,
        display_name: "Event qBit".to_owned(),
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
            "magnet:?xt=urn:btih:EVENT_SOURCE_MUST_NOT_LEAK".to_owned(),
        ),
        display_name: "Event task".to_owned(),
    }
}
