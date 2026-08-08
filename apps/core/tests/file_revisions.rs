mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::auxiliary::AuxiliaryReason;
use mediaflow_core::discovery::observations::FileObservation;
use mediaflow_core::discovery::policy::PutDiscoveryPolicyCommand;
use mediaflow_core::discovery::revisions::{
    ObservationSource, RevisionObserver, RevisionService, RevisionStatus,
};
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::tasks::model::{NewScanTask, ScanCounts};
use mediaflow_core::tasks::store::TaskStore;
use sqlx::Row;

#[tokio::test]
async fn identical_observations_create_one_revision_and_only_stabilize_after_both_gates() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let service = RevisionService::new(db.pool().clone());
    let file = observation(inbox, b"movies/movie.mkv", b"identity-a", 100, 0);

    let first = service
        .observe(file.clone(), ObservationSource::Reconcile, 60_000_000)
        .await
        .unwrap();
    assert_eq!(first.status, RevisionStatus::Observing);
    assert_eq!(first.matching_observations, 1);
    assert!(!first.processing_requested);

    let too_soon = service
        .observe(file.clone(), ObservationSource::Watcher, 89_999_999)
        .await
        .unwrap();
    assert_eq!(too_soon.revision_id, first.revision_id);
    assert_eq!(too_soon.matching_observations, 1);

    let stable = service
        .observe(file.clone(), ObservationSource::Reconcile, 90_000_000)
        .await
        .unwrap();
    assert_eq!(stable.status, RevisionStatus::Stable);
    assert_eq!(stable.matching_observations, 2);
    assert!(stable.processing_requested);

    let duplicate = service
        .observe(file, ObservationSource::Scan, 90_000_001)
        .await
        .unwrap();
    assert_eq!(duplicate.revision_id, stable.revision_id);
    assert_eq!(duplicate.status, RevisionStatus::Stable);
    assert!(!duplicate.processing_requested);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_processing_requests")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn replacement_missing_and_reappearance_preserve_immutable_revision_history() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let service = RevisionService::new(db.pool().clone());
    let first = observation(inbox, b"movie.mkv", b"identity-a", 100, 0);
    let first = service
        .observe(first.clone(), ObservationSource::Scan, 60_000_000)
        .await
        .unwrap();

    let replacement_observation = observation(inbox, b"movie.mkv", b"identity-b", 101, 1);
    let replacement = service
        .observe(
            replacement_observation.clone(),
            ObservationSource::Watcher,
            61_000_000,
        )
        .await
        .unwrap();
    assert_ne!(replacement.revision_id, first.revision_id);
    assert_eq!(replacement.status, RevisionStatus::Observing);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM discovery_file_revision_states WHERE revision_id=?",
        )
        .bind(first.revision_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "superseded"
    );

    assert!(
        service
            .mark_missing(inbox, b"movie.mkv", 70_000_000)
            .await
            .unwrap()
    );
    let reappeared = service
        .observe(
            replacement_observation,
            ObservationSource::Reconcile,
            71_000_000,
        )
        .await
        .unwrap();
    assert_eq!(reappeared.revision_id, replacement.revision_id);
    assert_eq!(reappeared.status, RevisionStatus::Observing);
    assert_eq!(reappeared.matching_observations, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_file_revisions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        2
    );

    assert!(
        service
            .mark_missing(inbox, b"movie.mkv", 72_000_000)
            .await
            .unwrap()
    );
    let new_file = service
        .observe(
            observation(inbox, b"movie.mkv", b"identity-c", 102, 2),
            ObservationSource::Reconcile,
            73_000_000,
        )
        .await
        .unwrap();
    assert_ne!(new_file.revision_id, replacement.revision_id);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM discovery_file_revision_states WHERE revision_id=?",
        )
        .bind(replacement.revision_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "missing"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_file_revisions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        3
    );
}

