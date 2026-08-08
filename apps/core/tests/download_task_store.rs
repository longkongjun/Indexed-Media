mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::{
    CreateDownloadTaskCommand, DownloadTaskFilter, DownloadTaskStatus, DownloaderConnectionInput,
    DownloaderKind,
};
use mediaflow_core::connectors::downloader::port::RemoteDownloadStatus;
use mediaflow_core::connectors::downloader::task_store::DownloadTaskStore;
use mediaflow_core::connectors::model::SecretString;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use uuid::Uuid;

use common::TestConfigDir;

const SOURCE: &str = "magnet:?xt=urn:btih:SOURCE_MUST_NOT_LEAK";

fn connection_input() -> DownloaderConnectionInput {
    DownloaderConnectionInput {
        kind: DownloaderKind::Qbittorrent,
        display_name: "Primary qBit".to_owned(),
        base_url: "https://download.test/qbit".to_owned(),
        username: SecretString::new("user".to_owned()),
        password: SecretString::new("password".to_owned()),
        enabled: true,
    }
}

fn task_command(
    connection_id: Uuid,
    source: &str,
    display_name: &str,
) -> CreateDownloadTaskCommand {
    CreateDownloadTaskCommand {
        connection_id,
        source: SecretString::new(source.to_owned()),
        display_name: display_name.to_owned(),
    }
}

async fn harness() -> (
    TestConfigDir,
    mediaflow_core::platform::db::Db,
    DownloaderConnectionStore,
    DownloadTaskStore,
    Uuid,
) {
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
    (fixture, db, connections, tasks, connection_id)
}

#[tokio::test]
async fn source_is_validated_encrypted_recoverable_and_redacted_from_debug() {
    let (fixture, db, _connections, tasks, connection_id) = harness().await;
    for invalid in [
        "/tmp/local.torrent",
        "file:///tmp/local.torrent",
        "http://tracker.test/file.torrent",
        "magnet:?dn=missing-info-hash",
    ] {
        let error = tasks
            .accept(
                task_command(connection_id, invalid, "invalid"),
                "invalid-source-key",
                200,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ValidationFailed);
    }

    let command = task_command(connection_id, SOURCE, "Ubuntu ISO");
    let debug = format!("{command:?}");
    assert!(!debug.contains("SOURCE_MUST_NOT_LEAK"));
    assert!(debug.contains("[REDACTED]"));
    let accepted = tasks
        .accept(command, "create-download-1", 300)
        .await
        .unwrap();
    assert_eq!(accepted.status, DownloadTaskStatus::Queued);
    assert_eq!(accepted.progress_basis_points, 0);
    assert!(!format!("{accepted:?}").contains("SOURCE_MUST_NOT_LEAK"));

    let (nonce, ciphertext, key_digest) = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, Vec<u8>)>(
        "SELECT source_nonce,source_ciphertext,idempotency_key_sha256 FROM download_tasks WHERE id=?",
    )
    .bind(accepted.id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(nonce.len(), 24);
    assert_eq!(key_digest.len(), 32);
    assert!(
        !ciphertext
            .windows(b"SOURCE_MUST_NOT_LEAK".len())
            .any(|window| window == b"SOURCE_MUST_NOT_LEAK")
    );
    let database = std::fs::read(fixture.database_path()).unwrap();
    assert!(
        !database
            .windows(b"create-download-1".len())
            .any(|window| window == b"create-download-1")
    );
    let source = tasks.load_source(accepted.id).await.unwrap();
    assert_eq!(source.expose(), SOURCE.as_bytes());
}

