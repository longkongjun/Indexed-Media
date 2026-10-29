//! Durable notification delivery uses actual `SQLite`; transports are isolated.
mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::notifications::store::{NotificationKind, NotificationStore};
use mediaflow_core::notifications::webhook::WebhookStore;
use mediaflow_core::platform::migrations::migrate_with_backup;
use uuid::Uuid;

#[tokio::test]
async fn enabled_webhook_enqueues_only_new_notifications_in_the_same_transaction() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    NotificationStore::insert_in_transaction(
        &mut tx,
        NotificationKind::SystemNotice,
        "Before configuration",
        "No history backfill",
        None,
        1,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let webhook = Uuid::now_v7();
    WebhookStore::new(db.pool().clone())
        .save(
            webhook,
            0,
            "https://receiver.example.test/events",
            None,
            true,
            2,
        )
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    let notification = NotificationStore::insert_in_transaction(
        &mut tx,
        NotificationKind::SystemNotice,
        "New notification",
        "Fixed redacted body",
        None,
        3,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let rows: Vec<(Vec<u8>, Vec<u8>)> =
        sqlx::query_as("SELECT notification_id,webhook_id FROM notification_deliveries")
            .fetch_all(db.pool())
            .await
            .unwrap();
    assert_eq!(
        rows,
        [(
            notification.as_bytes().to_vec(),
            webhook.as_bytes().to_vec()
        )]
    );
    let mut tx = db.pool().begin().await.unwrap();
    NotificationStore::insert_in_transaction(
        &mut tx,
        NotificationKind::SystemNotice,
        "Rolled back",
        "No outbound work",
        None,
        4,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_deliveries")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 1);
}

use async_trait::async_trait;
use mediaflow_core::notifications::delivery::NotificationDeliveryWorker;
use mediaflow_core::notifications::delivery_store::{
    DeliveryOutcome, DeliveryPayload, DeliveryStore,
};
use mediaflow_core::notifications::transport::WebhookTransport;
use std::sync::{Arc, Mutex};

struct RecordingTransport {
    outcome: DeliveryOutcome,
    calls: Mutex<Vec<String>>,
}
#[async_trait]
impl WebhookTransport for RecordingTransport {
    async fn deliver(&self, _: &str, payload: &DeliveryPayload) -> DeliveryOutcome {
        self.calls.lock().unwrap().push(payload.delivery_id.clone());
        self.outcome
    }
}

async fn delivery_fixture() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    Uuid,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let webhook = Uuid::now_v7();
    WebhookStore::new(db.pool().clone())
        .save(
            webhook,
            0,
            "https://receiver.example.test/events",
            None,
            true,
            1,
        )
        .await
        .unwrap();
    let mut tx = db.pool().begin().await.unwrap();
    NotificationStore::insert_in_transaction(
        &mut tx,
        NotificationKind::SystemNotice,
        "New notification",
        "Redacted body",
        None,
        2,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (fixture, db, webhook)
}

#[tokio::test]
async fn concurrent_claim_and_restarted_worker_keep_one_identity_and_fence_old_receipt() {
    let (_fixture, db, _) = delivery_fixture().await;
    let store = DeliveryStore::new(db.pool().clone());
    let (left, right) = tokio::join!(store.claim(10), store.claim(10));
    let (left, right) = (left.unwrap(), right.unwrap());
    assert_eq!(
        usize::from(left.is_some()) + usize::from(right.is_some()),
        1
    );
    let first = left.or(right).unwrap();
    let restarted = DeliveryStore::new(db.pool().clone());
    let second = restarted.claim(30_000_011).await.unwrap().unwrap();
    assert_eq!(first.payload.delivery_id, second.payload.delivery_id);
    assert!(!store.configuration_current(&first).await.unwrap());
    assert!(restarted.configuration_current(&second).await.unwrap());
    assert!(
        !store
            .finish(&first, DeliveryOutcome::Delivered, 30_000_012)
            .await
            .unwrap()
    );
    assert!(
        restarted
            .finish(&second, DeliveryOutcome::Delivered, 30_000_013)
            .await
            .unwrap()
    );
    assert!(restarted.claim(60_000_014).await.unwrap().is_none());
}

#[tokio::test]
async fn retry_backoff_is_durable_bounded_and_reuses_the_receiver_key() {
    let (_fixture, db, _) = delivery_fixture().await;
    let transport = Arc::new(RecordingTransport {
        outcome: DeliveryOutcome::Retryable,
        calls: Mutex::new(Vec::new()),
    });
    let mut now = 10;
    for attempt in 1..=10 {
        // Recreate worker each time, proving that retry state is not in process memory.
        let worker = NotificationDeliveryWorker::new(db.pool().clone(), transport.clone());
        assert!(worker.run_once(now).await.unwrap());
        let (status, count, retry): (String, i64, Option<i64>) =
            sqlx::query_as("SELECT status,attempt_count,retry_at_us FROM notification_deliveries")
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(count, attempt);
        if attempt < 10 {
            assert_eq!(status, "retry-wait");
            let due = retry.unwrap();
            assert!(due > now);
            assert!(!worker.run_once(due - 1).await.unwrap());
            now = due;
        } else {
            assert_eq!(status, "failed");
            assert!(retry.is_none());
            assert!(!worker.run_once(now + 4_000_000_000).await.unwrap());
        }
    }
    let calls = transport.calls.lock().unwrap();
    assert_eq!(calls.len(), 10);
    assert!(calls.iter().all(|key| key == &calls[0]));
}

#[tokio::test]
async fn configuration_change_and_late_receipt_cannot_target_or_heal_the_new_configuration() {
    let (_fixture, db, webhook) = delivery_fixture().await;
    let store = DeliveryStore::new(db.pool().clone());
    let lease = store.claim(10).await.unwrap().unwrap();
    WebhookStore::new(db.pool().clone())
        .save(
            webhook,
            1,
            "https://new-receiver.example.test/events",
            None,
            true,
            20,
        )
        .await
        .unwrap();
    assert!(!store.configuration_current(&lease).await.unwrap());
    assert!(
        store
            .finish(&lease, DeliveryOutcome::Delivered, 30)
            .await
            .unwrap()
    );
    let state: (String, String) = sqlx::query_as(
        "SELECT d.status,w.health FROM notification_deliveries d JOIN notification_webhooks w ON w.id=d.webhook_id")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(state, ("cancelled".to_owned(), "unconfigured".to_owned()));
    assert!(store.claim(40).await.unwrap().is_none());
}

#[tokio::test]
async fn deleting_or_disabling_a_webhook_stops_queued_work_without_transport_calls() {
    for delete in [false, true] {
        let (_fixture, db, webhook) = delivery_fixture().await;
        let config = WebhookStore::new(db.pool().clone());
        if delete {
            config.delete(webhook, 1).await.unwrap();
        } else {
            config
                .save(
                    webhook,
                    1,
                    "https://receiver.example.test/events",
                    None,
                    false,
                    20,
                )
                .await
                .unwrap();
        }
        let transport = Arc::new(RecordingTransport {
            outcome: DeliveryOutcome::Delivered,
            calls: Mutex::new(Vec::new()),
        });
        assert!(
            !NotificationDeliveryWorker::new(db.pool().clone(), transport.clone())
                .run_once(30)
                .await
                .unwrap()
        );
        assert!(transport.calls.lock().unwrap().is_empty());
        let status: String = sqlx::query_scalar("SELECT status FROM notification_deliveries")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(status, "cancelled");
    }
}

#[tokio::test]
async fn terminal_receiver_rejections_do_not_retry_or_change_local_notification() {
    for outcome in [DeliveryOutcome::Unauthorized, DeliveryOutcome::Rejected] {
        let (_fixture, db, _) = delivery_fixture().await;
        let transport = Arc::new(RecordingTransport {
            outcome,
            calls: Mutex::new(Vec::new()),
        });
        let worker = NotificationDeliveryWorker::new(db.pool().clone(), transport.clone());
        assert!(worker.run_once(10).await.unwrap());
        assert!(!worker.run_once(4_000_000_000).await.unwrap());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn forward_upgrade_preserves_history_without_backfill_and_creates_a_backup() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let baseline = tempfile::tempdir().unwrap();
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations")).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name
            .split_once('_')
            .and_then(|(n, _)| n.parse::<u16>().ok())
            .is_some_and(|n| n <= 38)
        {
            std::fs::copy(entry.path(), baseline.path().join(name.as_ref())).unwrap();
        }
    }
    let pool = mediaflow_core::platform::db::open_pool(&fixture.database_path())
        .await
        .unwrap();
    sqlx::migrate::Migrator::new(baseline.path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    let old = Uuid::now_v7();
    sqlx::query("INSERT INTO notifications(id,kind,title,body,task_id,created_at_us) VALUES (?,'system.notice','Existing','Preserve history',NULL,1)")
        .bind(old.as_bytes().as_slice()).execute(&pool).await.unwrap();
    pool.close().await;
    let upgraded = migrate_with_backup(fixture.config()).await.unwrap();
    let history: Vec<Vec<u8>> = sqlx::query_scalar("SELECT id FROM notifications")
        .fetch_all(upgraded.pool())
        .await
        .unwrap();
    assert_eq!(history, [old.as_bytes().to_vec()]);
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_deliveries")
        .fetch_one(upgraded.pool())
        .await
        .unwrap();
    assert_eq!(pending, 0);
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(upgraded.pool())
        .await
        .unwrap();
    assert!(version >= 39);
    assert!(
        std::fs::read_dir(fixture.config().config_dir.join("backups"))
            .unwrap()
            .next()
            .is_some()
    );
}

struct UnresponsiveTransport;
#[async_trait]
impl WebhookTransport for UnresponsiveTransport {
    async fn deliver(&self, _: &str, _: &DeliveryPayload) -> DeliveryOutcome {
        std::future::pending().await
    }
}

#[tokio::test]
async fn unresponsive_transport_has_a_total_budget_and_durable_retry() {
    let (_fixture, db, _) = delivery_fixture().await;
    let worker =
        NotificationDeliveryWorker::new(db.pool().clone(), Arc::new(UnresponsiveTransport));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(15), worker.run_once(10))
            .await
            .expect("worker must impose its own ten-second transport budget")
            .unwrap()
    );
    let status: String = sqlx::query_scalar("SELECT status FROM notification_deliveries")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(status, "retry-wait");
}

#[tokio::test]
async fn deleting_and_recreating_the_same_webhook_identity_never_retargets_old_payloads() {
    let (_fixture, db, webhook) = delivery_fixture().await;
    let configs = WebhookStore::new(db.pool().clone());
    configs.delete(webhook, 1).await.unwrap();
    configs
        .save(
            webhook,
            0,
            "https://new-receiver.example.test/events",
            None,
            true,
            30,
        )
        .await
        .unwrap();
    let transport = Arc::new(RecordingTransport {
        outcome: DeliveryOutcome::Delivered,
        calls: Mutex::new(Vec::new()),
    });
    assert!(
        !NotificationDeliveryWorker::new(db.pool().clone(), transport.clone())
            .run_once(40)
            .await
            .unwrap(),
        "deletion must cancel old work even if the configuration UUID/version is reused"
    );
    assert!(transport.calls.lock().unwrap().is_empty());
    let mut tx = db.pool().begin().await.unwrap();
    NotificationStore::insert_in_transaction(
        &mut tx,
        NotificationKind::SystemNotice,
        "New config",
        "New payload",
        None,
        50,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let fresh = DeliveryStore::new(db.pool().clone())
        .claim(60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fresh.target_url, "https://new-receiver.example.test/events");
    assert_eq!(fresh.payload.title, "New config");
}

#[tokio::test]
async fn cancelled_inflight_lease_cannot_pass_preflight_after_configuration_identity_reuse() {
    let (_fixture, db, webhook) = delivery_fixture().await;
    let store = DeliveryStore::new(db.pool().clone());
    let lease = store.claim(10).await.unwrap().unwrap();
    let configs = WebhookStore::new(db.pool().clone());
    configs.delete(webhook, 1).await.unwrap();
    configs
        .save(
            webhook,
            0,
            "https://new-receiver.example.test/events",
            None,
            true,
            20,
        )
        .await
        .unwrap();
    assert!(
        !store.configuration_current(&lease).await.unwrap(),
        "configuration identity alone cannot revive a cancelled lease"
    );
    assert!(
        !store
            .finish(&lease, DeliveryOutcome::Delivered, 30)
            .await
            .unwrap()
    );
    let health: String = sqlx::query_scalar("SELECT health FROM notification_webhooks")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(health, "unconfigured");
}
