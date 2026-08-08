mod common;

use mediaflow_core::automation::model::{
    AutomationSourceInput, AutomationSourceKind, WebhookAction,
};
use mediaflow_core::automation::source_store::AutomationSourceStore;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::{DownloaderConnectionInput, DownloaderKind};
use mediaflow_core::connectors::model::{IntegrationHealth, SecretString};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use uuid::Uuid;

const FEED_URL: &str = "https://feeds.example.test/private/rss.xml?token=feed-secret";

async fn seed_downloader(
    db: &mediaflow_core::platform::db::Db,
    fixture: &common::TestConfigDir,
) -> Uuid {
    let id = Uuid::now_v7();
    DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            id,
            DownloaderConnectionInput {
                kind: DownloaderKind::Qbittorrent,
                display_name: "Automation qBit".to_owned(),
                base_url: "https://download.example.test".to_owned(),
                username: SecretString::new("user".to_owned()),
                password: SecretString::new("password".to_owned()),
                enabled: true,
            },
            1,
        )
        .await
        .unwrap();
    id
}

fn rss(downloader_connection_id: Uuid, feed_url: &str) -> AutomationSourceInput {
    AutomationSourceInput::Rss {
        display_name: "Private feed".to_owned(),
        enabled: true,
        feed_url: SecretString::new(feed_url.to_owned()),
        downloader_connection_id,
        poll_interval_seconds: 300,
    }
}

#[tokio::test]
async fn strict_source_shapes_validate_references_before_writing() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store =
        AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();

    for url in [
        "http://feeds.example.test/rss.xml",
        "https://user:secret@feeds.example.test/rss.xml",
        "https://feeds.example.test/rss.xml#fragment",
    ] {
        let error = store
            .insert(Uuid::now_v7(), rss(Uuid::now_v7(), url), None, 10)
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ValidationFailed);
    }

    let missing_downloader = store
        .insert(
            Uuid::now_v7(),
            rss(Uuid::now_v7(), "https://feeds.example.test/rss.xml"),
            None,
            10,
        )
        .await
        .unwrap_err();
    assert_eq!(missing_downloader.code(), ErrorCode::NotFound);

    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM automation_sources")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn one_connection_has_at_most_one_enabled_completion_mapping() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let downloader_id = seed_downloader(&db, &fixture).await;
    let first_inbox = common::seed_inbox(db.pool()).await;
    let second_inbox = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'completion-second',x'2e','.',x'03',x'04','available',0,1,0,0)",
    )
    .bind(second_inbox.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let store =
        AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let mapping = |enabled, inbox_directory_id| AutomationSourceInput::DownloadCompletion {
        display_name: "Completion mapping".to_owned(),
        enabled,
        downloader_connection_id: downloader_id,
        inbox_directory_id,
    };

    store
        .insert(first, mapping(true, first_inbox), None, 100)
        .await
        .unwrap();
    let conflict = store
        .insert(second, mapping(true, second_inbox), None, 101)
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::ResourceConflict);
    store
        .insert(second, mapping(false, second_inbox), None, 102)
        .await
        .unwrap();
    assert_eq!(
        store
            .replace(second, 1, mapping(true, second_inbox), 103)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ResourceConflict
    );
    store
        .replace(first, 1, mapping(false, first_inbox), 104)
        .await
        .unwrap();
    assert!(
        store
            .replace(second, 1, mapping(true, second_inbox), 105)
            .await
            .unwrap()
            .enabled
    );
}

