#![allow(clippy::too_many_lines)]

mod common;

use mediaflow_core::discovery::observations::{FileObservation, ScanEntryError};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::model::{NewScanTask, ScanCounts};
use mediaflow_core::tasks::store::TaskStore;
use uuid::Uuid;

#[tokio::test]
async fn task_keyset_is_stable_for_concurrent_inserts_and_invalid_cursors_fail_closed() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    let inbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let store = TaskStore::new(db.pool().clone());
    for index in 0..5 {
        store
            .create(NewScanTask {
                account_id,
                inbox_directory_id: inbox_id,
                idempotency_key: format!("page-{index}"),
                now_us: 1_000 + index,
            })
            .await
            .unwrap();
    }
    let newest = store
        .claim_next("page-worker", 20_000)
        .await
        .unwrap()
        .unwrap();
    let observations = ["b.mkv", "a.mkv"].map(|path| FileObservation {
        inbox_directory_id: inbox_id,
        relative_path_bytes: path.as_bytes().to_vec(),
        relative_path_display: path.to_owned(),
        identity_snapshot: vec![1; 25],
        size_bytes: 1,
        modified_at_ns: 1,
    });
    let errors = ["z.mkv", "y.mkv"].map(|path| ScanEntryError {
        code: "entry.unavailable",
        scope: "entry",
        relative_path_bytes: path.as_bytes().to_vec(),
        relative_path_display: path.to_owned(),
    });
    let newest = store
        .record_observations(
            &newest,
            &observations,
            &errors,
            ScanCounts {
                observed_files: 2,
                errors: 2,
                ..ScanCounts::default()
            },
            21_000,
        )
        .await
        .unwrap();
    let files_one = store
        .list_files(
            account_id,
            newest.task.id,
            &PageRequest::new(None, Some(1)).unwrap(),
        )
        .await
        .unwrap();
    let errors_one = store
        .list_errors(
            account_id,
            newest.task.id,
            &PageRequest::new(None, Some(1)).unwrap(),
        )
        .await
        .unwrap();
    let file_cursor = files_one.next_cursor.clone().unwrap();
    let error_cursor = errors_one.next_cursor.clone().unwrap();
    let concurrent_file = FileObservation {
        inbox_directory_id: inbox_id,
        relative_path_bytes: b"aa-concurrent.mkv".to_vec(),
        relative_path_display: "aa-concurrent.mkv".to_owned(),
        identity_snapshot: vec![2; 25],
        size_bytes: 2,
        modified_at_ns: 2,
    };
    let concurrent_error = ScanEntryError {
        code: "entry.changed",
        scope: "entry",
        relative_path_bytes: b"concurrent-error.mkv".to_vec(),
        relative_path_display: "concurrent-error.mkv".to_owned(),
    };
    let newest = store
        .record_observations(
            &newest,
            &[concurrent_file],
            &[concurrent_error],
            ScanCounts {
                observed_files: 3,
                errors: 3,
                ..ScanCounts::default()
            },
            22_000,
        )
        .await
        .unwrap();
    sqlx::query(
        "DELETE FROM discovery_scan_file_observations
         WHERE scan_batch_id=? AND discovered_file_id=?",
    )
    .bind(newest.batch_id.as_bytes().as_slice())
    .bind(files_one.items[0].id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query("DELETE FROM discovery_files WHERE id=?")
        .bind(files_one.items[0].id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let files_two = store
        .list_files(
            account_id,
            newest.task.id,
            &PageRequest::new(Some(file_cursor.clone()), Some(200)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(files_one.items[0].relative_path, "a.mkv");
    assert_eq!(
        files_two
            .items
            .iter()
            .map(|item| item.relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["b.mkv"],
        "the first-page snapshot excludes later file inserts"
    );
    let errors_two = store
        .list_errors(
            account_id,
            newest.task.id,
            &PageRequest::new(Some(error_cursor.clone()), Some(200)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(errors_one.items.len() + errors_two.items.len(), 2);
    assert!(
        errors_two
            .items
            .iter()
            .all(|item| item.code != "entry.changed"),
        "the first-page snapshot excludes later error inserts"
    );
    let another_task = store
        .list_tasks(account_id, &PageRequest::new(None, Some(200)).unwrap())
        .await
        .unwrap()
        .items
        .into_iter()
        .find(|task| task.id != newest.task.id)
        .unwrap();
    let wrong_task = store
        .list_files(
            account_id,
            another_task.id,
            &PageRequest::new(Some(file_cursor.clone()), Some(1)).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(wrong_task.code(), ErrorCode::ValidationFailed);
    let mut tampered = file_cursor.into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();
    let tampered_error = store
        .list_files(
            account_id,
            newest.task.id,
            &PageRequest::new(Some(tampered), Some(1)).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(tampered_error.code(), ErrorCode::ValidationFailed);
    assert!(PageRequest::new(Some("A".repeat(513)), Some(1)).is_err());
    let first = store
        .list_tasks(account_id, &PageRequest::new(None, Some(2)).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: "page-concurrent".to_owned(),
            now_us: 10_000,
        })
        .await
        .unwrap();
    let second = store
        .list_tasks(
            account_id,
            &PageRequest::new(first.next_cursor, Some(200)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 3);
    let invalid = store
        .list_tasks(
            account_id,
            &PageRequest::new(Some("not-a-versioned-cursor".to_owned()), None).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.code(), ErrorCode::ValidationFailed);
}

#[tokio::test]
async fn file_pagination_accepts_long_relative_paths_without_exceeding_the_cursor_bound() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    let inbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();

    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: "long-path-page".to_owned(),
            now_us: 1_000,
        })
        .await
        .unwrap();
    let claim = store
        .claim_next("long-path-worker", 2_000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claim.task.id, task.id);

    let prefix = "a".repeat(100);
    let observations = ["a.mkv", "b.mkv"].map(|name| {
        let path = format!("{prefix}/{name}");
        FileObservation {
            inbox_directory_id: inbox_id,
            relative_path_bytes: path.as_bytes().to_vec(),
            relative_path_display: path,
            identity_snapshot: vec![1; 25],
            size_bytes: 1,
            modified_at_ns: 1,
        }
    });
    store
        .record_observations(
            &claim,
            &observations,
            &[],
            ScanCounts {
                observed_files: 2,
                ..ScanCounts::default()
            },
            3_000,
        )
        .await
        .unwrap();

    let first = store
        .list_files(
            account_id,
            task.id,
            &PageRequest::new(None, Some(1)).unwrap(),
        )
        .await
        .expect("长相对路径应生成有界游标");
    let next_cursor = first.next_cursor.expect("应返回下一页游标");
    assert!(next_cursor.len() <= mediaflow_core::shared::page::MAX_CURSOR_BYTES);

    let second = store
        .list_files(
            account_id,
            task.id,
            &PageRequest::new(Some(next_cursor), Some(1)).unwrap(),
        )
        .await
        .expect("长相对路径游标应可继续分页");
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].id, second.items[0].id);
}

#[tokio::test]
async fn task_snapshot_freezes_updated_order_when_unreturned_and_returned_tasks_move() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    let inbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let mut created = Vec::new();
    for index in 0..6 {
        created.push(
            store
                .create(NewScanTask {
                    account_id,
                    inbox_directory_id: inbox_id,
                    idempotency_key: format!("moving-page-{index}"),
                    now_us: 1_000 + index,
                })
                .await
                .unwrap()
                .id,
        );
    }

    let first = store
        .list_tasks(account_id, &PageRequest::new(None, Some(2)).unwrap())
        .await
        .unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    assert_eq!(
        first.items.iter().map(|task| task.id).collect::<Vec<_>>(),
        vec![created[5], created[4]]
    );
    let wrong_account = store
        .list_tasks(
            Uuid::now_v7(),
            &PageRequest::new(Some(cursor.clone()), Some(2)).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(wrong_account.code(), ErrorCode::ValidationFailed);
    let mut tampered = cursor.clone().into_bytes();
    let last = tampered.len() - 1;
    tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
    let tampered = store
        .list_tasks(
            account_id,
            &PageRequest::new(Some(String::from_utf8(tampered).unwrap()), Some(2)).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(tampered.code(), ErrorCode::ValidationFailed);

    sqlx::query("UPDATE tasks_scan_tasks SET updated_at_us=3000 WHERE id=?")
        .bind(created[2].as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE tasks_scan_tasks SET updated_at_us=1 WHERE id=?")
        .bind(created[5].as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();

    let second = store
        .list_tasks(
            account_id,
            &PageRequest::new(Some(cursor), Some(200)).unwrap(),
        )
        .await
        .unwrap();
    let mut paged = first
        .items
        .into_iter()
        .chain(second.items)
        .map(|task| task.id)
        .collect::<Vec<_>>();
    let mut expected = created;
    paged.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        paged, expected,
        "a task snapshot must neither omit a task that moved forward nor repeat one that moved backward"
    );
}
