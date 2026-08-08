mod common;

use sha2::{Digest as _, Sha256};

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::observations::FileObservation;
use mediaflow_core::discovery::revisions::{ObservationSource, RevisionObserver, RevisionService};
use mediaflow_core::identification::decision::{
    DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use mediaflow_core::identification::evidence::{
    EvidenceDraft, EvidenceKind, EvidenceSource, EvidenceStrength,
};
use mediaflow_core::identification::store::{
    IdentificationCommit, IdentificationCommitStatus, IdentificationStore,
};
use mediaflow_core::platform::migrations::migrate_with_backup;

#[tokio::test]
async fn changed_current_revision_ends_attempt_without_candidate_decision_or_review() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let path = b"movies/replaced.mkv";
    let lease = common::seed_processing_lease(db.pool(), inbox, path, vec![1], "revision-a").await;
    let store = IdentificationStore::new(db.pool().clone());
    let attempt = store
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();

    RevisionService::new(db.pool().clone())
        .observe(
            FileObservation {
                inbox_directory_id: inbox,
                relative_path_bytes: path.to_vec(),
                relative_path_display: "movies/replaced.mkv".to_owned(),
                identity_snapshot: vec![2],
                size_bytes: 101,
                modified_at_ns: 1,
            },
            ObservationSource::Watcher,
            94_000_000,
        )
        .await
        .unwrap();

    let outcome = store
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &[evidence("replaced")],
            candidates: &[],
            decision: &decision(),
            title_hint: Some("replaced"),
            now_us: 95_000_000,
        })
        .await
        .unwrap();
    assert_eq!(outcome.status, IdentificationCommitStatus::RevisionChanged);
    for table in [
        "identification_evidence",
        "identification_candidates",
        "identification_decisions",
        "identification_review_cases",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0,
            "{table}"
        );
    }
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM identification_attempts WHERE id=?")
            .bind(attempt.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "revision-changed"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT failure_code FROM identification_attempts WHERE id=?"
        )
        .bind(attempt.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "identification.revision-changed"
    );
}

#[tokio::test]
async fn commit_is_database_only_and_preserves_source_media_and_nfo_hashes() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let media = fixture.config().config_dir.join("source.mkv");
    let nfo = fixture.config().config_dir.join("source.nfo");
    std::fs::write(&media, b"immutable-media").unwrap();
    std::fs::write(&nfo, b"<movie><title>Immutable</title></movie>").unwrap();
    let before = (hash(&media), hash(&nfo));
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease =
        common::seed_processing_lease(db.pool(), inbox, b"source.mkv", vec![7], "hash-a").await;
    let store = IdentificationStore::new(db.pool().clone());
    let attempt = store
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    store
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &[evidence("immutable")],
            candidates: &[],
            decision: &decision(),
            title_hint: Some("immutable"),
            now_us: 94_000_000,
        })
        .await
        .unwrap();
    assert_eq!((hash(&media), hash(&nfo)), before);
    for table in [
        "catalog_media_items",
        "catalog_media_nodes",
        "catalog_media_versions",
        "catalog_file_assets",
        "catalog_version_files",
        "catalog_metadata_values",
        "catalog_artwork_refs",
        "catalog_applied_local_results",
        "catalog_media_order_history",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(db.pool())
                .await
                .unwrap(),
            0,
            "identification commit must not populate {table}"
        );
    }
}

fn evidence(value: &str) -> EvidenceDraft {
    EvidenceDraft {
        source: EvidenceSource::Filename,
        source_version: "filename-v1".to_owned(),
        kind: EvidenceKind::Title,
        normalized_value: value.to_owned(),
        strength: EvidenceStrength::Strong,
        reason: "filename.title".to_owned(),
        source_hash: Sha256::digest(value.as_bytes()).into(),
    }
}

fn decision() -> IdentificationDecisionDraft {
    IdentificationDecisionDraft {
        level: DecisionLevel::Unidentified,
        selected_candidate: None,
        reasons: vec![DecisionReason::NoCandidate],
        retry_at_us: None,
        graph: None,
        rule_version: 1,
    }
}

fn hash(path: &std::path::Path) -> [u8; 32] {
    Sha256::digest(std::fs::read(path).unwrap()).into()
}
