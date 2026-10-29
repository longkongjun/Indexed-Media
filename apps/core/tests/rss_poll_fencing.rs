//! Poll success and failure must fence the real `SQLite` source lease.
mod common;
use async_trait::async_trait;
use mediaflow_core::automation::event_store::AutomationEventStore;
use mediaflow_core::automation::model::AutomationSourceInput;
use mediaflow_core::automation::rss::client::{
    FeedClient, FeedClientError, FeedResponse, RssCursor,
};
use mediaflow_core::automation::rss::poller::{FeedPollSource, FeedPoller};
use mediaflow_core::automation::source_store::AutomationSourceStore;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::SecretString;
use mediaflow_core::platform::db::Db;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::secrets::SecretBytes;
use std::sync::Arc;
use uuid::Uuid;

struct ResponseClient {
    failure: bool,
    not_modified: bool,
}
#[async_trait]
impl FeedClient for ResponseClient {
    async fn fetch(
        &self,
        _: &SecretBytes,
        _: Option<&RssCursor>,
    ) -> Result<FeedResponse, FeedClientError> {
        if self.failure {
            return Err(FeedClientError::Unavailable);
        }
        let cursor = Some(RssCursor::new(Some("\"fresh\"".to_owned()), None).unwrap());
        if self.not_modified {
            return Ok(FeedResponse::NotModified { cursor });
        }
        Ok(FeedResponse::Modified {
            body: include_bytes!("fixtures/rss/rss20.xml").to_vec(),
            cursor,
            previous_cursor_present: false,
        })
    }
}

async fn fixture() -> (
    common::TestConfigDir,
    Db,
    Arc<AutomationEventStore>,
    FeedPollSource,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let connection: Vec<u8> = sqlx::query_scalar("SELECT id FROM downloader_connections LIMIT 1")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let source = Uuid::now_v7();
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            source,
            AutomationSourceInput::Rss {
                display_name: "Lease fenced source".to_owned(),
                enabled: true,
                feed_url: SecretString::new("https://feed.example.test/rss".to_owned()),
                downloader_connection_id: Uuid::from_slice(&connection).unwrap(),
                poll_interval_seconds: 60,
            },
            None,
            1,
        )
        .await
        .unwrap();
    let store = Arc::new(
        AutomationEventStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap(),
    );
    let lease = store
        .claim_due_rss_source(chrono::Utc::now().timestamp_micros(), 30_000_000)
        .await
        .unwrap()
        .unwrap();
    (fixture, db, store, lease)
}

