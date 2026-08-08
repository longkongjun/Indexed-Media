mod common;

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::NaiveDate;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{
    CacheStatus, CandidateIdentity, EpisodeIdentity, FieldLanguageSource, LocalizedField,
    MetadataCandidate, ProviderError, ProviderMediaKind, TmdbExternalIdRequest, TmdbSearchRequest,
};
use mediaflow_core::connectors::service::MetadataProvider;
use mediaflow_core::discovery::observations::FileObservation;
use mediaflow_core::discovery::revisions::{ObservationSource, RevisionObserver, RevisionService};
use mediaflow_core::identification::decision::{
    DecisionLevel, DecisionReason, IdentificationDecisionDraft,
};
use mediaflow_core::identification::evidence::{
    EvidenceDraft, EvidenceKind, EvidenceSource, EvidenceStrength,
};
use mediaflow_core::identification::manual::model::{
    ManualDecisionInput, ManualIdentificationContext,
};
use mediaflow_core::identification::manual::service::DecisionCoordinator;
use mediaflow_core::identification::manual::store::ManualDecisionStore;
use mediaflow_core::identification::model::{MediaKind, ParsedIdentityHint};
use mediaflow_core::identification::parser::FilenameParser;
use mediaflow_core::identification::service::IdentificationService;
use mediaflow_core::identification::store::{
    IdentificationCommit, IdentificationCommitStatus, IdentificationStore,
};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::secrets::SecretBytes;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::tasks::processing::worker::ProcessingStopToken;
use mediaflow_core::tasks::processing::{model::ProcessingStage, store::ProcessingStore};
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;
use uuid::Uuid;

#[tokio::test]
async fn selected_provider_identity_is_refetched_and_explicit_title_override_can_confirm() {
    let (fixture, lease, hint) = harness(b"movies/local-title.2026.mkv", vec![1]).await;
    let provider = ManualProvider::with_detail(candidate(
        438_631,
        ProviderMediaKind::Movie,
        "different provider title",
        Some(2025),
        &[],
    ));
    let context = ManualIdentificationContext::SelectedProvider {
        decision_id: Uuid::now_v7(),
        media_kind: MediaKind::Movie,
        provider_id: "438631".to_owned(),
    };

    let outcome = IdentificationService::new(fixture.pool().clone())
        .identify_with_manual_context(
            &lease,
            &hint,
            &[],
            &context,
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(95_000_000),
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Confirmed));
    assert_eq!(provider.detail_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.search_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT source FROM identification_evidence WHERE source='manual-decision'"
        )
        .fetch_one(fixture.pool())
        .await
        .unwrap(),
        "manual-decision"
    );
}

#[tokio::test]
async fn selected_identity_rejects_media_type_and_missing_episode_structure() {
    for (path, kind, provider_candidate) in [
        (
            b"movies/type-conflict.2026.mkv".as_slice(),
            MediaKind::Movie,
            candidate(12, ProviderMediaKind::Tv, "type conflict", Some(2026), &[]),
        ),
        (
            b"shows/series.s01e02.mkv".as_slice(),
            MediaKind::Episode,
            candidate(13, ProviderMediaKind::Tv, "series", None, &[]),
        ),
    ] {
        let (fixture, lease, mut hint) = harness(path, vec![path[0]]).await;
        hint.media_kind = kind;
        if kind == MediaKind::Episode {
            hint.season = Some(1);
            hint.episodes = vec![2];
        }
        let provider = ManualProvider::with_detail(provider_candidate);
        let context = ManualIdentificationContext::SelectedProvider {
            decision_id: Uuid::now_v7(),
            media_kind: kind,
            provider_id: if kind == MediaKind::Movie { "12" } else { "13" }.to_owned(),
        };

        let outcome = IdentificationService::new(fixture.pool().clone())
            .identify_with_manual_context(
                &lease,
                &hint,
                &[],
                &context,
                &provider,
                &token(),
                "zh-CN",
                None,
                &ProcessingStopToken::default(),
                &ManualTaskClock::new(95_000_000),
            )
            .await
            .unwrap();

        assert_eq!(outcome.level, Some(DecisionLevel::Ambiguous));
        assert!(outcome.review_case_id.is_some());
    }
}

#[tokio::test]
async fn selected_identity_provider_failures_are_recoverable_and_do_not_confirm() {
    let failure = ProviderError::TemporarilyUnavailable {
        retry_at_us: 180_000_000,
    };
    let (fixture, lease, hint) = harness(b"movies/provider-failure.mkv", vec![9]).await;
    let provider = ManualProvider::with_detail_error(failure);
    let context = ManualIdentificationContext::SelectedProvider {
        decision_id: Uuid::now_v7(),
        media_kind: MediaKind::Movie,
        provider_id: "99".to_owned(),
    };

    let outcome = IdentificationService::new(fixture.pool().clone())
        .identify_with_manual_context(
            &lease,
            &hint,
            &[],
            &context,
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(95_000_000),
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Blocked));
    assert_eq!(outcome.provider_failure, Some(failure));
    assert!(outcome.review_case_id.is_none());
}

