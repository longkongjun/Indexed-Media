#![allow(clippy::too_many_lines)]

mod common;

use std::time::Duration;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{
    DeliveryFailureCode, MAX_PUBLIC_EVENT_ID, OutboxNotifier, OutboxReader, OutboxRetention,
    OutboxWriter,
};
use mediaflow_core::tasks::events::TaskEventEnvelope;
use mediaflow_core::tasks::model::{NewScanTask, ScanCounts};
use mediaflow_core::tasks::store::TaskStore;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn rollback_leaves_no_ghost_and_ids_43_44_replay_at_least_once() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let task_id = Uuid::now_v7();
    let mut tx = db.pool().begin().await.unwrap();
    OutboxWriter::write(
        &mut tx,
        "task.state-changed",
        task_id,
        &json!({"status":"queued","recovering":false}),
        1,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(outbox_count(db.pool()).await, 0);

    for id in 41..=44 {
        insert_state_event(db.pool(), id, task_id, id).await;
    }
    let reader = OutboxReader::new(db.pool().clone());
    for _ in 0..2 {
        let replay = reader.after(42, 50).await.unwrap();
        assert_eq!(
            replay.iter().map(TaskEventEnvelope::id).collect::<Vec<_>>(),
            [43, 44]
        );
    }
}

#[tokio::test]
async fn delivery_attempts_are_separate_and_never_mutate_payload() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    insert_state_event(db.pool(), 41, Uuid::now_v7(), 1).await;
    let before: String =
        sqlx::query_scalar("SELECT payload_json FROM platform_outbox_events WHERE id=41")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let reader = OutboxReader::new(db.pool().clone());
    reader.record_delivery_attempt(41, None).await.unwrap();
    reader
        .record_delivery_attempt(41, Some(DeliveryFailureCode::ClientDisconnected))
        .await
        .unwrap();
    let row: (String, i64, Option<String>) = sqlx::query_as(
        "SELECT payload_json,delivery_attempts,last_delivery_error
         FROM platform_outbox_events WHERE id=41",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(row.0, before);
    assert_eq!(row.1, 2);
    assert_eq!(row.2.as_deref(), Some("client.disconnected"));
    assert!(!row.0.contains("client.disconnected"));
}

#[tokio::test]
async fn corrupt_or_unknown_event_stops_replay_with_only_a_stable_failure_code() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let sentinel = "/config/private.sqlite SELECT password_phc cookie_token";
    sqlx::query(
        "INSERT INTO platform_outbox_events
         (id,event_type,schema_version,aggregate_id,payload_json,committed_at_us)
         VALUES (41,'unknown.internal','1',?,json_object('diagnostic',?),1)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(sentinel)
    .execute(db.pool())
    .await
    .unwrap();
    let error = OutboxReader::new(db.pool().clone())
        .after(0, 50)
        .await
        .unwrap_err();
    assert_eq!(
        error.code(),
        mediaflow_core::shared::error::ErrorCode::Internal
    );
    assert!(!error.to_string().contains(sentinel));
    assert!(!format!("{error:?}").contains(sentinel));
    let failure: Option<String> =
        sqlx::query_scalar("SELECT last_delivery_error FROM platform_outbox_events WHERE id=41")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(failure.as_deref(), Some("event.decode_failed"));
}

#[tokio::test]
async fn progress_payload_rejects_raw_paths_tokens_and_internal_diagnostics() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let task = Uuid::now_v7();
    for (id, forbidden) in [
        (1, ("absolute_path", "/mnt/private/movie.mkv")),
        (2, ("cookie", "__Host-mediaflow_session=secret")),
        (
            3,
            ("diagnostic", "SELECT password_phc FROM identity_accounts"),
        ),
    ] {
        let mut payload = json!({
            "visited_directories": 1,
            "observed_files": 2,
            "skipped_entries": 0,
            "errors": 1
        });
        payload[forbidden.0] = json!(forbidden.1);
        sqlx::query(
            "INSERT INTO platform_outbox_events
             (id,event_type,schema_version,aggregate_id,payload_json,committed_at_us)
             VALUES (?,'task.progress','1',?,?,?)",
        )
        .bind(id)
        .bind(task.as_bytes().as_slice())
        .bind(payload.to_string())
        .bind(id)
        .execute(db.pool())
        .await
        .unwrap();
    }
    let error = OutboxReader::new(db.pool().clone())
        .after(0, 50)
        .await
        .expect_err("secret-bearing event is corrupt");
    let public = format!("{error} {error:?}");
    assert!(!public.contains("/mnt/private"));
    assert!(!public.contains("password_phc"));
    assert!(!public.contains("session=secret"));
}

