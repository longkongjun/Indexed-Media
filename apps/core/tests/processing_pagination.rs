mod common;

use std::collections::BTreeSet;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::processing::store::ProcessingStore;

#[tokio::test]
async fn task_cursor_freezes_membership_and_order_across_concurrent_updates_and_inserts() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let store = ProcessingStore::new(db.pool().clone());
    let mut original = Vec::new();
    for index in 0..3_u8 {
        let path = format!("movie-{index}.mkv");
        let revision =
            common::seed_stable_revision(db.pool(), inbox, path.as_bytes(), vec![index]).await;
        original.push(
            store
                .ensure_revision(revision, 100_000_000 + i64::from(index))
                .await
                .unwrap()
                .id,
        );
    }

    let first = store
        .list_tasks(account, &PageRequest::new(None, Some(2)).unwrap())
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let cursor = first.next_cursor.clone().unwrap();
    sqlx::query("UPDATE tasks_processing_tasks SET updated_at_us=200000000 WHERE id=?")
        .bind(first.items[0].id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let unreturned = original
        .iter()
        .copied()
        .find(|id| first.items.iter().all(|task| task.id != *id))
        .unwrap();
    sqlx::query("UPDATE tasks_processing_tasks SET updated_at_us=200000001 WHERE id=?")
        .bind(unreturned.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
    let revision =
        common::seed_stable_revision(db.pool(), inbox, b"new-after-snapshot.mkv", vec![9]).await;
    let inserted = store.ensure_revision(revision, 300_000_000).await.unwrap();

    let second = store
        .list_tasks(
            account,
            &PageRequest::new(Some(cursor.clone()), Some(2)).unwrap(),
        )
        .await
        .unwrap();
    let observed = first
        .items
        .iter()
        .chain(&second.items)
        .map(|task| task.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(observed, original.into_iter().collect());
    assert!(!observed.contains(&inserted.id));
    assert!(second.next_cursor.is_none());

    let mut decoded = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        cursor.as_bytes(),
    )
    .unwrap();
    let midpoint = decoded.len() / 2;
    decoded[midpoint] ^= 1;
    let tampered =
        base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, decoded);
    let error = store
        .list_tasks(account, &PageRequest::new(Some(tampered), Some(2)).unwrap())
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ValidationFailed);
}