#[tokio::test]
async fn idempotency_remote_uniqueness_progress_and_active_delete_are_enforced() {
    let (_fixture, _db, connections, tasks, connection_id) = harness().await;
    let first = tasks
        .accept(
            task_command(connection_id, SOURCE, "Ubuntu ISO"),
            "same-key",
            200,
        )
        .await
        .unwrap();
    let replay = tasks
        .accept(
            task_command(connection_id, SOURCE, "Ubuntu ISO"),
            "same-key",
            300,
        )
        .await
        .unwrap();
    assert_eq!(replay.id, first.id);
    let conflict = tasks
        .accept(
            task_command(connection_id, SOURCE, "Different display"),
            "same-key",
            400,
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::RequestConflict);

    let second = tasks
        .accept(
            task_command(
                connection_id,
                "https://tracker.test/private/SOURCE_2_MUST_NOT_LEAK.torrent?passkey=secret",
                "Second",
            ),
            "second-key",
            500,
        )
        .await
        .unwrap();
    let first_lease = tasks.claim("worker-a", 600).await.unwrap().unwrap();
    assert_eq!(first_lease.task_id, first.id);
    let remote = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    tasks
        .commit_remote_ref(first.id, "worker-a", remote, 700)
        .await
        .unwrap();
    let snapshot = tasks
        .commit_snapshot(
            first.id,
            "worker-a",
            RemoteDownloadStatus::Downloading,
            5000,
            800,
        )
        .await
        .unwrap();
    assert_eq!(snapshot.progress_basis_points, 5000);

    let second_lease = tasks.claim("worker-b", 900).await.unwrap().unwrap();
    assert_eq!(second_lease.task_id, second.id);
    let duplicate = tasks
        .commit_remote_ref(second.id, "worker-b", remote, 1000)
        .await
        .unwrap_err();
    assert_eq!(duplicate.code(), ErrorCode::RequestConflict);
    let invalid_progress = tasks
        .commit_snapshot(
            first.id,
            "worker-a",
            RemoteDownloadStatus::Downloading,
            10_001,
            1100,
        )
        .await
        .unwrap_err();
    assert_eq!(invalid_progress.code(), ErrorCode::ValidationFailed);

    let active_delete = connections.delete(connection_id, 1).await.unwrap_err();
    assert_eq!(active_delete.code(), ErrorCode::ResourceConflict);
    tasks
        .fail(
            first.id,
            None,
            mediaflow_core::connectors::downloader::model::DownloaderFailureCode::RemoteMissing,
            1200,
        )
        .await
        .unwrap();
    tasks
        .fail(
            second.id,
            Some("worker-b"),
            mediaflow_core::connectors::downloader::model::DownloaderFailureCode::InvalidResponse,
            1300,
        )
        .await
        .unwrap();
    connections.delete(connection_id, 1).await.unwrap();
    assert!(tasks.get(first.id).await.unwrap().is_some());
}

#[tokio::test]
async fn task_pages_use_stable_cursors_and_never_include_private_fields() {
    let (_fixture, _db, _connections, tasks, connection_id) = harness().await;
    for index in 0..3 {
        tasks
            .accept(
                task_command(connection_id, SOURCE, &format!("Task {index}")),
                &format!("page-key-{index}"),
                200 + index,
            )
            .await
            .unwrap();
    }
    let filter = DownloadTaskFilter::default();
    let first = tasks
        .list(&filter, &PageRequest::new(None, Some(2)).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let cursor = first.next_cursor.unwrap();
    let second = tasks
        .list(&filter, &PageRequest::new(Some(cursor), Some(2)).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next_cursor.is_none());
    let json = serde_json::to_string(&first.items).unwrap();
    for private in [
        "source",
        "sha256",
        "ciphertext",
        "nonce",
        "remote_path",
        "SOURCE_MUST_NOT_LEAK",
    ] {
        assert!(!json.contains(private));
    }
}

#[tokio::test]
async fn retry_wait_is_not_claimable_before_its_due_time() {
    let (_fixture, _db, _connections, tasks, connection_id) = harness().await;
    let task = tasks
        .accept(
            task_command(connection_id, SOURCE, "Retry later"),
            "retry-key",
            200,
        )
        .await
        .unwrap();
    let lease = tasks.claim("worker-retry", 300).await.unwrap().unwrap();
    assert_eq!(lease.task_id, task.id);
    let waiting = tasks
        .schedule_retry(
            task.id,
            "worker-retry",
            mediaflow_core::connectors::downloader::model::DownloaderFailureCode::ProviderTimeout,
            1_000,
            400,
        )
        .await
        .unwrap();
    assert_eq!(waiting.status, DownloadTaskStatus::RetryWait);
    assert!(tasks.claim("too-early", 999).await.unwrap().is_none());
    assert_eq!(
        tasks
            .claim("worker-recovered", 1_000)
            .await
            .unwrap()
            .unwrap()
            .task_id,
        task.id
    );
}