#[tokio::test]
async fn concurrent_duplicate_observation_has_one_file_and_revision() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let service = RevisionService::new(db.pool().clone());
    let file = observation(inbox, "并发/电影.mkv".as_bytes(), b"identity", 1, 0);
    let (left, right) = tokio::join!(
        service.observe(file.clone(), ObservationSource::Watcher, 60_000_000),
        service.observe(file, ObservationSource::Reconcile, 60_000_000),
    );
    assert_eq!(left.unwrap().revision_id, right.unwrap().revision_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_tracked_files")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_file_revisions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn policy_changes_only_apply_to_new_revisions_and_due_query_is_indexed() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let revisions = RevisionService::new(db.pool().clone());
    let policies = DiscoveryService::policy_only(db.pool().clone());
    let original = observation(inbox, b"movie.mkv", b"identity-a", 100, 0);
    let first = revisions
        .observe(original.clone(), ObservationSource::Scan, 60_000_000)
        .await
        .unwrap();
    policies
        .put_policy(
            inbox,
            PutDiscoveryPolicyCommand {
                minimum_age_seconds: 120,
                stable_observation_interval_seconds: 45,
                reconcile_interval_seconds: 1_800,
                watcher_enabled: true,
            },
            1,
        )
        .await
        .unwrap();

    let stable = revisions
        .observe(original, ObservationSource::Reconcile, 90_000_000)
        .await
        .unwrap();
    assert_eq!(stable.revision_id, first.revision_id);
    assert_eq!(stable.status, RevisionStatus::Stable);
    let newer = revisions
        .observe(
            observation(inbox, b"movie-2.mkv", b"identity-b", 200, 0),
            ObservationSource::Watcher,
            91_000_000,
        )
        .await
        .unwrap();
    let snapshot: (i64, i64, i64) = sqlx::query_as(
        "SELECT policy_version,minimum_age_seconds,stable_observation_interval_seconds
         FROM discovery_file_revisions WHERE id=?",
    )
    .bind(newer.revision_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(snapshot, (2, 120, 45));

    let plan = sqlx::query(
        "EXPLAIN QUERY PLAN SELECT revision_id FROM discovery_file_revision_states
         WHERE status='observing' AND next_check_at_us<=? ORDER BY next_check_at_us,revision_id",
    )
    .bind(200_000_000_i64)
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert!(plan.iter().any(|row| {
        row.get::<String, _>("detail")
            .contains("discovery_revision_states_due_idx")
    }));
}

#[tokio::test]
async fn auxiliary_and_non_utf8_paths_preserve_reason_and_never_use_file_size() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let service = RevisionService::new(db.pool().clone());
    let trailer = observation(inbox, b"Extras/trailer.mkv", b"aux", 1, 0);
    let skipped = service
        .observe(trailer, ObservationSource::Reconcile, 60_000_000)
        .await
        .unwrap();
    assert_eq!(skipped.status, RevisionStatus::SkippedAuxiliary);
    assert_eq!(skipped.skip_reason, Some(AuxiliaryReason::Extra));
    assert!(!skipped.processing_requested);
    service
        .mark_missing(inbox, b"Extras/trailer.mkv", 70_000_000)
        .await
        .unwrap();
    let skipped_again = service
        .observe(
            observation(inbox, b"Extras/trailer.mkv", b"aux", 1, 0),
            ObservationSource::Reconcile,
            71_000_000,
        )
        .await
        .unwrap();
    assert_eq!(skipped_again.status, RevisionStatus::SkippedAuxiliary);
    assert_eq!(skipped_again.skip_reason, Some(AuxiliaryReason::Extra));

    let raw_path = b"damaged-\xff/movie.mkv";
    let ordinary = FileObservation {
        inbox_directory_id: inbox,
        relative_path_bytes: raw_path.to_vec(),
        relative_path_display: "damaged-�/movie.mkv".to_owned(),
        identity_snapshot: b"ordinary".to_vec(),
        size_bytes: 1,
        modified_at_ns: 0,
    };
    service
        .observe(ordinary.clone(), ObservationSource::Watcher, 60_000_000)
        .await
        .unwrap();
    let stable = service
        .observe(ordinary, ObservationSource::Reconcile, 90_000_000)
        .await
        .unwrap();
    assert_eq!(stable.status, RevisionStatus::Stable);
    assert!(stable.processing_requested);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM discovery_processing_requests WHERE revision_id=?",
        )
        .bind(skipped.revision_id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn scan_snapshot_and_revision_write_roll_back_and_commit_together() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let account = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let store = TaskStore::new(db.pool().clone());
    store
        .create(NewScanTask {
            account_id: account,
            inbox_directory_id: inbox,
            idempotency_key: "m3-atomic-revision".to_owned(),
            now_us: 1,
        })
        .await
        .unwrap();
    let lease = store
        .claim_next("m3-worker", 60_000_000)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_revision BEFORE INSERT ON discovery_file_revisions
         BEGIN SELECT RAISE(ABORT,'injected revision failure'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let file = observation(inbox, b"atomic.mkv", b"identity", 1, 0);
    assert!(
        store
            .record_observations(
                &lease,
                std::slice::from_ref(&file),
                &[],
                ScanCounts::default(),
                60_100_000,
            )
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_files")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER fail_revision")
        .execute(db.pool())
        .await
        .unwrap();
    store
        .record_observations(&lease, &[file], &[], ScanCounts::default(), 60_100_000)
        .await
        .unwrap();
    for table in [
        "discovery_files",
        "discovery_scan_file_observations",
        "discovery_file_revisions",
    ] {
        let count = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1, "{table}");
    }
}

fn observation(
    inbox_directory_id: uuid::Uuid,
    path: &[u8],
    identity: &[u8],
    size_bytes: u64,
    modified_at_ns: i64,
) -> FileObservation {
    FileObservation {
        inbox_directory_id,
        relative_path_bytes: path.to_vec(),
        relative_path_display: String::from_utf8_lossy(path).into_owned(),
        identity_snapshot: identity.to_vec(),
        size_bytes,
        modified_at_ns,
    }
}
