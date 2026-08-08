mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::identification::decision::{
    DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use mediaflow_core::identification::manual::model::{
    ManualDecisionInput, ManualDecisionKind, ManualIdentityHint, TaskDecisionState,
};
use mediaflow_core::identification::manual::store::ManualDecisionStore;
use mediaflow_core::identification::model::MediaKind;
use mediaflow_core::identification::review::ReviewCaseStore;
use mediaflow_core::identification::store::{IdentificationCommit, IdentificationStore};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxReader;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::tasks::events::TaskEventEnvelope;
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
async fn accepts_once_replays_exactly_and_commits_case_dispatch_feedback_and_event() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (task_id, case_id) = seed_case(db.pool(), account, b"movies/Dune (2021).mkv", "dune").await;
    let store = ManualDecisionStore::new(db.pool().clone());
    let input = provider_decision("438631", true);

    let accepted = store
        .accept(account, case_id, 1, "decision-key", &input, 120_000_000)
        .await
        .unwrap();
    let replayed = store
        .accept(account, case_id, 1, "decision-key", &input, 121_000_000)
        .await
        .unwrap();

    assert_eq!(accepted, replayed);
    assert_eq!(accepted.task_id, task_id);
    assert_eq!(accepted.case_id, case_id);
    assert_eq!(accepted.case_version, 2);
    assert_eq!(accepted.kind, ManualDecisionKind::SelectProviderCandidate);
    assert_eq!(accepted.state, TaskDecisionState::Accepted);
    assert_eq!(count(db.pool(), "identification_task_decisions").await, 1);
    assert_eq!(
        count(db.pool(), "identification_decision_dispatches").await,
        1
    );
    assert_eq!(count(db.pool(), "identification_feedback").await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM platform_outbox_events WHERE event_type='task-decision.accepted'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    let event: String = sqlx::query_scalar(
        "SELECT payload_json FROM platform_outbox_events WHERE event_type='task-decision.accepted'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&event).unwrap(),
        json!({
            "case_id": case_id,
            "decision_id": accepted.id,
            "kind": "select-provider-candidate",
            "case_version": 2
        })
    );
    let events = OutboxReader::new(db.pool().clone())
        .after(0, 200)
        .await
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        TaskEventEnvelope::TaskDecisionAccepted { task_id: event_task_id, payload, .. }
            if *event_task_id == task_id
                && payload.case_id == case_id
                && payload.decision_id == accepted.id
                && payload.case_version == 2
    )));

    let view = ReviewCaseStore::new(db.pool().clone())
        .get_active(account, case_id)
        .await
        .unwrap();
    assert_eq!(view.version, 2);
    assert!(view.allowed_actions.is_empty());
    let latest = view.latest_task_decision.unwrap();
    assert_eq!(latest.id, accepted.id);
    assert_eq!(latest.kind, ManualDecisionKind::SelectProviderCandidate);
    assert_eq!(latest.state, TaskDecisionState::Accepted);

    let immutable =
        sqlx::query("UPDATE identification_task_decisions SET payload_json='{}' WHERE id=?")
            .bind(accepted.id.as_bytes().as_slice())
            .execute(db.pool())
            .await;
    assert!(immutable.is_err(), "TaskDecision rows must be immutable");
}

#[tokio::test]
async fn rejects_same_key_with_different_body_and_keeps_the_first_fact() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, case_id) = seed_case(db.pool(), account, b"movies/conflict.mkv", "conflict").await;
    let store = ManualDecisionStore::new(db.pool().clone());

    store
        .accept(
            account,
            case_id,
            1,
            "same-key",
            &provider_decision("10", false),
            120_000_000,
        )
        .await
        .unwrap();
    let error = store
        .accept(
            account,
            case_id,
            1,
            "same-key",
            &provider_decision("11", false),
            121_000_000,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::RequestConflict);
    assert_eq!(count(db.pool(), "identification_task_decisions").await, 1);
}

