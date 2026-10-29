mod common;

use std::sync::Arc;

use async_trait::async_trait;
use mediaflow_core::automation::event_store::AutomationEventStore;
use mediaflow_core::automation::model::{
    AutomationAction, AutomationEventFilter, AutomationEventStatus, AutomationSourceInput,
};
use mediaflow_core::automation::rss::client::{
    FeedClient, FeedClientError, FeedResponse, RssCursor,
};
use mediaflow_core::automation::rss::poller::{FeedPollCommitPort, FeedPoller};
use mediaflow_core::automation::source_store::AutomationSourceStore;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::SecretString;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxReader;
use mediaflow_core::platform::secrets::SecretBytes;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::events::TaskEventEnvelope;

const RSS: &[u8] = include_bytes!("fixtures/rss/rss20.xml");

struct StaticFeed;

#[async_trait]
impl FeedClient for StaticFeed {
    async fn fetch(
        &self,
        _: &SecretBytes,
        cursor: Option<&RssCursor>,
    ) -> Result<FeedResponse, FeedClientError> {
        Ok(FeedResponse::Modified {
            body: RSS.to_vec(),
            cursor: Some(RssCursor::new(Some("\"event-store-v1\"".to_owned()), None)?),
            previous_cursor_present: cursor.is_some(),
        })
    }
}

#[tokio::test]
async fn claim_retry_cancel_and_lease_recovery_are_versioned_and_bounded() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let event_id = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let store =
        AutomationEventStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();

    let page = store
        .list_page(&PageRequest::new(None, Some(20)).unwrap())
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].action, AutomationAction::CreateDownload);
    assert_eq!(page.items[0].status, AutomationEventStatus::Pending);

    let lease = store.claim_one(100, 50).await.unwrap().unwrap();
    assert_eq!(lease.id, event_id);
    assert_eq!(lease.attempt_count, 1);
    assert!(store.claim_one(100, 50).await.unwrap().is_none());
    let running_cancel = store
        .cancel(event_id, "cancel-running-0001", 120)
        .await
        .unwrap_err();
    assert_eq!(running_cancel.code(), ErrorCode::TaskInvalidState);
    assert_eq!(store.reclaim_expired(149).await.unwrap(), 0);
    assert_eq!(store.reclaim_expired(150).await.unwrap(), 1);
    assert_eq!(
        store.get(event_id).await.unwrap().unwrap().status,
        AutomationEventStatus::RetryWait
    );

    let cancelled = store
        .cancel(event_id, "cancel-key-0001", 200)
        .await
        .unwrap();
    assert_eq!(cancelled.status, AutomationEventStatus::Cancelled);
    let replay = store
        .cancel(event_id, "cancel-key-0001", 201)
        .await
        .unwrap();
    assert_eq!(replay.projection_version, cancelled.projection_version);
    let conflict = store
        .retry(event_id, "cancel-key-0001", 202)
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::RequestConflict);

    let outbox = OutboxReader::new(db.pool().clone())
        .after(0, 20)
        .await
        .unwrap();
    assert_eq!(outbox.len(), 3);
    assert!(outbox.iter().all(|event| matches!(
        event,
        TaskEventEnvelope::AutomationEventChanged { payload, .. }
            if payload.automation_event_id == event_id
    )));
}

