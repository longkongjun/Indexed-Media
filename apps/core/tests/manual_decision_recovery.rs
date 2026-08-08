mod common;

use std::sync::Arc;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::identification::decision::{
    DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use mediaflow_core::identification::manual::model::{ManualDecisionInput, ManualIdentityHint};
use mediaflow_core::identification::manual::service::DecisionCoordinator;
use mediaflow_core::identification::manual::store::ManualDecisionStore;
use mediaflow_core::identification::model::MediaKind;
use mediaflow_core::identification::review::ReviewCaseStore;
use mediaflow_core::identification::store::{IdentificationCommit, IdentificationStore};
use mediaflow_core::platform::audit;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::task_runtime::{ManualTaskClock, ProcessingTaskRuntime};
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::tasks::processing::model::{
    DecisionCheckpoint, DecisionDispatch, ProcessingCheckpoint, ProcessingStage, ProcessingStatus,
};
use mediaflow_core::tasks::processing::service::ProcessingTaskService;
use uuid::Uuid;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn candidate_and_rematch_dispatch_create_one_idempotent_identification_attempt_each() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, candidate_case) = seed_case(db.pool(), b"movies/candidate.mkv", "candidate").await;
    let (_, rematch_case) = seed_case(db.pool(), b"shows/rematch.mkv", "rematch").await;
    let decisions = ManualDecisionStore::new(db.pool().clone());
    let candidate = decisions
        .accept(
            account,
            candidate_case,
            1,
            "candidate",
            &ManualDecisionInput::SelectProviderCandidate {
                media_kind: MediaKind::Movie,
                provider_id: "438631".to_owned(),
                save_feedback: false,
            },
            120_000_000,
        )
        .await
        .unwrap();
    let rematch = decisions
        .accept(
            account,
            rematch_case,
            1,
            "rematch",
            &ManualDecisionInput::RematchWithHints {
                hint: ManualIdentityHint {
                    media_kind: MediaKind::Episode,
                    normalized_title: "rematch".to_owned(),
                    year: Some(2026),
                    season: Some(1),
                    episodes: vec![1, 2],
                },
                save_feedback: false,
            },
            120_000_001,
        )
        .await
        .unwrap();
    let coordinator = DecisionCoordinator::new(db.pool().clone(), OutboxNotifier::new());

    assert_eq!(
        coordinator.dispatch_once(100, 121_000_000).await.unwrap(),
        2
    );
    assert_eq!(
        coordinator.dispatch_once(100, 121_000_001).await.unwrap(),
        0
    );

    let service = ProcessingTaskService::new(db.pool().clone());
    for (receipt, expected_checkpoint) in [
        (candidate, DecisionCheckpoint::ManualDecisionPending),
        (rematch, DecisionCheckpoint::RematchPending),
    ] {
        let task = service.get(account, receipt.task_id).await.unwrap();
        assert_eq!(task.status, ProcessingStatus::Queued);
        assert_eq!(task.stage, ProcessingStage::Identification);
        assert_eq!(task.checkpoint, ProcessingCheckpoint::Pending);
        assert_eq!(task.current_task_decision_id, Some(receipt.id));
        assert_eq!(task.decision_checkpoint, Some(expected_checkpoint));
        assert_eq!(task.attempt_count, 2);
        assert_eq!(
            active_attempts(db.pool(), receipt.task_id, "identification").await,
            1
        );
        assert!(
            ReviewCaseStore::new(db.pool().clone())
                .get_active(account, receipt.case_id)
                .await
                .unwrap()
                .allowed_actions
                .is_empty()
        );
        let dispatch = DecisionDispatch::Reidentify {
            task_id: receipt.task_id,
            decision_id: receipt.id,
            checkpoint: expected_checkpoint,
        };
        let replay = service
            .apply_manual_decision(account, &dispatch, 121_000_002)
            .await
            .unwrap();
        assert_eq!(replay, task);
        assert_eq!(
            active_attempts(db.pool(), receipt.task_id, "identification").await,
            1
        );
    }
    assert_eq!(
        count(db.pool(), "tasks_processing_manual_decision_receipts").await,
        2
    );
    assert_eq!(manual_audits(db.pool()).await, 2);
    assert_eq!(
        decisions
            .accept(
                account,
                candidate_case,
                2,
                "candidate-too-soon",
                &ManualDecisionInput::SelectGenericVideo {
                    display_title: "must wait".to_owned(),
                    group_hint: None,
                    save_grouping_feedback: false,
                },
                122_000_000,
            )
            .await
            .unwrap_err()
            .code(),
        ErrorCode::TaskInvalidState
    );
    assert_eq!(count(db.pool(), "identification_task_decisions").await, 2);
}