#[tokio::test]
async fn concurrent_case_version_has_one_winner_and_no_ghost_side_effects() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, case_id) = seed_case(db.pool(), account, b"movies/concurrent.mkv", "concurrent").await;
    let store = ManualDecisionStore::new(db.pool().clone());
    let left_store = store.clone();
    let right_store = store.clone();
    let first = provider_decision("20", false);
    let second = ManualDecisionInput::SelectGenericVideo {
        display_title: "Family archive".to_owned(),
        group_hint: Some("2026".to_owned()),
        save_grouping_feedback: false,
    };

    let (left, right) = tokio::join!(
        left_store.accept(account, case_id, 1, "left", &first, 120_000_000),
        right_store.accept(account, case_id, 1, "right", &second, 120_000_001),
    );

    assert_eq!(
        [left.is_ok(), right.is_ok()]
            .into_iter()
            .filter(|ok| *ok)
            .count(),
        1
    );
    let loser = left.err().or_else(|| right.err()).unwrap();
    assert_eq!(loser.code(), ErrorCode::ConfigVersionConflict);
    assert_eq!(count(db.pool(), "identification_task_decisions").await, 1);
    assert_eq!(
        count(db.pool(), "identification_decision_dispatches").await,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT version FROM identification_review_cases WHERE id=?")
            .bind(case_id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        2
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn validates_versions_active_ownership_keys_and_bounded_payloads_without_writes() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, case_id) = seed_case(db.pool(), account, b"shows/bounds.mkv", "bounds").await;
    let store = ManualDecisionStore::new(db.pool().clone());

    for (key, input) in [
        ("", provider_decision("1", false)),
        (&"k".repeat(129), provider_decision("1", false)),
        ("provider", provider_decision(&"1".repeat(65), false)),
    ] {
        let error = store
            .accept(account, case_id, 1, key, &input, 120_000_000)
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ValidationFailed);
    }
    let bad_hint = ManualDecisionInput::RematchWithHints {
        hint: ManualIdentityHint {
            media_kind: MediaKind::Movie,
            normalized_title: "title".to_owned(),
            year: Some(2026),
            season: Some(1),
            episodes: vec![1, 1],
        },
        save_feedback: false,
    };
    assert_eq!(
        store
            .accept(account, case_id, 1, "bad-hint", &bad_hint, 120_000_000)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ValidationFailed
    );
    let oversized = ManualDecisionInput::SelectGenericVideo {
        display_title: "界".repeat(201),
        group_hint: None,
        save_grouping_feedback: false,
    };
    assert_eq!(
        store
            .accept(account, case_id, 1, "oversized", &oversized, 120_000_000)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ValidationFailed
    );
    assert_eq!(
        store
            .accept(
                account,
                case_id,
                0,
                "stale",
                &provider_decision("1", false),
                120_000_000,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ConfigVersionConflict
    );
    assert_eq!(
        store
            .accept(
                Uuid::now_v7(),
                case_id,
                1,
                "other-account",
                &provider_decision("1", false),
                120_000_000,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::NotFound
    );
    sqlx::query(
        "UPDATE identification_review_cases SET status='closed',closed_at_us=?,updated_at_us=? WHERE id=?",
    )
    .bind(121_000_000_i64)
    .bind(121_000_000_i64)
    .bind(case_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    assert_eq!(
        store
            .accept(
                account,
                case_id,
                1,
                "inactive",
                &provider_decision("1", false),
                122_000_000,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::NotFound
    );
    assert_eq!(count(db.pool(), "identification_task_decisions").await, 0);
}

#[tokio::test]
async fn exact_feedback_is_opt_in_and_uses_typed_selector_columns() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, first_case) = seed_case(db.pool(), account, b"movies/Once.mkv", "once").await;
    let (_, second_case) = seed_case(db.pool(), account, b"movies/Exact.mkv", "exact title").await;
    let store = ManualDecisionStore::new(db.pool().clone());

    store
        .accept(
            account,
            first_case,
            1,
            "one-shot",
            &provider_decision("30", false),
            120_000_000,
        )
        .await
        .unwrap();
    assert_eq!(count(db.pool(), "identification_feedback").await, 0);

    let accepted = store
        .accept(
            account,
            second_case,
            1,
            "save-exact",
            &provider_decision("31", true),
            121_000_000,
        )
        .await
        .unwrap();
    let selector = sqlx::query_as::<
        _,
        (
            i64,
            String,
            String,
            Option<i64>,
            Option<i64>,
            String,
            String,
            String,
            i64,
        ),
    >(
        "SELECT selector_version,media_type,normalized_title,year,season,episodes_json,
                provider,provider_id,enabled
         FROM identification_feedback WHERE source_decision_id=?",
    )
    .bind(accepted.id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        selector,
        (
            1,
            "movie".to_owned(),
            "exact title".to_owned(),
            None,
            None,
            "[]".to_owned(),
            "tmdb".to_owned(),
            "31".to_owned(),
            1,
        )
    );
}

fn provider_decision(provider_id: &str, save_feedback: bool) -> ManualDecisionInput {
    ManualDecisionInput::SelectProviderCandidate {
        media_kind: MediaKind::Movie,
        provider_id: provider_id.to_owned(),
        save_feedback,
    }
}

async fn seed_case(
    pool: &sqlx::SqlitePool,
    _account: Uuid,
    path: &[u8],
    title_hint: &str,
) -> (Uuid, Uuid) {
    let inbox = match sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT id FROM discovery_inbox_directories ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .unwrap()
    {
        Some(bytes) => Uuid::from_slice(&bytes).unwrap(),
        None => common::seed_inbox(pool).await,
    };
    let lease = common::seed_processing_lease(pool, inbox, path, vec![path[0]], title_hint).await;
    let store = IdentificationStore::new(pool.clone());
    let attempt = store
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    let committed = store
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &[],
            candidates: &[],
            decision: &IdentificationDecisionDraft {
                level: DecisionLevel::Unidentified,
                selected_candidate: None,
                reasons: vec![DecisionReason::NoCandidate],
                retry_at_us: None,
                graph: None,
                rule_version: 1,
            },
            title_hint: Some(title_hint),
            now_us: 94_000_000,
        })
        .await
        .unwrap();
    (lease.task.id, committed.review_case_id.unwrap())
}

async fn count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}