type Snapshot = (
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<i64>,
    Option<i64>,
    String,
    Option<i64>,
    i64,
);
async fn snapshot(db: &Db, id: Uuid) -> Snapshot {
    sqlx::query_as(
        "SELECT r.cursor_nonce,r.cursor_ciphertext,r.lease_token,r.lease_expires_at_us,
        r.next_poll_at_us,s.health,s.checked_at_us,r.attempt_count FROM automation_source_runtime r
        JOIN automation_sources s ON s.id=r.source_id WHERE s.id=?",
    )
    .bind(id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn old_success_and_failure_cannot_clear_a_new_lease_or_change_cursor_and_health() {
    for failure in [false, true] {
        let (_fixture, db, store, old) = fixture().await;
        let id = old.id;
        sqlx::query("UPDATE automation_source_runtime SET lease_expires_at_us=0 WHERE source_id=?")
            .bind(id.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .unwrap();
        let _new = store
            .claim_due_rss_source(chrono::Utc::now().timestamp_micros(), 30_000_000)
            .await
            .unwrap()
            .unwrap();
        let before = snapshot(&db, id).await;
        let poller = FeedPoller::new(
            Arc::new(ResponseClient {
                failure,
                not_modified: false,
            }),
            store,
        );
        assert!(
            poller.poll(old).await.is_err(),
            "old result must not commit"
        );
        assert_eq!(
            snapshot(&db, id).await,
            before,
            "old success/failure must not change any source checkpoint"
        );
        let events: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM automation_events WHERE source_id=?")
                .bind(id.as_bytes().as_slice())
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(events, 0);
    }
}

#[tokio::test]
async fn expired_lease_is_rejected_even_before_another_worker_claims_it() {
    for failure in [false, true] {
        let (_fixture, db, store, old) = fixture().await;
        let id = old.id;
        sqlx::query("UPDATE automation_source_runtime SET lease_expires_at_us=0 WHERE source_id=?")
            .bind(id.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .unwrap();
        let before = snapshot(&db, id).await;
        let poller = FeedPoller::new(
            Arc::new(ResponseClient {
                failure,
                not_modified: false,
            }),
            store,
        );
        assert!(poller.poll(old).await.is_err());
        assert_eq!(snapshot(&db, id).await, before);
    }
}

#[tokio::test]
async fn disabled_configuration_rejects_late_success_and_failure_without_clearing_lease() {
    for failure in [false, true] {
        let (_fixture, db, store, old) = fixture().await;
        let id = old.id;
        sqlx::query(
            "UPDATE automation_sources SET enabled=0,config_version=config_version+1 WHERE id=?",
        )
        .bind(id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
        let before = snapshot(&db, id).await;
        assert!(
            FeedPoller::new(
                Arc::new(ResponseClient {
                    failure,
                    not_modified: false
                }),
                store
            )
            .poll(old)
            .await
            .is_err()
        );
        assert_eq!(snapshot(&db, id).await, before);
    }
}

#[tokio::test]
async fn unconditional_not_modified_response_never_sets_a_cursor_or_marks_source_healthy() {
    let (_fixture, db, store, source) = fixture().await;
    let id = source.id;
    assert!(source.cursor.is_none());
    assert_eq!(
        FeedPoller::new(
            Arc::new(ResponseClient {
                failure: false,
                not_modified: true
            }),
            store
        )
        .poll(source)
        .await,
        Err(FeedClientError::InvalidResponse)
    );
    let after = snapshot(&db, id).await;
    assert!(after.0.is_none() && after.1.is_none());
    assert_ne!(after.5, "healthy");
}

struct SignallingClient {
    fetched: Arc<tokio::sync::Notify>,
    failure: bool,
}
#[async_trait]
impl FeedClient for SignallingClient {
    async fn fetch(
        &self,
        url: &SecretBytes,
        cursor: Option<&RssCursor>,
    ) -> Result<FeedResponse, FeedClientError> {
        self.fetched.notify_one();
        ResponseClient {
            failure: self.failure,
            not_modified: false,
        }
        .fetch(url, cursor)
        .await
    }
}

#[tokio::test]
async fn expiry_is_checked_after_waiting_for_the_sqlite_write_lock() {
    for failure in [false, true] {
        let (_fixture, db, store, source) = fixture().await;
        let id = source.id;
        sqlx::query("UPDATE automation_source_runtime SET lease_expires_at_us=? WHERE source_id=?")
            .bind(chrono::Utc::now().timestamp_micros() + 50_000)
            .bind(id.as_bytes().as_slice())
            .execute(db.pool())
            .await
            .unwrap();
        let before = snapshot(&db, id).await;
        let writer = db.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
        let fetched = Arc::new(tokio::sync::Notify::new());
        let poller = FeedPoller::new(
            Arc::new(SignallingClient {
                fetched: fetched.clone(),
                failure,
            }),
            store,
        );
        let pending = tokio::spawn(async move { poller.poll(source).await });
        fetched.notified().await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        writer.rollback().await.unwrap();
        assert!(pending.await.unwrap().is_err());
        assert_eq!(
            snapshot(&db, id).await,
            before,
            "time sampled before the lock cannot authorize an expired result"
        );
    }
}

#[tokio::test]
async fn runtime_propagates_failed_success_and_failure_checkpoints() {
    use mediaflow_core::automation::runtime::AutomationRuntime;
    use mediaflow_core::platform::outbox::OutboxNotifier;
    for failure in [false, true] {
        let (fixture, db, _store, source) = fixture().await;
        sqlx::query("UPDATE automation_source_runtime SET lease_token=NULL,lease_expires_at_us=NULL WHERE source_id=?")
            .bind(source.id.as_bytes().as_slice()).execute(db.pool()).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_poll_checkpoint BEFORE UPDATE ON automation_source_runtime
            WHEN NEW.next_poll_at_us IS NOT OLD.next_poll_at_us BEGIN SELECT RAISE(ABORT,'checkpoint failure fixture'); END")
            .execute(db.pool()).await.unwrap();
        let before = snapshot(&db, source.id).await;
        let runtime = AutomationRuntime::with_feed_client(
            db.pool().clone(),
            &fixture.config().config_dir,
            OutboxNotifier::new(),
            Arc::new(ResponseClient {
                failure,
                not_modified: false,
            }),
        )
        .unwrap();
        assert!(
            runtime.run_once().await.is_err(),
            "checkpoint failure cannot be reported as successfully handled work"
        );
        let state = snapshot(&db, source.id).await;
        assert_eq!((state.0, state.1, state.4), (before.0, before.1, before.4));
        assert_ne!(state.5, "healthy");
    }
}

#[tokio::test]
async fn runtime_failure_subscription_reports_a_checkpoint_failure() {
    use mediaflow_core::automation::runtime::AutomationRuntime;
    use mediaflow_core::platform::outbox::OutboxNotifier;
    use mediaflow_core::shared::error::ErrorCode;
    let (fixture, db, _store, source) = fixture().await;
    sqlx::query("UPDATE automation_source_runtime SET lease_token=NULL,lease_expires_at_us=NULL WHERE source_id=?")
        .bind(source.id.as_bytes().as_slice()).execute(db.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_poll_checkpoint BEFORE UPDATE ON automation_source_runtime
        WHEN NEW.next_poll_at_us IS NOT OLD.next_poll_at_us BEGIN SELECT RAISE(ABORT,'checkpoint failure fixture'); END")
        .execute(db.pool()).await.unwrap();
    let runtime = AutomationRuntime::with_feed_client(
        db.pool().clone(),
        &fixture.config().config_dir,
        OutboxNotifier::new(),
        Arc::new(ResponseClient {
            failure: false,
            not_modified: false,
        }),
    )
    .unwrap();
    let mut failures = runtime.subscribe_failures();
    let handle = runtime.start();
    let observed =
        tokio::time::timeout(std::time::Duration::from_secs(5), failures.changed()).await;
    handle.abort();
    let _ = handle.await;
    observed.unwrap().unwrap();
    assert_eq!(*failures.borrow(), Some(ErrorCode::Internal));
}

#[tokio::test]
async fn a_disabled_source_is_not_misclassified_as_a_checkpoint_storage_failure() {
    let (_fixture, db, store, source) = fixture().await;
    sqlx::query("UPDATE automation_sources SET enabled=0 WHERE id=?")
        .bind(source.id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let outcome = FeedPoller::new(
        Arc::new(ResponseClient {
            failure: false,
            not_modified: false,
        }),
        store,
    )
    .poll(source)
    .await;
    assert_eq!(outcome, Err(FeedClientError::InvalidResponse));
}
