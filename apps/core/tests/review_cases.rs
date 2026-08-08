mod common;

use std::collections::BTreeSet;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::identification::decision::{
    DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use mediaflow_core::identification::review::{ReviewCaseFilter, ReviewCaseStore};
use mediaflow_core::identification::store::{IdentificationCommit, IdentificationStore};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::page::PageRequest;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn only_three_uncertain_levels_create_cases_and_confirmed_retry_closes_the_projection() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let processing =
        mediaflow_core::tasks::processing::store::ProcessingStore::new(db.pool().clone());
    let identification = IdentificationStore::new(db.pool().clone());
    let reviews = ReviewCaseStore::new(db.pool().clone());

    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/probable.mkv",
        vec![1],
        "review-a",
    )
    .await;
    let attempt = identification
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    let probable = decision(DecisionLevel::Probable, DecisionReason::YearMissing);
    let committed = identification
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &[],
            candidates: &[],
            decision: &probable,
            title_hint: Some("probable"),
            now_us: 94_000_000,
        })
        .await
        .unwrap();
    assert!(committed.review_case_id.is_some());

    processing
        .retry(account, lease.task.id, "retry-probable", 95_000_000)
        .await
        .unwrap();
    let retry_lease = processing
        .claim_next(
            "review-retry",
            &[mediaflow_core::tasks::processing::model::ProcessingStage::Identification],
            96_000_000,
        )
        .await
        .unwrap()
        .unwrap();
    let retry_attempt = identification
        .begin_attempt(&retry_lease, "filename-v1", "tmdb-v1", 1, 96_000_001)
        .await
        .unwrap();
    let candidate = mediaflow_core::identification::decision::DecisionCandidate {
        id: uuid::Uuid::now_v7(),
        identity: mediaflow_core::connectors::model::CandidateIdentity {
            provider_id: 1,
            media_kind: mediaflow_core::connectors::model::ProviderMediaKind::Movie,
        },
        titles: vec!["probable".to_owned()],
        aliases: Vec::new(),
        year: Some(2026),
        locale: "en-US".to_owned(),
        original_title: None,
        release_dates: Vec::new(),
        episodes: Vec::new(),
        external_ids: Vec::new(),
        ranking_score: 0,
        provider_version: 1,
    };
    let mut confirmed = decision(DecisionLevel::Confirmed, DecisionReason::ExternalIdVerified);
    confirmed.selected_candidate = Some(candidate.id);
    identification
        .commit(IdentificationCommit {
            lease: &retry_lease,
            attempt_id: retry_attempt.id,
            evidence: &[],
            candidates: std::slice::from_ref(&candidate),
            decision: &confirmed,
            title_hint: Some("probable"),
            now_us: 97_000_000,
        })
        .await
        .unwrap();

    let page = reviews
        .list_active(
            account,
            &ReviewCaseFilter::default(),
            &PageRequest::new(None, Some(10)).unwrap(),
        )
        .await
        .unwrap();
    assert!(page.items.is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM identification_review_cases WHERE task_id=? AND status='closed'",
        )
        .bind(lease.task.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );

    for (index, level) in [
        DecisionLevel::Ambiguous,
        DecisionLevel::Unidentified,
        DecisionLevel::Blocked,
    ]
    .into_iter()
    .enumerate()
    {
        let index_u8 = u8::try_from(index).unwrap();
        let index_i64 = i64::try_from(index).unwrap();
        let owner = format!("review-{index}");
        let path = format!("movies/{index}.mkv");
        let lease = common::seed_processing_lease(
            db.pool(),
            inbox,
            path.as_bytes(),
            vec![index_u8 + 3],
            &owner,
        )
        .await;
        let attempt = identification
            .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 100_000_000 + index_i64)
            .await
            .unwrap();
        identification
            .commit(IdentificationCommit {
                lease: &lease,
                attempt_id: attempt.id,
                evidence: &[],
                candidates: &[],
                decision: &decision(
                    level,
                    if level == DecisionLevel::Blocked {
                        DecisionReason::ProviderUnavailable
                    } else if level == DecisionLevel::Ambiguous {
                        DecisionReason::MultipleStrongCandidates
                    } else {
                        DecisionReason::NoCandidate
                    },
                ),
                title_hint: Some(&owner),
                now_us: 110_000_000 + index_i64,
            })
            .await
            .unwrap();
    }
    let page = reviews
        .list_active(
            account,
            &ReviewCaseFilter::default(),
            &PageRequest::new(None, Some(10)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        page.items
            .iter()
            .map(|case| case.level)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([DecisionLevel::Ambiguous, DecisionLevel::Unidentified])
    );
    assert!(page.items.iter().all(|case| case.version == 1));
    assert!(
        page.items
            .iter()
            .all(|case| case.allowed_actions.len() == 3 && case.latest_task_decision.is_none())
    );
}

#[tokio::test]
async fn review_cursor_freezes_filtered_membership_and_order() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let identification = IdentificationStore::new(db.pool().clone());
    let reviews = ReviewCaseStore::new(db.pool().clone());
    let mut original = BTreeSet::new();
    for index in 0..3_u8 {
        let path = format!("movies/page-{index}.mkv");
        let lease = common::seed_processing_lease(
            db.pool(),
            inbox,
            path.as_bytes(),
            vec![index],
            &format!("page-{index}"),
        )
        .await;
        let attempt = identification
            .begin_attempt(
                &lease,
                "filename-v1",
                "tmdb-v1",
                1,
                100_000_000 + i64::from(index),
            )
            .await
            .unwrap();
        let result = identification
            .commit(IdentificationCommit {
                lease: &lease,
                attempt_id: attempt.id,
                evidence: &[],
                candidates: &[],
                decision: &decision(DecisionLevel::Unidentified, DecisionReason::NoCandidate),
                title_hint: Some("page"),
                now_us: 110_000_000 + i64::from(index),
            })
            .await
            .unwrap();
        original.insert(result.review_case_id.unwrap());
    }
    let filter = ReviewCaseFilter {
        level: Some(DecisionLevel::Unidentified),
        inbox_directory_id: Some(inbox),
        updated_before_us: Some(120_000_000),
    };
    let first = reviews
        .list_active(account, &filter, &PageRequest::new(None, Some(2)).unwrap())
        .await
        .unwrap();
    let cursor = first.next_cursor.clone().unwrap();
    sqlx::query("UPDATE identification_review_cases SET updated_at_us=200000000 WHERE id=?")
        .bind(first.items[0].id.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();

    let second = reviews
        .list_active(
            account,
            &filter,
            &PageRequest::new(Some(cursor), Some(2)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .chain(&second.items)
            .map(|case| case.id)
            .collect::<BTreeSet<_>>(),
        original
    );
    assert!(second.next_cursor.is_none());
}

fn decision(level: DecisionLevel, reason: DecisionReason) -> IdentificationDecisionDraft {
    IdentificationDecisionDraft {
        level,
        selected_candidate: None,
        reasons: vec![reason],
        retry_at_us: (level == DecisionLevel::Blocked).then_some(200_000_000),
        graph: None,
        rule_version: 1,
    }
}
