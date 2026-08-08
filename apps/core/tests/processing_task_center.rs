#![allow(clippy::too_many_lines)]

mod common;

use std::collections::BTreeSet;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::processing::model::{
    ProcessingStage, ProcessingStatus, ProcessingTaskFilter, TaskCenterView,
};
use mediaflow_core::tasks::processing::service::ProcessingTaskService;
use uuid::Uuid;

#[tokio::test]
async fn four_views_and_summary_share_the_same_projection_semantics() {
    let (fixture, account, inbox, service) = setup().await;
    let queued = seed(&fixture, inbox, b"queued.mkv", 1, 101_000_000).await;
    let running = seed(&fixture, inbox, b"running.mkv", 2, 102_000_000).await;
    let waiting = seed(&fixture, inbox, b"waiting.mkv", 3, 103_000_000).await;
    let paused = seed(&fixture, inbox, b"paused.mkv", 4, 104_000_000).await;
    let cancelled = seed(&fixture, inbox, b"cancelled.mkv", 5, 105_000_000).await;
    set_state(
        fixture.pool(),
        running,
        "running",
        "identification",
        "pending",
        112_000_000,
    )
    .await;
    set_state(
        fixture.pool(),
        waiting,
        "waiting-confirmation",
        "identification",
        "waiting-confirmation",
        113_000_000,
    )
    .await;
    set_state(
        fixture.pool(),
        paused,
        "paused",
        "identification",
        "dependency-blocked",
        114_000_000,
    )
    .await;
    set_state(
        fixture.pool(),
        cancelled,
        "cancelled",
        "identification",
        "cancelled",
        115_000_000,
    )
    .await;

    let pending = service
        .list_center(
            account,
            &ProcessingTaskFilter::default(),
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&pending.items), BTreeSet::from([waiting, paused]));
    assert_eq!(pending.summary.pending, 2);
    assert_eq!(pending.summary.running, 2);
    assert_eq!(pending.summary.all, 5);
    assert_eq!(pending.summary.completed, 0);

    let running_page = service
        .list_center(
            account,
            &ProcessingTaskFilter {
                view: TaskCenterView::Running,
                ..ProcessingTaskFilter::default()
            },
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&running_page.items), BTreeSet::from([queued, running]));

    let all = service
        .list_center(
            account,
            &ProcessingTaskFilter {
                view: TaskCenterView::All,
                ..ProcessingTaskFilter::default()
            },
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(all.items.len(), 5);

    let completed = service
        .list_center(
            account,
            &ProcessingTaskFilter {
                view: TaskCenterView::Completed,
                ..ProcessingTaskFilter::default()
            },
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert!(completed.items.is_empty());

    let cancelled_only = service
        .list_center(
            account,
            &ProcessingTaskFilter {
                view: TaskCenterView::All,
                status: Some(ProcessingStatus::Cancelled),
                ..ProcessingTaskFilter::default()
            },
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&cancelled_only.items), BTreeSet::from([cancelled]));
}

#[tokio::test]
async fn combined_filters_bind_inbox_stage_status_and_escape_literal_like_metacharacters() {
    let (fixture, account, inbox, service) = setup().await;
    let literal = seed(
        &fixture,
        inbox,
        b"shows/literal_%_match.mkv",
        10,
        120_000_000,
    )
    .await;
    let other = seed(
        &fixture,
        inbox,
        b"shows/literal-xx-match.mkv",
        11,
        121_000_000,
    )
    .await;
    for (id, now) in [(literal, 130_000_000), (other, 131_000_000)] {
        set_state(
            fixture.pool(),
            id,
            "waiting-confirmation",
            "identification",
            "waiting-confirmation",
            now,
        )
        .await;
    }

    let filtered = service
        .list_center(
            account,
            &ProcessingTaskFilter {
                view: TaskCenterView::Pending,
                stage: Some(ProcessingStage::Identification),
                status: Some(ProcessingStatus::WaitingConfirmation),
                inbox_directory_id: Some(inbox),
                query: Some("%_".to_owned()),
            },
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&filtered.items), BTreeSet::from([literal]));
    assert_eq!(filtered.summary.pending, 1);
    assert_eq!(filtered.summary.all, 1);

    let id_query = service
        .list_center(
            account,
            &ProcessingTaskFilter {
                view: TaskCenterView::All,
                query: Some(literal.to_string()),
                ..ProcessingTaskFilter::default()
            },
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ids(&id_query.items), BTreeSet::from([literal]));
}

