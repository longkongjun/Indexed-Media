mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{CandidateIdentity, ProviderMediaKind};
use mediaflow_core::identification::decision::{
    DecisionCandidate, DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use mediaflow_core::identification::evidence::{
    EvidenceDraft, EvidenceKind, EvidenceSource, EvidenceStrength,
};
use mediaflow_core::identification::store::{IdentificationCommit, IdentificationStore};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxReader;
use mediaflow_core::tasks::events::TaskEventEnvelope;
use uuid::Uuid;

#[tokio::test]
async fn one_attempt_persists_immutable_ordered_evidence_candidates_decision_and_event() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/dune-2021.mkv",
        vec![1],
        "identify-a",
    )
    .await;
    let store = IdentificationStore::new(db.pool().clone());
    let attempt = store
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    let candidate = movie_candidate("Dune", 2021);
    let evidence = vec![title_evidence("dune")];
    let decision = confirmed(candidate.id);

    let committed = store
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &evidence,
            candidates: std::slice::from_ref(&candidate),
            decision: &decision,
            title_hint: Some("dune"),
            now_us: 94_000_000,
        })
        .await
        .unwrap();
    assert_eq!(committed.level, Some(DecisionLevel::Confirmed));
    assert!(committed.review_case_id.is_none());
    assert_eq!(count(db.pool(), "identification_attempts").await, 1);
    assert_eq!(count(db.pool(), "identification_evidence").await, 1);
    assert_eq!(count(db.pool(), "identification_candidates").await, 1);
    assert_eq!(count(db.pool(), "identification_decisions").await, 1);
    assert_eq!(count(db.pool(), "identification_decision_reasons").await, 2);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT normalized_value FROM identification_evidence WHERE attempt_id=?",
        )
        .bind(attempt.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "dune"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM identification_candidate_episodes WHERE candidate_id=?",
        )
        .bind(candidate.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0
    );

    let events = OutboxReader::new(db.pool().clone())
        .after(0, 20)
        .await
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TaskEventEnvelope::ProcessingTaskIdentificationDecided { task_id, payload, .. }
            if *task_id == lease.task.id
                && Some(payload.decision_id) == committed.decision_id
                && payload.level == DecisionLevel::Confirmed
    )));

    assert!(
        store
            .commit(IdentificationCommit {
                lease: &lease,
                attempt_id: attempt.id,
                evidence: &evidence,
                candidates: std::slice::from_ref(&candidate),
                decision: &decision,
                title_hint: Some("dune"),
                now_us: 95_000_000,
            })
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE identification_evidence SET normalized_value='changed'")
            .execute(db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn outbox_failure_rolls_back_evidence_candidates_decision_review_and_task_checkpoint() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/unknown.mkv",
        vec![2],
        "identify-b",
    )
    .await;
    let store = IdentificationStore::new(db.pool().clone());
    let attempt = store
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_identification_event BEFORE INSERT ON platform_outbox_events
         WHEN NEW.event_type='processing-task.identification-decided'
         BEGIN SELECT RAISE(ABORT, 'fixture failure'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let decision = IdentificationDecisionDraft {
        level: DecisionLevel::Unidentified,
        selected_candidate: None,
        reasons: vec![DecisionReason::NoCandidate],
        retry_at_us: None,
        graph: None,
        rule_version: 1,
    };

    assert!(
        store
            .commit(IdentificationCommit {
                lease: &lease,
                attempt_id: attempt.id,
                evidence: &[title_evidence("unknown")],
                candidates: &[],
                decision: &decision,
                title_hint: Some("unknown"),
                now_us: 94_000_000,
            })
            .await
            .is_err()
    );
    assert_eq!(count(db.pool(), "identification_evidence").await, 0);
    assert_eq!(count(db.pool(), "identification_decisions").await, 0);
    assert_eq!(count(db.pool(), "identification_review_cases").await, 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM tasks_processing_tasks WHERE id=?")
            .bind(lease.task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "running"
    );
}

fn title_evidence(value: &str) -> EvidenceDraft {
    EvidenceDraft {
        source: EvidenceSource::Filename,
        source_version: "filename-v1".to_owned(),
        kind: EvidenceKind::Title,
        normalized_value: value.to_owned(),
        strength: EvidenceStrength::Strong,
        reason: "filename.title".to_owned(),
        source_hash: [7; 32],
    }
}

fn movie_candidate(title: &str, year: u16) -> DecisionCandidate {
    DecisionCandidate {
        id: Uuid::now_v7(),
        identity: CandidateIdentity {
            provider_id: 438_631,
            media_kind: ProviderMediaKind::Movie,
        },
        titles: vec![title.to_owned()],
        aliases: vec!["沙丘".to_owned()],
        year: Some(year),
        locale: "en-US".to_owned(),
        original_title: Some(title.to_owned()),
        release_dates: Vec::new(),
        episodes: Vec::new(),
        external_ids: Vec::new(),
        ranking_score: 95,
        provider_version: 1,
    }
}

fn confirmed(candidate_id: Uuid) -> IdentificationDecisionDraft {
    IdentificationDecisionDraft {
        level: DecisionLevel::Confirmed,
        selected_candidate: Some(candidate_id),
        reasons: vec![DecisionReason::TitleMatched, DecisionReason::YearMatched],
        retry_at_us: None,
        graph: None,
        rule_version: 1,
    }
}

async fn count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}
