mod common;
mod downloader_runtime_support;

use std::sync::Arc;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_service::DownloaderRegistry;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::{DownloadTaskStatus, DownloaderFailureCode};
use mediaflow_core::connectors::downloader::port::{
    DownloadSourceError, RemoteDownloadRef, RemoteDownloadSnapshot, RemoteDownloadStatus,
};
use mediaflow_core::connectors::downloader::runtime::DownloadTaskRuntime;
use mediaflow_core::connectors::downloader::task_store::DownloadTaskStore;
use mediaflow_core::connectors::model::IntegrationHealth;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::task_runtime::{ManualTaskClock, TaskClock};
use uuid::Uuid;

use common::TestConfigDir;
use downloader_runtime_support::{FakeDownloadSource, connection_input, task_command};

#[tokio::test]
async fn temporary_failure_preserves_projection_and_a_new_runtime_resumes_when_due() {
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
    let task = tasks
        .accept(
            task_command(connection_id, "Resumable"),
            "recovery-key",
            200,
        )
        .await
        .unwrap();
    let remote = "cccccccccccccccccccccccccccccccccccccccc";
    tasks.claim("pre-restart", 300).await.unwrap().unwrap();
    tasks
        .commit_remote_ref(task.id, "pre-restart", remote, 400)
        .await
        .unwrap();
    tasks
        .commit_snapshot(
            task.id,
            "pre-restart",
            RemoteDownloadStatus::Downloading,
            3_300,
            500,
        )
        .await
        .unwrap();
    let fake = Arc::new(FakeDownloadSource::default());
    fake.fetch_error(DownloadSourceError::Unavailable);
    let first_clock: Arc<dyn TaskClock> = Arc::new(ManualTaskClock::new(31_000_000));
    let registry = DownloaderRegistry::new(fake.clone(), fake.clone());
    let runtime = DownloadTaskRuntime::new(
        db.pool().clone(),
        OutboxNotifier::new(),
        registry.clone(),
        first_clock,
    );
    assert_eq!(runtime.run_once().await.unwrap(), 1);
    let waiting = tasks.get(task.id).await.unwrap().unwrap();
    assert_eq!(waiting.status, DownloadTaskStatus::RetryWait);
    assert_eq!(waiting.progress_basis_points, 3_300);
    assert_eq!(
        waiting.failure_code,
        Some(DownloaderFailureCode::IntegrationUnavailable)
    );

    fake.snapshot(RemoteDownloadSnapshot {
        remote_id: remote.to_owned(),
        status: RemoteDownloadStatus::Completed,
        progress_basis_points: 10_000,
    });
    let retry_at_us = chrono::DateTime::parse_from_rfc3339(waiting.retry_at.as_deref().unwrap())
        .unwrap()
        .timestamp_micros();
    let second_clock: Arc<dyn TaskClock> = Arc::new(ManualTaskClock::new(retry_at_us));
    let restarted = DownloadTaskRuntime::new(
        db.pool().clone(),
        OutboxNotifier::new(),
        registry,
        second_clock,
    );
    assert_eq!(restarted.run_once().await.unwrap(), 1);
    assert_eq!(
        tasks.get(task.id).await.unwrap().unwrap().status,
        DownloadTaskStatus::Completed
    );
}

#[tokio::test]
async fn authorization_failure_blocks_until_the_connection_configuration_changes() {
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
    let task = tasks
        .accept(task_command(connection_id, "Auth blocked"), "auth-key", 200)
        .await
        .unwrap();
    let fake = Arc::new(FakeDownloadSource::default());
    fake.fetch_error(DownloadSourceError::Unauthorized);
    let clock: Arc<dyn TaskClock> = Arc::new(ManualTaskClock::new(500));
    let registry = DownloaderRegistry::new(fake.clone(), fake.clone());
    let runtime = DownloadTaskRuntime::new(
        db.pool().clone(),
        OutboxNotifier::new(),
        registry.clone(),
        clock.clone(),
    );
    assert_eq!(runtime.run_once().await.unwrap(), 1);
    let failed = tasks.get(task.id).await.unwrap().unwrap();
    assert_eq!(failed.status, DownloadTaskStatus::Failed);
    assert_eq!(
        failed.failure_code,
        Some(DownloaderFailureCode::IntegrationUnauthorized)
    );
    assert_eq!(
        connections
            .get(connection_id)
            .await
            .unwrap()
            .unwrap()
            .health,
        IntegrationHealth::Unauthorized
    );
    assert_eq!(runtime.run_once().await.unwrap(), 0);

    connections
        .replace(connection_id, 1, connection_input(), 1_000)
        .await
        .unwrap();
    let resumed =
        DownloadTaskRuntime::new(db.pool().clone(), OutboxNotifier::new(), registry, clock);
    assert_eq!(resumed.prepare().await.unwrap(), 1);
    assert_eq!(
        tasks.get(task.id).await.unwrap().unwrap().status,
        DownloadTaskStatus::Queued
    );
}

#[tokio::test]
async fn a_missing_linked_remote_becomes_a_stable_terminal_failure() {
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
    let task = tasks
        .accept(
            task_command(connection_id, "Missing remote"),
            "missing-key",
            200,
        )
        .await
        .unwrap();
    let fake = Arc::new(FakeDownloadSource::default());
    fake.add_result(Ok(RemoteDownloadRef {
        remote_id: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned(),
    }));
    let clock: Arc<dyn TaskClock> = Arc::new(ManualTaskClock::new(500));
    let registry = DownloaderRegistry::new(fake.clone(), fake);
    let runtime =
        DownloadTaskRuntime::new(db.pool().clone(), OutboxNotifier::new(), registry, clock);
    assert_eq!(runtime.run_once().await.unwrap(), 1);
    let failed = tasks.get(task.id).await.unwrap().unwrap();
    assert_eq!(failed.status, DownloadTaskStatus::Failed);
    assert_eq!(
        failed.failure_code,
        Some(DownloaderFailureCode::RemoteMissing)
    );
}
