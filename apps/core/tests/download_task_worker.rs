mod common;
mod downloader_runtime_support;

use std::sync::Arc;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_service::DownloaderRegistry;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::DownloadTaskStatus;
use mediaflow_core::connectors::downloader::port::{RemoteDownloadSnapshot, RemoteDownloadStatus};
use mediaflow_core::connectors::downloader::runtime::DownloadTaskRuntime;
use mediaflow_core::connectors::downloader::task_store::DownloadTaskStore;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::task_runtime::{ManualTaskClock, TaskClock};
use uuid::Uuid;

use common::TestConfigDir;
use downloader_runtime_support::{FakeDownloadSource, connection_input, task_command};

#[tokio::test]
async fn recovery_queries_correlation_before_add_and_batches_linked_monitoring() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let connections =
        DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let tasks = DownloadTaskStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let connection_id = Uuid::now_v7();
    connections
        .insert(connection_id, connection_input(), 100)
        .await
        .unwrap();
    let linked = tasks
        .accept(task_command(connection_id, "Linked"), "worker-linked", 200)
        .await
        .unwrap();
    let recovered = tasks
        .accept(
            task_command(connection_id, "Recovered"),
            "worker-recovered",
            201,
        )
        .await
        .unwrap();
    tasks.claim("crashed-worker", 300).await.unwrap().unwrap();
    let linked_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    tasks
        .commit_remote_ref(linked.id, "crashed-worker", linked_hash, 400)
        .await
        .unwrap();

    let recovered_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let fake = Arc::new(FakeDownloadSource::default());
    fake.correlation(
        format!("mediaflow-{}", recovered.id),
        vec![RemoteDownloadSnapshot {
            remote_id: recovered_hash.to_owned(),
            status: RemoteDownloadStatus::Downloading,
            progress_basis_points: 1_000,
        }],
    );
    fake.snapshot(RemoteDownloadSnapshot {
        remote_id: linked_hash.to_owned(),
        status: RemoteDownloadStatus::Downloading,
        progress_basis_points: 2_500,
    });
    fake.snapshot(RemoteDownloadSnapshot {
        remote_id: recovered_hash.to_owned(),
        status: RemoteDownloadStatus::Completed,
        progress_basis_points: 10_000,
    });
    let clock: Arc<dyn TaskClock> = Arc::new(ManualTaskClock::new(31_000_000));
    let registry = DownloaderRegistry::new(fake.clone(), fake.clone());
    let runtime =
        DownloadTaskRuntime::new(db.pool().clone(), OutboxNotifier::new(), registry, clock);
    assert_eq!(runtime.run_once().await.unwrap(), 2);
    assert_eq!(fake.add_count(), 0);
    assert_eq!(
        tasks
            .get(linked.id)
            .await
            .unwrap()
            .unwrap()
            .progress_basis_points,
        2_500
    );
    assert_eq!(
        tasks.get(recovered.id).await.unwrap().unwrap().status,
        DownloadTaskStatus::Completed
    );
    let queries = fake.queries();
    assert!(queries.iter().any(|query| query.correlation_tag.is_some()));
    assert!(queries.iter().any(|query| query.remote_ids.len() == 2));
}