#[tokio::test]
async fn selected_identity_never_accepts_a_detail_response_for_another_provider_id() {
    let (fixture, lease, hint) = harness(b"movies/wrong-provider-id.mkv", vec![10]).await;
    let provider = ManualProvider::with_detail(candidate(
        100,
        ProviderMediaKind::Movie,
        "wrong provider id",
        None,
        &[],
    ));
    let context = ManualIdentificationContext::SelectedProvider {
        decision_id: Uuid::now_v7(),
        media_kind: MediaKind::Movie,
        provider_id: "99".to_owned(),
    };

    let outcome = IdentificationService::new(fixture.pool().clone())
        .identify_with_manual_context(
            &lease,
            &hint,
            &[],
            &context,
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(95_000_000),
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Blocked));
    assert_eq!(
        outcome.provider_failure,
        Some(ProviderError::InvalidResponse)
    );
}

#[tokio::test]
async fn rematch_runs_strong_rules_again_and_can_remain_ambiguous() {
    let (fixture, lease, hint) = harness(b"movies/rematch.2026.mkv", vec![4]).await;
    let provider = ManualProvider::with_search(vec![
        candidate(21, ProviderMediaKind::Movie, "rematch", Some(2026), &[]),
        candidate(22, ProviderMediaKind::Movie, "rematch", Some(2026), &[]),
    ]);
    let context = ManualIdentificationContext::Rematch {
        decision_id: Uuid::now_v7(),
        hint: mediaflow_core::identification::manual::model::ManualIdentityHint {
            media_kind: MediaKind::Movie,
            normalized_title: "rematch".to_owned(),
            year: Some(2026),
            season: None,
            episodes: Vec::new(),
        },
    };

    let outcome = IdentificationService::new(fixture.pool().clone())
        .identify_with_manual_context(
            &lease,
            &hint,
            &[],
            &context,
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(95_000_000),
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Ambiguous));
    assert_eq!(provider.search_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn exact_feedback_matches_the_full_selector_and_never_a_near_title() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/exact.2026.mkv",
        vec![7],
        "feedback-seed",
    )
    .await;
    let identification = IdentificationStore::new(db.pool().clone());
    let attempt = identification
        .begin_attempt(&lease, "filename-v1", "tmdb-v1", 1, 93_000_000)
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
    let evidence = [title_evidence("exact"), year_evidence(2026)];
    let committed = identification
        .commit(IdentificationCommit {
            lease: &lease,
            attempt_id: attempt.id,
            evidence: &evidence,
            candidates: &[],
            decision: &decision,
            title_hint: Some("exact"),
            now_us: 94_000_000,
        })
        .await
        .unwrap();
    let store = ManualDecisionStore::new(db.pool().clone());
    let receipt = store
        .accept(
            account,
            committed.review_case_id.unwrap(),
            1,
            "feedback-exact",
            &ManualDecisionInput::SelectProviderCandidate {
                media_kind: MediaKind::Movie,
                provider_id: "31".to_owned(),
                save_feedback: true,
            },
            95_000_000,
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .decision_context_for_task(lease.task.id, receipt.id)
            .await
            .unwrap(),
        Some(ManualIdentificationContext::SelectedProvider { provider_id, .. }) if provider_id == "31"
    ));

    let exact = FilenameParser::default()
        .parse("movies/exact.2026.mkv")
        .unwrap();
    let near = FilenameParser::default()
        .parse("movies/exact-sequel.2026.mkv")
        .unwrap();
    let exact_context = store.exact_feedback_context(account, &exact).await.unwrap();
    assert!(matches!(
        &exact_context,
        Some(ManualIdentificationContext::ExactFeedback { provider_id, .. }) if provider_id == "31"
    ));
    assert_eq!(
        store.exact_feedback_context(account, &near).await.unwrap(),
        None
    );

    let second = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"incoming/exact.2026.mkv",
        vec![8],
        "feedback-apply",
    )
    .await;
    let provider = ManualProvider::with_detail(candidate(
        31,
        ProviderMediaKind::Movie,
        "exact",
        Some(2026),
        &[],
    ));
    let outcome = IdentificationService::new(db.pool().clone())
        .identify_with_manual_context(
            &second,
            &exact,
            &[],
            &exact_context.unwrap(),
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(97_000_000),
        )
        .await
        .unwrap();
    assert_eq!(outcome.level, Some(DecisionLevel::Confirmed));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM identification_evidence WHERE source='manual-feedback'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn revision_change_during_selected_detail_lookup_prevents_manual_commit() {
    let path = b"movies/revision-race.mkv";
    let (fixture, lease, hint) = harness(path, vec![11]).await;
    let provider = RevisionChangingProvider {
        pool: fixture.pool().clone(),
        inbox: lease.task.inbox_directory_id,
        path: path.to_vec(),
        detail: candidate(41, ProviderMediaKind::Movie, "revision race", None, &[]),
    };
    let context = ManualIdentificationContext::SelectedProvider {
        decision_id: Uuid::now_v7(),
        media_kind: MediaKind::Movie,
        provider_id: "41".to_owned(),
    };

    let outcome = IdentificationService::new(fixture.pool().clone())
        .identify_with_manual_context(
            &lease,
            &hint,
            &[],
            &context,
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(96_000_000),
        )
        .await
        .unwrap();

    assert_eq!(outcome.status, IdentificationCommitStatus::RevisionChanged);
    assert_eq!(outcome.level, None);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_decisions")
            .fetch_one(fixture.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn uncertain_manual_retry_replaces_the_case_with_a_higher_version() {
    let (db, original_case, retry, hint, context) =
        accepted_manual_retry(b"movies/versioned-case.2026.mkv", "51").await;
    let provider = ManualProvider::with_detail(candidate(
        51,
        ProviderMediaKind::Tv,
        "versioned case",
        Some(2026),
        &[],
    ));

    let outcome = IdentificationService::new(db.pool().clone())
        .identify_with_manual_context(
            &retry,
            &hint,
            &[],
            &context,
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(98_000_000),
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Ambiguous));
    let row = sqlx::query(
        "SELECT id,version FROM identification_review_cases
         WHERE task_id=? AND status='active'",
    )
    .bind(retry.task.id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_ne!(
        Uuid::from_slice(&row.get::<Vec<u8>, _>("id")).unwrap(),
        original_case
    );
    assert_eq!(row.get::<i64, _>("version"), 3);
}

#[tokio::test]
async fn blocked_manual_retry_preserves_the_original_recoverable_case() {
    let (db, original_case, retry, hint, context) =
        accepted_manual_retry(b"movies/preserved-case.2026.mkv", "52").await;
    let provider = ManualProvider::with_detail_error(ProviderError::Timeout);

    let outcome = IdentificationService::new(db.pool().clone())
        .identify_with_manual_context(
            &retry,
            &hint,
            &[],
            &context,
            &provider,
            &token(),
            "zh-CN",
            None,
            &ProcessingStopToken::default(),
            &ManualTaskClock::new(98_000_000),
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Blocked));
    assert_eq!(outcome.review_case_id, Some(original_case));
    let row = sqlx::query(
        "SELECT id,version FROM identification_review_cases
         WHERE task_id=? AND status='active'",
    )
    .bind(retry.task.id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        Uuid::from_slice(&row.get::<Vec<u8>, _>("id")).unwrap(),
        original_case
    );
    assert_eq!(row.get::<i64, _>("version"), 2);
}

async fn accepted_manual_retry(
    path: &[u8],
    provider_id: &str,
) -> (
    mediaflow_core::platform::db::Db,
    Uuid,
    mediaflow_core::tasks::processing::model::ProcessingLease,
    ParsedIdentityHint,
    ManualIdentificationContext,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let initial =
        common::seed_processing_lease(db.pool(), inbox, path, path.to_vec(), "case-seed").await;
    let identification = IdentificationStore::new(db.pool().clone());
    let attempt = identification
        .begin_attempt(&initial, "filename-v1", "tmdb-v1", 1, 93_000_000)
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
    let original_case = identification
        .commit(IdentificationCommit {
            lease: &initial,
            attempt_id: attempt.id,
            evidence: &[],
            candidates: &[],
            decision: &decision,
            title_hint: Some("case seed"),
            now_us: 94_000_000,
        })
        .await
        .unwrap()
        .review_case_id
        .unwrap();
    let decisions = ManualDecisionStore::new(db.pool().clone());
    let receipt = decisions
        .accept(
            account,
            original_case,
            1,
            "manual-retry",
            &ManualDecisionInput::SelectProviderCandidate {
                media_kind: MediaKind::Movie,
                provider_id: provider_id.to_owned(),
                save_feedback: false,
            },
            95_000_000,
        )
        .await
        .unwrap();
    DecisionCoordinator::new(db.pool().clone(), OutboxNotifier::new())
        .dispatch_once(10, 96_000_000)
        .await
        .unwrap();
    let retry = ProcessingStore::new(db.pool().clone())
        .claim_next(
            "manual-retry-worker",
            &[ProcessingStage::Identification],
            97_000_000,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.task.current_task_decision_id, Some(receipt.id));
    let context = decisions
        .decision_context_for_task(retry.task.id, receipt.id)
        .await
        .unwrap()
        .unwrap();
    let hint = FilenameParser::default()
        .parse(&String::from_utf8_lossy(path))
        .unwrap();
    (db, original_case, retry, hint, context)
}

async fn harness(
    path: &[u8],
    identity: Vec<u8>,
) -> (
    mediaflow_core::platform::db::Db,
    mediaflow_core::tasks::processing::model::ProcessingLease,
    ParsedIdentityHint,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(db.pool(), inbox, path, identity, "manual").await;
    let hint = FilenameParser::default()
        .parse(&String::from_utf8_lossy(path))
        .unwrap();
    (db, lease, hint)
}

fn token() -> SecretBytes {
    SecretBytes::new(b"manual-test-token".to_vec())
}

fn title_evidence(value: &str) -> EvidenceDraft {
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

fn year_evidence(value: u16) -> EvidenceDraft {
    EvidenceDraft {
        source: EvidenceSource::Filename,
        source_version: "filename-v1".to_owned(),
        kind: EvidenceKind::Year,
        normalized_value: value.to_string(),
        strength: EvidenceStrength::Strong,
        reason: "filename.year".to_owned(),
        source_hash: Sha256::digest(value.to_string().as_bytes()).into(),
    }
}

struct ManualProvider {
    detail: Result<MetadataCandidate, ProviderError>,
    search: Vec<MetadataCandidate>,
    detail_calls: AtomicUsize,
    search_calls: AtomicUsize,
}

struct RevisionChangingProvider {
    pool: sqlx::SqlitePool,
    inbox: Uuid,
    path: Vec<u8>,
    detail: MetadataCandidate,
}

#[async_trait]
impl MetadataProvider for RevisionChangingProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(Vec::new())
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(Vec::new())
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        RevisionService::new(self.pool.clone())
            .observe(
                FileObservation {
                    inbox_directory_id: self.inbox,
                    relative_path_bytes: self.path.clone(),
                    relative_path_display: String::from_utf8_lossy(&self.path).into_owned(),
                    identity_snapshot: vec![12],
                    size_bytes: 101,
                    modified_at_ns: 1,
                },
                ObservationSource::Watcher,
                95_000_000,
            )
            .await
            .unwrap();
        Ok(self.detail.clone())
    }

    async fn verify_episodes(
        &self,
        _token: &SecretBytes,
        _series_id: i64,
        _episodes: &[(u16, u16)],
        _locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        Ok(Vec::new())
    }
}

