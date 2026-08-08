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
use mediaflow_core::identification::decision::DecisionLevel;
use mediaflow_core::identification::parser::FilenameParser;
use mediaflow_core::identification::service::IdentificationService;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::secrets::SecretBytes;

#[tokio::test]
async fn service_uses_external_id_before_search_and_persists_only_bounded_provider_projection() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/dune-[imdb-tt1160419].mkv",
        vec![1],
        "service-a",
    )
    .await;
    let hint = FilenameParser::default()
        .parse("movies/dune.2021.[imdb-tt1160419].mkv")
        .unwrap();
    let provider = FakeProvider::successful();

    let outcome = IdentificationService::new(db.pool().clone())
        .identify(
            &lease,
            &hint,
            &[],
            &provider,
            &SecretBytes::new(b"never-persist-this-token".to_vec()),
            "zh-CN",
            Some("CN"),
            93_000_000,
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Confirmed));
    assert_eq!(outcome.provider_failure, None);
    assert_eq!(provider.find_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.search_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT source FROM identification_evidence LIMIT 1")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "tmdb"
    );
    let database_bytes = std::fs::read(fixture.database_path()).unwrap();
    assert!(
        !database_bytes
            .windows(b"never-persist-this-token".len())
            .any(|window| window == b"never-persist-this-token")
    );
}

#[tokio::test]
async fn service_converts_provider_dependency_failure_to_blocked_without_review_case() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/unavailable.mkv",
        vec![2],
        "service-b",
    )
    .await;
    let hint = FilenameParser::default()
        .parse("movies/unavailable.2026.mkv")
        .unwrap();
    let provider = FakeProvider::failing();

    let outcome = IdentificationService::new(db.pool().clone())
        .identify(
            &lease,
            &hint,
            &[],
            &provider,
            &SecretBytes::new(b"another-secret-token".to_vec()),
            "zh-CN",
            None,
            93_000_000,
        )
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Blocked));
    assert_eq!(
        outcome.provider_failure,
        Some(ProviderError::TemporarilyUnavailable {
            retry_at_us: 153_000_000,
        })
    );
    assert!(outcome.review_case_id.is_none());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_review_cases")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT status FROM tasks_processing_tasks WHERE id=?")
            .bind(lease.task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "paused"
    );
}

#[tokio::test]
async fn service_persists_not_configured_as_a_recoverable_blocked_decision() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/not-configured.mkv",
        vec![3],
        "service-c",
    )
    .await;
    let hint = FilenameParser::default()
        .parse("movies/not-configured.2026.mkv")
        .unwrap();

    let outcome = IdentificationService::new(db.pool().clone())
        .identify_provider_failure(&lease, &hint, &[], ProviderError::NotConfigured, 93_000_000)
        .await
        .unwrap();

    assert_eq!(outcome.level, Some(DecisionLevel::Blocked));
    assert_eq!(outcome.provider_failure, Some(ProviderError::NotConfigured));
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT reason FROM tasks_processing_tasks WHERE id=?")
            .bind(lease.task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "identification.provider-unavailable"
    );
}

struct FakeProvider {
    fail: bool,
    find_calls: AtomicUsize,
    search_calls: AtomicUsize,
}

impl FakeProvider {
    fn successful() -> Self {
        Self {
            fail: false,
            find_calls: AtomicUsize::new(0),
            search_calls: AtomicUsize::new(0),
        }
    }

    fn failing() -> Self {
        Self {
            fail: true,
            find_calls: AtomicUsize::new(0),
            search_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl MetadataProvider for FakeProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.find_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![candidate()])
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.search_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(ProviderError::TemporarilyUnavailable {
                retry_at_us: 153_000_000,
            })
        } else {
            Ok(vec![candidate()])
        }
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Ok(candidate())
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

fn candidate() -> MetadataCandidate {
    MetadataCandidate {
        identity: CandidateIdentity {
            provider_id: 438_631,
            media_kind: ProviderMediaKind::Movie,
        },
        titles: vec![LocalizedField {
            value: "dune".to_owned(),
            language: "en-US".to_owned(),
            source: FieldLanguageSource::English,
        }],
        summaries: Vec::new(),
        release_dates: vec![LocalizedField {
            value: NaiveDate::from_ymd_opt(2021, 10, 22).unwrap(),
            language: "en-US".to_owned(),
            source: FieldLanguageSource::English,
        }],
        aliases: Vec::new(),
        episodes: Vec::new(),
        provider_version: 1,
        cache_status: CacheStatus::Miss,
        retrieved_at_us: 90_000_000,
    }
}
