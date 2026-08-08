mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::reconcile_service::{
    ReconcileRequestService, ReconcileRequestStatus,
};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::shared::error::ErrorCode;

#[tokio::test]
async fn request_keys_replay_conflict_reuse_active_scan_and_freeze_terminal_counts() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let first_inbox = common::seed_inbox(db.pool()).await;
    let second_inbox = seed_distinct_inbox(db.pool()).await;
    let service = ReconcileRequestService::new(db.pool().clone(), OutboxNotifier::new());

    let first = service
        .accept(first_inbox, "automation:event-001", 100)
        .await
        .unwrap();
    assert_eq!(first.status, ReconcileRequestStatus::Accepted);
    let replay = service
        .accept(first_inbox, "automation:event-001", 101)
        .await
        .unwrap();
    assert_eq!(replay, first);
    let conflict = service
        .accept(second_inbox, "automation:event-001", 102)
        .await
        .unwrap_err();
    assert_eq!(conflict.code(), ErrorCode::RequestConflict);

    let second = service
        .accept(first_inbox, "automation:event-002", 103)
        .await
        .unwrap();
    assert_ne!(second.id, first.id);
    assert_eq!(second.scan_task_id, first.scan_task_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_scan_tasks WHERE inbox_directory_id=?"
        )
        .bind(first_inbox.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );

    sqlx::query(
        "UPDATE tasks_scan_tasks SET status='completed',stage='finished',
         observed_files=7,errors=2,updated_at_us=200 WHERE id=?",
    )
    .bind(first.scan_task_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let completed = service
        .accept(first_inbox, "automation:event-001", 201)
        .await
        .unwrap();
    assert_eq!(completed.status, ReconcileRequestStatus::Completed);
    assert_eq!(completed.observed_files, Some(7));
    assert_eq!(completed.errors, Some(2));
    sqlx::query("UPDATE tasks_scan_tasks SET observed_files=99,errors=99 WHERE id=?")
        .bind(first.scan_task_id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        service
            .accept(first_inbox, "automation:event-001", 202)
            .await
            .unwrap(),
        completed
    );
}

#[tokio::test]
async fn unavailable_inbox_rejects_without_creating_a_request_or_scan() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    sqlx::query("UPDATE discovery_inbox_directories SET health='unavailable' WHERE id=?")
        .bind(inbox.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let service = ReconcileRequestService::new(db.pool().clone(), OutboxNotifier::new());

    let error = service
        .accept(inbox, "automation:disabled-inbox", 100)
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::RootUnavailable);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_reconcile_requests")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_scan_tasks")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

async fn seed_distinct_inbox(pool: &sqlx::SqlitePool) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'second',x'2e','.',x'03',x'04','available',0,1,0,0)",
    )
    .bind(id.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
    id
}
