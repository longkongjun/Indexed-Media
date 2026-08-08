mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::{
    DownloaderCapabilities, DownloaderConnectionInput, DownloaderConnectionProbe,
    DownloaderFailureCode, DownloaderKind,
};
use mediaflow_core::connectors::model::{IntegrationHealth, SecretString};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use uuid::Uuid;

use common::TestConfigDir;

fn connection_input(
    kind: DownloaderKind,
    name: &str,
    base_url: &str,
    username: &str,
    password: &str,
) -> DownloaderConnectionInput {
    DownloaderConnectionInput {
        kind,
        display_name: name.to_owned(),
        base_url: base_url.to_owned(),
        username: SecretString::new(username.to_owned()),
        password: SecretString::new(password.to_owned()),
        enabled: true,
    }
}

#[tokio::test]
async fn unsafe_or_ambiguous_base_urls_are_rejected_before_a_row_is_written() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store =
        DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();

    for base_url in [
        "ftp://download.test",
        "https://user:secret@download.test",
        "https://download.test/path?token=secret",
        "https://download.test/path#fragment",
    ] {
        let error = store
            .insert(
                Uuid::now_v7(),
                connection_input(
                    DownloaderKind::Qbittorrent,
                    "unsafe",
                    base_url,
                    "admin",
                    "secret",
                ),
                100,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ValidationFailed);
    }

    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM downloader_connections")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn credentials_are_encrypted_at_rest_recoverable_and_redacted_from_debug() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store =
        DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let connection_id = Uuid::now_v7();
    let input = connection_input(
        DownloaderKind::Qbittorrent,
        "Primary qBit",
        "https://download.test/qbit/",
        "m4-store-user",
        "m4-store-password",
    );
    let input_debug = format!("{input:?}");
    assert!(!input_debug.contains("m4-store-user"));
    assert!(!input_debug.contains("m4-store-password"));

    let saved = store.insert(connection_id, input, 100).await.unwrap();
    assert_eq!(saved.id, connection_id);
    assert_eq!(saved.config_version, 1);
    assert_eq!(saved.base_url, "https://download.test/qbit");
    assert_eq!(saved.health, IntegrationHealth::Degraded);
    assert_eq!(
        saved.failure_code,
        Some(DownloaderFailureCode::IntegrationUnavailable)
    );
    let saved_debug = format!("{saved:?}");
    assert!(!saved_debug.contains("m4-store-user"));
    assert!(!saved_debug.contains("m4-store-password"));

    let (nonce, ciphertext) = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
        "SELECT secret_nonce,secret_ciphertext FROM downloader_connections WHERE id=?",
    )
    .bind(connection_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(nonce.len(), 24);
    assert!(
        !ciphertext
            .windows(b"m4-store-user".len())
            .any(|window| window == b"m4-store-user")
    );
    assert!(
        !ciphertext
            .windows(b"m4-store-password".len())
            .any(|window| window == b"m4-store-password")
    );

    let credentials = store.load_secret(connection_id).await.unwrap();
    assert_eq!(credentials.username().expose(), b"m4-store-user");
    assert_eq!(credentials.password().expose(), b"m4-store-password");
    let credentials_debug = format!("{credentials:?}");
    assert!(!credentials_debug.contains("m4-store-user"));
    assert!(!credentials_debug.contains("m4-store-password"));
}

#[tokio::test]
async fn replacement_and_deletion_require_the_current_configuration_version() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store =
        DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let connection_id = Uuid::now_v7();
    store
        .insert(
            connection_id,
            connection_input(
                DownloaderKind::Transmission,
                "Transmission",
                "http://download.test:9091/transmission",
                "first-user",
                "first-password",
            ),
            100,
        )
        .await
        .unwrap();

    let stale_replace = store
        .replace(
            connection_id,
            0,
            connection_input(
                DownloaderKind::Transmission,
                "Stale",
                "http://download.test:9091/transmission",
                "stale-user",
                "stale-password",
            ),
            200,
        )
        .await
        .unwrap_err();
    assert_eq!(stale_replace.code(), ErrorCode::ConfigVersionConflict);

    let replaced = store
        .replace(
            connection_id,
            1,
            connection_input(
                DownloaderKind::Transmission,
                "Transmission updated",
                "http://download.test:9091/transmission/",
                "second-user",
                "second-password",
            ),
            300,
        )
        .await
        .unwrap();
    assert_eq!(replaced.config_version, 2);
    assert_eq!(replaced.display_name, "Transmission updated");
    let credentials = store.load_secret(connection_id).await.unwrap();
    assert_eq!(credentials.username().expose(), b"second-user");
    assert_eq!(credentials.password().expose(), b"second-password");

    let stale_delete = store.delete(connection_id, 1).await.unwrap_err();
    assert_eq!(stale_delete.code(), ErrorCode::ConfigVersionConflict);
    store.delete(connection_id, 2).await.unwrap();
    assert!(store.get(connection_id).await.unwrap().is_none());
}

#[tokio::test]
async fn probe_commits_are_version_checked_and_isolated_per_connection() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store =
        DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let first_id = Uuid::now_v7();
    let second_id = Uuid::now_v7();
    for (id, kind, name) in [
        (first_id, DownloaderKind::Qbittorrent, "qBit"),
        (second_id, DownloaderKind::Transmission, "Transmission"),
    ] {
        store
            .insert(
                id,
                connection_input(kind, name, "https://download.test", "user", "password"),
                100,
            )
            .await
            .unwrap();
    }

    store
        .commit_probe(
            first_id,
            1,
            DownloaderConnectionProbe {
                health: IntegrationHealth::Healthy,
                failure_code: None,
                capabilities: Some(DownloaderCapabilities {
                    manual_add: true,
                    task_monitoring: true,
                    product_version: "5.1.2".to_owned(),
                    api_version: "2.11.4".to_owned(),
                }),
            },
            500,
        )
        .await
        .unwrap();

    let first = store.get(first_id).await.unwrap().unwrap();
    let second = store.get(second_id).await.unwrap().unwrap();
    assert_eq!(first.health, IntegrationHealth::Healthy);
    assert_eq!(first.checked_at_us, Some(500));
    assert_eq!(first.capabilities.unwrap().api_version, "2.11.4");
    assert_eq!(second.health, IntegrationHealth::Degraded);
    assert_eq!(second.checked_at_us, None);
    assert_eq!(second.capabilities, None);

    let stale = store
        .commit_probe(
            second_id,
            2,
            DownloaderConnectionProbe {
                health: IntegrationHealth::Unavailable,
                failure_code: Some(DownloaderFailureCode::ProviderTimeout),
                capabilities: None,
            },
            600,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), ErrorCode::ConfigVersionConflict);
}