#[tokio::test]
async fn filtered_pages_bind_the_cursor_to_the_exact_filter() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let store =
        AutomationEventStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let page = store
        .list_filtered_page(
            &PageRequest::new(None, Some(1)).unwrap(),
            AutomationEventFilter {
                action: Some(AutomationAction::CreateDownload),
                ..AutomationEventFilter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    let cursor = page.next_cursor.unwrap();
    let next = store
        .list_filtered_page(
            &PageRequest::new(Some(cursor.clone()), Some(1)).unwrap(),
            AutomationEventFilter {
                action: Some(AutomationAction::CreateDownload),
                ..AutomationEventFilter::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(next.items.len(), 1);
    let changed = store
        .list_filtered_page(
            &PageRequest::new(Some(cursor), Some(1)).unwrap(),
            AutomationEventFilter {
                status: Some(AutomationEventStatus::Completed),
                ..AutomationEventFilter::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(changed.code(), ErrorCode::ValidationFailed);
}

#[tokio::test]
async fn rss_events_deduplicate_and_commit_the_encrypted_cursor_atomically() {
    let (fixture, db, source_id, store) = rss_storage_fixture().await;
    let poller = FeedPoller::new(Arc::new(StaticFeed), store.clone());
    let source = async || claim_for_poll(&store, &db, source_id).await;

    assert_eq!(
        poller
            .poll(source().await)
            .await
            .unwrap()
            .accepted_event_count,
        2
    );
    assert_eq!(
        poller
            .poll(source().await)
            .await
            .unwrap()
            .accepted_event_count,
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM automation_events WHERE source_id=?")
            .bind(source_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        2
    );
    let cursor_before = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
        "SELECT cursor_nonce,cursor_ciphertext FROM automation_source_runtime WHERE source_id=?",
    )
    .bind(source_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE automation_events SET request_sha256=randomblob(32)
         WHERE id=(SELECT id FROM automation_events WHERE source_id=? ORDER BY id LIMIT 1)",
    )
    .bind(source_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let conflicting = source().await;
    let conflicting_token = conflicting.lease_token;
    assert_eq!(
        poller.poll(conflicting).await,
        Err(FeedClientError::CheckpointFailed)
    );
    assert_eq!(
        sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "SELECT cursor_nonce,cursor_ciphertext FROM automation_source_runtime WHERE source_id=?",
        )
        .bind(source_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        cursor_before
    );
    FeedPollCommitPort::commit_failure(
        &*store,
        source_id,
        1,
        conflicting_token,
        FeedClientError::RateLimited,
    )
    .await
    .unwrap();
    let runtime = sqlx::query_as::<_, (Vec<u8>, Vec<u8>, i64, i64)>(
        "SELECT cursor_nonce,cursor_ciphertext,attempt_count,next_poll_at_us
         FROM automation_source_runtime WHERE source_id=?",
    )
    .bind(source_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!((runtime.0, runtime.1), cursor_before);
    assert_eq!(runtime.2, 1);
    assert!(runtime.3 > chrono::Utc::now().timestamp_micros());
    let raw = sqlx::query_scalar::<_, String>(
        "SELECT lower(hex(payload_ciphertext)) FROM automation_events WHERE source_id=? LIMIT 1",
    )
    .bind(source_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!raw.contains("6d61676e6574"));
    sqlx::query("UPDATE automation_source_runtime SET next_poll_at_us=0,lease_token=NULL,lease_expires_at_us=NULL WHERE source_id=?")
        .bind(source_id.as_bytes().as_slice()).execute(db.pool()).await.unwrap();
    let runtime = mediaflow_core::automation::runtime::AutomationRuntime::with_feed_client(
        db.pool().clone(),
        &fixture.config().config_dir,
        mediaflow_core::platform::outbox::OutboxNotifier::new(),
        Arc::new(StaticFeed),
    )
    .unwrap();
    assert!(
        runtime.run_once().await.is_err(),
        "same-identity payload conflict cannot be acknowledged as handled work"
    );
}

async fn rss_storage_fixture() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    uuid::Uuid,
    Arc<AutomationEventStore>,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let downloader_id: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM downloader_connections ORDER BY created_at_us LIMIT 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let downloader_id = uuid::Uuid::from_slice(&downloader_id).unwrap();
    let source_id = uuid::Uuid::now_v7();
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            source_id,
            AutomationSourceInput::Rss {
                display_name: "Persistent feed".to_owned(),
                enabled: true,
                feed_url: SecretString::new(
                    "https://feed.example.test/private?token=secret".to_owned(),
                ),
                downloader_connection_id: downloader_id,
                poll_interval_seconds: 300,
            },
            None,
            10,
        )
        .await
        .unwrap();
    let store = Arc::new(
        AutomationEventStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap(),
    );
    (fixture, db, source_id, store)
}

async fn claim_for_poll(
    store: &AutomationEventStore,
    db: &mediaflow_core::platform::db::Db,
    source_id: uuid::Uuid,
) -> mediaflow_core::automation::rss::poller::FeedPollSource {
    sqlx::query("UPDATE automation_source_runtime SET next_poll_at_us=0 WHERE source_id=?")
        .bind(source_id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    store
        .claim_due_rss_source(chrono::Utc::now().timestamp_micros(), 30_000_000)
        .await
        .unwrap()
        .unwrap()
}