impl ManualProvider {
    fn with_detail(detail: MetadataCandidate) -> Self {
        Self {
            detail: Ok(detail),
            search: Vec::new(),
            detail_calls: AtomicUsize::new(0),
            search_calls: AtomicUsize::new(0),
        }
    }

    fn with_detail_error(error: ProviderError) -> Self {
        Self {
            detail: Err(error),
            search: Vec::new(),
            detail_calls: AtomicUsize::new(0),
            search_calls: AtomicUsize::new(0),
        }
    }

    fn with_search(search: Vec<MetadataCandidate>) -> Self {
        Self {
            detail: Err(ProviderError::InvalidResponse),
            search,
            detail_calls: AtomicUsize::new(0),
            search_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl MetadataProvider for ManualProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(Vec::new())
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.search_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.search.clone())
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        self.detail_calls.fetch_add(1, Ordering::SeqCst);
        self.detail.clone()
    }

    async fn verify_episodes(
        &self,
        _token: &SecretBytes,
        _series_id: i64,
        _episodes: &[(u16, u16)],
        _locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        Ok(self
            .detail
            .as_ref()
            .map_or_else(|_| Vec::new(), |candidate| candidate.episodes.clone()))
    }
}

fn candidate(
    provider_id: i64,
    media_kind: ProviderMediaKind,
    title: &str,
    year: Option<u16>,
    episodes: &[EpisodeIdentity],
) -> MetadataCandidate {
    MetadataCandidate {
        identity: CandidateIdentity {
            provider_id,
            media_kind,
        },
        titles: vec![LocalizedField {
            value: title.to_owned(),
            language: "en-US".to_owned(),
            source: FieldLanguageSource::English,
        }],
        summaries: Vec::new(),
        release_dates: year
            .map(|year| LocalizedField {
                value: NaiveDate::from_ymd_opt(i32::from(year), 1, 1).unwrap(),
                language: "en-US".to_owned(),
                source: FieldLanguageSource::English,
            })
            .into_iter()
            .collect(),
        aliases: Vec::new(),
        episodes: episodes.to_vec(),
        provider_version: 1,
        cache_status: CacheStatus::Miss,
        retrieved_at_us: 94_000_000,
    }
}