#[tokio::test]
async fn generic_video_queues_planning_without_another_identification_attempt() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, case_id) = seed_case(db.pool(), b"courses/lesson.mkv", "lesson").await;
    let decisions = ManualDecisionStore::new(db.pool().clone());
    let accepted = decisions
        .accept(
            account,
            case_id,
            1,
            "generic",
            &ManualDecisionInput::SelectGenericVideo {
                display_title: "Lesson 1".to_owned(),
                group_hint: Some("Course".to_owned()),
                save_grouping_feedback: false,
            },
            120_000_000,
        )
        .await
        .unwrap();
    let before = attempt_count(db.pool(), accepted.task_id, "identification").await;

    assert_eq!(
        DecisionCoordinator::new(db.pool().clone(), OutboxNotifier::new())
            .dispatch_once(100, 121_000_000)
            .await
            .unwrap(),
        1
    );

    let task = ProcessingTaskService::new(db.pool().clone())
        .get(account, accepted.task_id)
        .await
        .unwrap();
    assert_eq!(task.status, ProcessingStatus::Queued);
    assert_eq!(task.stage, ProcessingStage::Planning);
    assert_eq!(
        task.checkpoint,
        ProcessingCheckpoint::IdentificationComplete
    );
    assert_eq!(task.current_task_decision_id, Some(accepted.id));
    assert_eq!(
        task.decision_checkpoint,
        Some(DecisionCheckpoint::PlanningRequested)
    );
    assert_eq!(
        attempt_count(db.pool(), accepted.task_id, "identification").await,
        before
    );
    assert_eq!(
        active_attempts(db.pool(), accepted.task_id, "planning").await,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT checkpoint FROM tasks_processing_manual_decision_receipts WHERE decision_id=?",
        )
        .bind(accepted.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "generic-video-selected"
    );
}

#[tokio::test]
async fn a_new_coordinator_recovers_an_accepted_but_unapplied_decision() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, case_id) = seed_case(db.pool(), b"movies/restart.mkv", "restart").await;
    let decisions = ManualDecisionStore::new(db.pool().clone());
    let accepted = decisions
        .accept(
            account,
            case_id,
            1,
            "restart",
            &ManualDecisionInput::SelectProviderCandidate {
                media_kind: MediaKind::Movie,
                provider_id: "50".to_owned(),
                save_feedback: false,
            },
            120_000_000,
        )
        .await
        .unwrap();
    drop(decisions);

    let restarted = DecisionCoordinator::new(db.pool().clone(), OutboxNotifier::new());
    assert_eq!(restarted.dispatch_once(100, 130_000_000).await.unwrap(), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM identification_decision_dispatches WHERE decision_id=?",
        )
        .bind(accepted.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "applied"
    );
    assert_eq!(
        active_attempts(db.pool(), accepted.task_id, "identification").await,
        1
    );
}