#[tokio::test]
async fn cursor_binds_every_filter_and_freezes_membership_summary_and_order() {
    let (fixture, account, inbox, service) = setup().await;
    let mut original = BTreeSet::new();
    let mut seeded = Vec::new();
    for index in 0..3_u8 {
        let task = seed(
            &fixture,
            inbox,
            format!("snapshot-{index}.mkv").as_bytes(),
            index + 20,
            140_000_000 + i64::from(index),
        )
        .await;
        original.insert(task);
        seeded.push(task);
    }
    let filter = ProcessingTaskFilter {
        view: TaskCenterView::All,
        ..ProcessingTaskFilter::default()
    };
    let first = service
        .list_center(account, &filter, &PageRequest::new(None, Some(2)).unwrap())
        .await
        .unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    assert_eq!(first.summary.all, 3);

    let inserted = seed(&fixture, inbox, b"snapshot-new.mkv", 30, 200_000_000).await;
    set_state(
        fixture.pool(),
        seeded[0],
        "waiting-confirmation",
        "identification",
        "waiting-confirmation",
        201_000_000,
    )
    .await;
    let second = service
        .list_center(
            account,
            &filter,
            &PageRequest::new(Some(cursor.clone()), Some(2)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.summary, second.summary);
    let observed = first
        .items
        .iter()
        .chain(&second.items)
        .map(|task| task.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(observed, original);
    assert!(!observed.contains(&inserted));
    assert_eq!(
        second
            .items
            .iter()
            .find(|task| task.id == seeded[0])
            .unwrap()
            .status,
        ProcessingStatus::Queued
    );

    let error = service
        .list_center(
            account,
            &ProcessingTaskFilter {
                view: TaskCenterView::Pending,
                ..ProcessingTaskFilter::default()
            },
            &PageRequest::new(Some(cursor), Some(2)).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::ValidationFailed);
}

async fn setup() -> (TestDb, Uuid, Uuid, ProcessingTaskService) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let service = ProcessingTaskService::new(db.pool().clone());
    (
        TestDb {
            _fixture: fixture,
            db,
        },
        account,
        inbox,
        service,
    )
}

struct TestDb {
    _fixture: common::TestConfigDir,
    db: mediaflow_core::platform::db::Db,
}

impl TestDb {
    fn pool(&self) -> &sqlx::SqlitePool {
        self.db.pool()
    }
}

async fn seed(fixture: &TestDb, inbox: Uuid, path: &[u8], identity: u8, now_us: i64) -> Uuid {
    let revision = common::seed_stable_revision(fixture.pool(), inbox, path, vec![identity]).await;
    ProcessingTaskService::new(fixture.pool().clone())
        .store()
        .ensure_revision(revision, now_us)
        .await
        .unwrap()
        .id
}

async fn set_state(
    pool: &sqlx::SqlitePool,
    id: Uuid,
    status: &str,
    stage: &str,
    checkpoint: &str,
    now_us: i64,
) {
    let running = status == "running";
    sqlx::query(
        "UPDATE tasks_processing_tasks
         SET status=?,stage=?,checkpoint=?,lease_owner=?,lease_expires_at_us=?,
             next_retry_at_us=?,updated_at_us=? WHERE id=?",
    )
    .bind(status)
    .bind(stage)
    .bind(checkpoint)
    .bind(running.then_some("task-center-test"))
    .bind(running.then_some(i64::MAX))
    .bind((status == "paused").then_some(now_us + 1_000_000))
    .bind(now_us)
    .bind(id.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
}

fn ids(items: &[mediaflow_core::tasks::processing::model::ProcessingTaskView]) -> BTreeSet<Uuid> {
    items.iter().map(|task| task.id).collect()
}