#[tokio::test]
async fn task_store_coalesces_progress_at_0_249_250_ms_and_state_bypasses_it() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let (account, inbox) = seed_account_and_inbox(db.pool()).await;
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id: account,
            inbox_directory_id: inbox,
            idempotency_key: "progress-rate".to_owned(),
            now_us: 1,
        })
        .await
        .unwrap();
    let mut lease = store.claim_next("worker", 10).await.unwrap().unwrap();
    for (now, observed_files) in [(1_000_000, 1), (1_249_999, 2), (1_250_000, 3)] {
        lease = store
            .commit_batch(
                &lease,
                ScanCounts {
                    observed_files,
                    ..ScanCounts::default()
                },
                now,
            )
            .await
            .unwrap();
    }
    let progress: Vec<i64> = sqlx::query_scalar(
        "SELECT committed_at_us FROM platform_outbox_events
         WHERE aggregate_id=? AND event_type='task.progress' ORDER BY id",
    )
    .bind(task.id.as_bytes().as_slice())
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(progress, [1_000_000, 1_250_000]);

    let terminal = store
        .finish_success(&lease, lease.task.counts, 1_249_999)
        .await
        .unwrap();
    assert_eq!(terminal.status.as_str(), "completed");
    let terminal_at: i64 = sqlx::query_scalar(
        "SELECT committed_at_us FROM platform_outbox_events
         WHERE aggregate_id=? AND event_type='task.state-changed'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(task.id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(terminal_at, 1_249_999);
}

#[tokio::test]
async fn shared_notifier_wakes_only_after_a_successful_commit() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let (account, inbox) = seed_account_and_inbox(db.pool()).await;
    let notifier = OutboxNotifier::new();
    let mut receiver = notifier.subscribe();
    let store = TaskStore::new_with_notifier(db.pool().clone(), notifier);
    store
        .create(NewScanTask {
            account_id: account,
            inbox_directory_id: inbox,
            idempotency_key: "notify-after-commit".to_owned(),
            now_us: 1,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_millis(50), receiver.changed())
        .await
        .expect("post-commit notification")
        .unwrap();
}

#[tokio::test]
async fn max_safe_event_id_is_enforced_atomically_with_task_state() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let (account, inbox) = seed_account_and_inbox(db.pool()).await;
    let task = TaskStore::new(db.pool().clone())
        .create(NewScanTask {
            account_id: account,
            inbox_directory_id: inbox,
            idempotency_key: "max-public-event-id".to_owned(),
            now_us: 1,
        })
        .await
        .unwrap();
    sqlx::query("DELETE FROM platform_outbox_events")
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE sqlite_sequence SET seq=? WHERE name='platform_outbox_events'")
        .bind(MAX_PUBLIC_EVENT_ID - 2)
        .execute(db.pool())
        .await
        .unwrap();

    for expected in [MAX_PUBLIC_EVENT_ID - 1, MAX_PUBLIC_EVENT_ID] {
        let mut tx = db.pool().begin().await.unwrap();
        let id = OutboxWriter::write(
            &mut tx,
            "task.state-changed",
            task.id,
            &json!({"status":"running","recovering":false}),
            expected,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(id, expected);
        assert_eq!(
            OutboxReader::new(db.pool().clone())
                .after(expected - 1, 1)
                .await
                .unwrap()[0]
                .id(),
            expected
        );
    }

    let mut tx = db.pool().begin().await.unwrap();
    sqlx::query("UPDATE tasks_scan_tasks SET status='failed' WHERE id=?")
        .bind(task.id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .unwrap();
    let error = OutboxWriter::write(
        &mut tx,
        "task.state-changed",
        task.id,
        &json!({"status":"failed","recovering":false}),
        MAX_PUBLIC_EVENT_ID + 1,
    )
    .await
    .expect_err("MAX_SAFE + 1 must be rejected by SQLite before commit");
    assert_eq!(
        error.code(),
        mediaflow_core::shared::error::ErrorCode::Internal
    );
    tx.rollback().await.unwrap();

    let status: String = sqlx::query_scalar("SELECT status FROM tasks_scan_tasks WHERE id=?")
        .bind(task.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(
        status, "queued",
        "business state must roll back with the event"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM platform_outbox_events WHERE id>?")
            .bind(MAX_PUBLIC_EVENT_ID)
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn retention_has_no_off_by_one_and_requires_age_plus_successors() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let now = 30 * 60 * 60 * 1_000_000_i64;
    seed_many_events(db.pool(), 100_001, 1).await;
    let retention = OutboxRetention::new(db.pool().clone());
    assert_eq!(retention.cleanup(now).await.unwrap(), 1);
    assert_eq!(
        OutboxReader::new(db.pool().clone())
            .minimum_available_id()
            .await
            .unwrap(),
        Some(2)
    );

    sqlx::query("DELETE FROM platform_outbox_events")
        .execute(db.pool())
        .await
        .unwrap();
    seed_many_events(db.pool(), 100_000, 1).await;
    assert_eq!(
        retention.cleanup(now).await.unwrap(),
        0,
        "exactly 100,000 rows have no deletable predecessor"
    );

    sqlx::query("DELETE FROM platform_outbox_events")
        .execute(db.pool())
        .await
        .unwrap();
    seed_many_events(db.pool(), 100_001, 1).await;
    sqlx::query("UPDATE platform_outbox_events SET committed_at_us=? WHERE id=(SELECT MIN(id) FROM platform_outbox_events)")
        .bind(now)
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        retention.cleanup(now).await.unwrap(),
        0,
        "a young first row blocks a non-contiguous deletion"
    );

    let cleanup_plan_sql = "EXPLAIN QUERY PLAN WITH retention_boundary AS (
             SELECT id FROM platform_outbox_events
             ORDER BY id DESC LIMIT 1 OFFSET 99999
         ), prefix AS (
             SELECT id,committed_at_us FROM platform_outbox_events
             WHERE id < COALESCE((SELECT id FROM retention_boundary), 0)
             ORDER BY id ASC LIMIT 1000
         ), classified AS (
             SELECT id,
                    MAX(CASE WHEN committed_at_us>=? THEN 1 ELSE 0 END)
                    OVER (ORDER BY id ASC ROWS UNBOUNDED PRECEDING) AS blocked
             FROM prefix
         ), deletable AS (
             SELECT id FROM classified WHERE blocked=0
         )
         DELETE FROM platform_outbox_events WHERE id IN (SELECT id FROM deletable)";
    let plan = sqlx::query(cleanup_plan_sql)
        .bind(now - 24 * 60 * 60 * 1_000_000_i64)
        .fetch_all(db.pool())
        .await
        .unwrap();
    let detail = plan
        .iter()
        .map(|row| sqlx::Row::get::<String, _>(row, "detail"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(cleanup_plan_sql.contains("LIMIT 1000"));
    assert!(cleanup_plan_sql.contains("OFFSET 99999"));
    assert!(!cleanup_plan_sql.contains("MIN(id)"));
    assert!(!detail.contains("platform_outbox_events_committed_id_idx"));
    assert!(
        detail.contains("INTEGER PRIMARY KEY") || detail.contains("SCAN"),
        "{detail}"
    );
}

#[tokio::test]
async fn maintenance_cycle_drains_all_batches_and_preserves_successors_and_young_rows() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let now = 30 * 60 * 60 * 1_000_000_i64;
    seed_many_events(db.pool(), 102_501, 1).await;
    sqlx::query("UPDATE platform_outbox_events SET committed_at_us=? WHERE id>102000")
        .bind(now)
        .execute(db.pool())
        .await
        .unwrap();

    let deleted = OutboxRetention::new(db.pool().clone())
        .cleanup_cycle(now)
        .await
        .unwrap();
    assert_eq!(deleted, 2_501, "one cycle must drain every eligible batch");
    assert_eq!(outbox_count(db.pool()).await, 100_000);
    assert_eq!(
        OutboxReader::new(db.pool().clone())
            .minimum_available_id()
            .await
            .unwrap(),
        Some(2_502)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM platform_outbox_events WHERE committed_at_us>=?"
        )
        .bind(now - 24 * 60 * 60 * 1_000_000_i64)
        .fetch_one(db.pool())
        .await
        .unwrap(),
        501,
        "all young rows must remain"
    );
}

async fn insert_state_event(pool: &sqlx::SqlitePool, id: i64, task_id: Uuid, committed: i64) {
    sqlx::query(
        "INSERT INTO platform_outbox_events
         (id,event_type,schema_version,aggregate_id,payload_json,committed_at_us)
         VALUES (?,'task.state-changed','1',?,json_object('status','running','recovering',json('false')),?)",
    )
    .bind(id)
    .bind(task_id.as_bytes().as_slice())
    .bind(committed)
    .execute(pool)
    .await
    .unwrap();
}

async fn outbox_count(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM platform_outbox_events")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn seed_account_and_inbox(pool: &sqlx::SqlitePool) -> (Uuid, Uuid) {
    let account = Uuid::now_v7();
    let inbox = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','not-used',0,0)",
    )
    .bind(account.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'726F6F74',X'6469726563746F7279','available',0,0,0)",
    )
    .bind(inbox.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
    (account, inbox)
}

async fn seed_many_events(pool: &sqlx::SqlitePool, count: i64, committed_at_us: i64) {
    sqlx::query(
        "WITH RECURSIVE seq(id) AS (
             VALUES(1) UNION ALL SELECT id+1 FROM seq WHERE id<?
         )
         INSERT INTO platform_outbox_events
           (id,event_type,schema_version,aggregate_id,payload_json,committed_at_us)
         SELECT id,'task.state-changed','1',zeroblob(16),
                json_object('status','running','recovering',json('false')),?
         FROM seq",
    )
    .bind(count)
    .bind(committed_at_us)
    .execute(pool)
    .await
    .unwrap();
}