#[tokio::test]
async fn task_and_audit_receipts_make_a_crash_before_mark_applied_safe_to_replay() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, case_id) = seed_case(db.pool(), b"movies/crash.mkv", "crash").await;
    let decisions = ManualDecisionStore::new(db.pool().clone());
    let accepted = decisions
        .accept(
            account,
            case_id,
            1,
            "crash",
            &ManualDecisionInput::SelectProviderCandidate {
                media_kind: MediaKind::Movie,
                provider_id: "60".to_owned(),
                save_feedback: false,
            },
            120_000_000,
        )
        .await
        .unwrap();
    let pending = decisions.pending_dispatches(100).await.unwrap();
    assert_eq!(pending.len(), 1);
    let dispatch = pending[0].dispatch.clone();
    ProcessingTaskService::new(db.pool().clone())
        .apply_manual_decision(account, &dispatch, 121_000_000)
        .await
        .unwrap();
    audit::record_manual_decision_once(db.pool(), accepted.id, 121_000_001)
        .await
        .unwrap();
    // 模拟崩溃：任务和审计提交后，识别派发仍保持已接受状态。
    drop(decisions);

    assert_eq!(
        DecisionCoordinator::new(db.pool().clone(), OutboxNotifier::new())
            .dispatch_once(100, 130_000_000)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        active_attempts(db.pool(), accepted.task_id, "identification").await,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM tasks_processing_manual_decision_receipts WHERE decision_id=?",
        )
        .bind(accepted.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
    assert_eq!(manual_audits(db.pool()).await, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM platform_event_consumer_receipts
             WHERE consumer='audit' AND decision_id=?",
        )
        .bind(accepted.id.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn processing_runtime_reconciles_during_prepare_and_its_bounded_poll_loop() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let (_, first_case) = seed_case(db.pool(), b"movies/prepare.mkv", "prepare").await;
    let (_, second_case) = seed_case(db.pool(), b"movies/poll.mkv", "poll").await;
    let decisions = ManualDecisionStore::new(db.pool().clone());
    let first = decisions
        .accept(
            account,
            first_case,
            1,
            "prepare",
            &ManualDecisionInput::SelectProviderCandidate {
                media_kind: MediaKind::Movie,
                provider_id: "70".to_owned(),
                save_feedback: false,
            },
            120_000_000,
        )
        .await
        .unwrap();
    let runtime = ProcessingTaskRuntime::new_with_concurrency(
        db.pool().clone(),
        OutboxNotifier::new(),
        &[],
        Arc::new(ManualTaskClock::new(130_000_000)),
        1,
    )
    .unwrap();

    runtime.prepare().await.unwrap();
    assert_eq!(dispatch_state(db.pool(), first.id).await, "applied");
    let second = decisions
        .accept(
            account,
            second_case,
            1,
            "poll",
            &ManualDecisionInput::SelectProviderCandidate {
                media_kind: MediaKind::Movie,
                provider_id: "71".to_owned(),
                save_feedback: false,
            },
            120_000_001,
        )
        .await
        .unwrap();
    let handle = runtime.start();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while dispatch_state(db.pool(), second.id).await != "applied" {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    handle.abort();
    let _ = handle.await;
    assert_eq!(
        active_attempts(db.pool(), second.task_id, "identification").await,
        1
    );
}

async fn seed_case(pool: &sqlx::SqlitePool, path: &[u8], title_hint: &str) -> (Uuid, Uuid) {
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
    let lease = common::seed_processing_lease(pool, inbox, path, path.to_vec(), title_hint).await;
    let identification = IdentificationStore::new(pool.clone());
    let attempt = identification
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
        .await
        .unwrap();
    let committed = identification
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

async fn active_attempts(pool: &sqlx::SqlitePool, task_id: Uuid, stage: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks_processing_attempts
         WHERE task_id=? AND stage=? AND status IN ('queued','running')",
    )
    .bind(task_id.as_bytes().as_slice())
    .bind(stage)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn attempt_count(pool: &sqlx::SqlitePool, task_id: Uuid, stage: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM tasks_processing_attempts WHERE task_id=? AND stage=?")
        .bind(task_id.as_bytes().as_slice())
        .bind(stage)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn manual_audits(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM platform_audit_events
         WHERE action='identification.manual-decision.accepted'",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn dispatch_state(pool: &sqlx::SqlitePool, decision_id: Uuid) -> String {
    sqlx::query_scalar("SELECT state FROM identification_decision_dispatches WHERE decision_id=?")
        .bind(decision_id.as_bytes().as_slice())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}