#[tokio::test]
async fn rss_feed_and_webhook_secret_are_encrypted_and_redacted() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let downloader_id = seed_downloader(&db, &fixture).await;
    let store =
        AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();

    let rss_id = Uuid::now_v7();
    let rss_input = rss(downloader_id, FEED_URL);
    assert!(!format!("{rss_input:?}").contains("feed-secret"));
    let saved = store.insert(rss_id, rss_input, None, 100).await.unwrap();
    assert_eq!(saved.kind, AutomationSourceKind::Rss);
    assert_eq!(
        saved.endpoint_summary.as_deref(),
        Some("https://feeds.example.test")
    );
    assert_eq!(saved.health, IntegrationHealth::Degraded);
    assert!(!format!("{saved:?}").contains("feed-secret"));
    assert_eq!(
        store.load_secret(rss_id).await.unwrap().expose(),
        FEED_URL.as_bytes()
    );

    let webhook_id = Uuid::now_v7();
    let webhook_secret = SecretString::new("webhook-secret-that-must-never-leak".to_owned());
    let webhook = store
        .insert(
            webhook_id,
            AutomationSourceInput::Webhook {
                display_name: "Inbound webhook".to_owned(),
                enabled: true,
                allowed_actions: vec![WebhookAction::DownloadCreate, WebhookAction::InboxReconcile],
            },
            Some(webhook_secret),
            200,
        )
        .await
        .unwrap();
    assert_eq!(webhook.kind, AutomationSourceKind::Webhook);
    assert_eq!(webhook.secret_fingerprint.as_deref().map(str::len), Some(8));
    assert_eq!(
        store.load_secret(webhook_id).await.unwrap().expose(),
        b"webhook-secret-that-must-never-leak"
    );

    let rows = sqlx::query_as::<_, (String, Vec<u8>)>(
        "SELECT kind,secret_ciphertext FROM automation_sources ORDER BY created_at_us",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for (_, ciphertext) in rows {
        assert!(
            !ciphertext
                .windows(b"feed-secret".len())
                .any(|part| part == b"feed-secret")
        );
        assert!(
            !ciphertext
                .windows(b"webhook-secret".len())
                .any(|part| part == b"webhook-secret")
        );
    }
}

#[tokio::test]
async fn all_source_kinds_page_replace_and_delete_with_versions() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let downloader_id = seed_downloader(&db, &fixture).await;
    let inbox_id = common::seed_inbox(db.pool()).await;
    let store =
        AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();

    let rss_id = Uuid::now_v7();
    store
        .insert(rss_id, rss(downloader_id, FEED_URL), None, 100)
        .await
        .unwrap();
    let webhook_id = Uuid::now_v7();
    store
        .insert(
            webhook_id,
            AutomationSourceInput::Webhook {
                display_name: "Webhook".to_owned(),
                enabled: true,
                allowed_actions: vec![WebhookAction::DownloadCreate],
            },
            Some(SecretString::new(
                "a-webhook-secret-that-is-long-enough".to_owned(),
            )),
            200,
        )
        .await
        .unwrap();
    let completion_id = Uuid::now_v7();
    let completion = store
        .insert(
            completion_id,
            AutomationSourceInput::DownloadCompletion {
                display_name: "Completion mapping".to_owned(),
                enabled: false,
                downloader_connection_id: downloader_id,
                inbox_directory_id: inbox_id,
            },
            None,
            300,
        )
        .await
        .unwrap();
    assert_eq!(completion.kind, AutomationSourceKind::DownloadCompletion);
    assert!(completion.secret_fingerprint.is_none());
    assert_eq!(
        store.load_secret(completion_id).await.unwrap_err().code(),
        ErrorCode::IntegrationNotConfigured
    );

    let first = store
        .list_page(&PageRequest::new(None, Some(2)).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    assert!(first.next_cursor.is_some());
    let second = store
        .list_page(&PageRequest::new(first.next_cursor, Some(2)).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);

    let stale = store
        .replace(
            rss_id,
            0,
            rss(downloader_id, "https://feeds.example.test/new.xml"),
            400,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), ErrorCode::ConfigVersionConflict);
    let replaced = store
        .replace(
            rss_id,
            1,
            rss(downloader_id, "https://feeds.example.test/new.xml"),
            500,
        )
        .await
        .unwrap();
    assert_eq!(replaced.config_version, 2);
    assert_eq!(replaced.projection_version, 2);

    let enabled_delete = store.delete(webhook_id, 1).await.unwrap_err();
    assert_eq!(enabled_delete.code(), ErrorCode::ResourceConflict);
    store.delete(completion_id, 1).await.unwrap();
    assert!(store.get(completion_id).await.unwrap().is_none());
}
