#![allow(clippy::too_many_lines)]

mod common;

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::NaiveDate;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::enhancer::model::{
    EnhancementInput, EnhancerConfigInput, EnhancerMediaKind,
};
use mediaflow_core::connectors::enhancer::port::{
    EnhancementHints, EnhancerEndpoint, EnhancerError, EnhancerProbe, IdentificationEnhancer,
};
use mediaflow_core::connectors::enhancer::service::EnhancerService;
use mediaflow_core::connectors::model::{
    CacheStatus, CandidateIdentity, EpisodeIdentity, FieldLanguageSource, LocalizedField,
    MetadataCandidate, ProviderError, ProviderMediaKind, SecretString, TmdbConfigCommand,
    TmdbExternalIdRequest, TmdbSearchRequest,
};
use mediaflow_core::connectors::service::{
    ConnectorService, MetadataProvider, TmdbConnectionTester,
};
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::observations::FileObservation;
use mediaflow_core::discovery::revisions::{
    ObservationSource, RevisionObserver as _, RevisionService,
};
use mediaflow_core::identification::service::IdentificationStageHandler;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::secrets::SecretBytes;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::tasks::processing::model::{ProcessingStage, ProcessingStatus};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::{ProcessingStageHandler, ProcessingStopToken};
use sha2::{Digest as _, Sha256};

type SearchLog = Arc<Mutex<Vec<(String, Option<u16>)>>>;

#[tokio::test]
async fn nonstandard_name_hints_reach_provider_as_probable_without_downstream_side_effects() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"incoming/release-group-x.mkv",
        vec![31],
        "enhanced-worker",
    )
    .await;
    let notifier = OutboxNotifier::new();
    let protocol = Arc::new(ScriptedEnhancer::new(vec![Ok(movie_hints())]));
    let enhancer = enabled_enhancer(db.pool().clone(), notifier.clone(), protocol.clone()).await;
    let searches = Arc::new(Mutex::new(Vec::new()));
    let handler = IdentificationStageHandler::new(
        db.pool().clone(),
        notifier.clone(),
        configured_connectors(&fixture, &db, notifier).await,
        Arc::new(DuneProvider {
            searches: searches.clone(),
        }),
        None,
        Arc::new(ManualTaskClock::new(93_000_000)),
    )
    .with_enhancer(enhancer);

    handler
        .run(&lease, ProcessingStopToken::default())
        .await
        .unwrap();

    let task = ProcessingStore::new(db.pool().clone())
        .get_by_task_id(lease.task.id)
        .await
        .unwrap();
    assert_eq!(task.status, ProcessingStatus::WaitingConfirmation);
    assert_eq!(task.stage, ProcessingStage::Identification);
    assert_eq!(
        searches.lock().unwrap().as_slice(),
        &[("Dune".to_owned(), Some(2021))]
    );
    assert_eq!(protocol.enhance_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT level FROM identification_decisions ORDER BY decided_at_us DESC LIMIT 1"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "probable"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM identification_evidence
             WHERE source='enhancer' AND strength='supporting'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        3
    );
    for table in [
        "catalog_media_items",
        "organization_plans",
        "organization_file_operation_journals",
        "organization_plan_nfo_inputs",
    ] {
        let count = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "table={table}");
    }
}

#[tokio::test]
async fn disabled_timeout_invalid_and_resource_failures_preserve_deterministic_result() {
    let cases = [
        (None, "enhancer.fallback.disabled", 0),
        (Some(EnhancerError::Timeout), "enhancer.fallback.timeout", 1),
        (
            Some(EnhancerError::InvalidResponse),
            "enhancer.fallback.invalid-response",
            1,
        ),
        (
            Some(EnhancerError::Overloaded),
            "enhancer.fallback.overloaded",
            1,
        ),
    ];
    for (failure, expected_reason, expected_calls) in cases {
        let result = run_fallback_case(failure).await;
        assert_eq!(result.status, ProcessingStatus::Queued);
        assert_eq!(result.stage, ProcessingStage::Planning);
        assert_eq!(result.decision_level, "confirmed");
        assert_eq!(result.search, ("dune".to_owned(), Some(2021)));
        assert_eq!(result.fallback_reason, expected_reason);
        assert_eq!(result.enhance_calls, expected_calls);
    }
}

#[tokio::test]
async fn overload_opens_a_bounded_breaker_and_restart_still_falls_back_without_waiting() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let notifier = OutboxNotifier::new();
    let protocol = Arc::new(ScriptedEnhancer::new(vec![
        Err(EnhancerError::Overloaded),
        Ok(movie_hints()),
    ]));
    let enhancer = enabled_enhancer(db.pool().clone(), notifier.clone(), protocol.clone()).await;
    let handler = IdentificationStageHandler::new(
        db.pool().clone(),
        notifier.clone(),
        configured_connectors(&fixture, &db, notifier.clone()).await,
        Arc::new(DuneProvider::default()),
        None,
        Arc::new(ManualTaskClock::new(93_000_000)),
    )
    .with_enhancer(enhancer);

    for (path, identity, owner) in [
        (b"a/dune.2021.mkv".as_slice(), vec![41], "breaker-a"),
        (b"b/dune.2021.mkv".as_slice(), vec![42], "breaker-b"),
    ] {
        let lease = common::seed_processing_lease(db.pool(), inbox, path, identity, owner).await;
        handler
            .run(&lease, ProcessingStopToken::default())
            .await
            .unwrap();
    }
    assert_eq!(protocol.enhance_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM identification_evidence
             WHERE reason='enhancer.fallback.overloaded'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        2
    );

    let restarted_protocol = Arc::new(ScriptedEnhancer::new(vec![Err(EnhancerError::Unavailable)]));
    let restarted = IdentificationStageHandler::new(
        db.pool().clone(),
        notifier.clone(),
        existing_connectors(&fixture, &db, notifier.clone()),
        Arc::new(DuneProvider::default()),
        None,
        Arc::new(ManualTaskClock::new(93_000_000)),
    )
    .with_enhancer(EnhancerService::new(
        db.pool().clone(),
        restarted_protocol.clone(),
        notifier,
    ));
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"restart/dune.2021.mkv",
        vec![43],
        "restart-worker",
    )
    .await;
    restarted
        .run(&lease, ProcessingStopToken::default())
        .await
        .unwrap();
    assert_eq!(restarted_protocol.enhance_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        ProcessingStore::new(db.pool().clone())
            .get_by_task_id(lease.task.id)
            .await
            .unwrap()
            .stage,
        ProcessingStage::Planning
    );
}

#[tokio::test]
async fn nfo_identity_overrides_conflicting_model_hints_without_touching_files() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let inbox_path = fixture.config().config_dir.join("inbox");
    let movies = inbox_path.join("movies");
    std::fs::create_dir_all(&movies).unwrap();
    let media = movies.join("mystery.mkv");
    let nfo = movies.join("mystery.nfo");
    std::fs::write(&media, b"immutable-media").unwrap();
    std::fs::write(
        &nfo,
        b"<movie><title>Dune</title><year>2021</year><uniqueid type=\"tmdb\" default=\"true\">438631</uniqueid></movie>",
    )
    .unwrap();
    let before = (file_hash(&media), file_hash(&nfo));
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming",
            "label":"Incoming",
            "container_path":std::fs::canonicalize(&fixture.config().config_dir).unwrap(),
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = seed_real_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/mystery.mkv",
        vec![44],
        "nfo-priority",
    )
    .await;
    let notifier = OutboxNotifier::new();
    let protocol = Arc::new(ScriptedEnhancer::new(vec![Ok(EnhancementHints {
        title: Some("Wrong Model Title".to_owned()),
        year: Some(1999),
        media_kind: Some(EnhancerMediaKind::Movie),
        season: None,
        episodes: Vec::new(),
    })]));
    let enhancer = enabled_enhancer(db.pool().clone(), notifier.clone(), protocol).await;
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> =
        Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
    let handler = IdentificationStageHandler::new(
        db.pool().clone(),
        notifier.clone(),
        configured_connectors(&fixture, &db, notifier).await,
        Arc::new(DuneProvider::default()),
        Some(fs),
        Arc::new(ManualTaskClock::new(93_000_000)),
    )
    .with_enhancer(enhancer);

    handler
        .run(&lease, ProcessingStopToken::default())
        .await
        .unwrap();

    let task = ProcessingStore::new(db.pool().clone())
        .get_by_task_id(lease.task.id)
        .await
        .unwrap();
    assert_eq!(task.stage, ProcessingStage::Planning);
    assert_eq!(task.status, ProcessingStatus::Queued);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT reason FROM identification_decisions ORDER BY decided_at_us DESC LIMIT 1"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "identification.confirmed-external-id"
    );
    assert_eq!(before, (file_hash(&media), file_hash(&nfo)));
}

#[tokio::test]
async fn revision_change_while_enhancer_is_running_discards_all_hint_side_effects() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/dune.2021.mkv",
        vec![51],
        "revision-worker",
    )
    .await;
    let notifier = OutboxNotifier::new();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let protocol = Arc::new(BlockingEnhancer {
        entered: entered.clone(),
        release: release.clone(),
    });
    let enhancer = EnhancerService::new(db.pool().clone(), protocol, notifier.clone());
    enhancer.replace(1, enhancer_config(true)).await.unwrap();
    let handler = Arc::new(
        IdentificationStageHandler::new(
            db.pool().clone(),
            notifier.clone(),
            configured_connectors(&fixture, &db, notifier).await,
            Arc::new(DuneProvider::default()),
            None,
            Arc::new(ManualTaskClock::new(93_000_000)),
        )
        .with_enhancer(enhancer),
    );
    let running_lease = lease.clone();
    let running = tokio::spawn(async move {
        handler
            .run(&running_lease, ProcessingStopToken::default())
            .await
    });
    entered.notified().await;
    RevisionService::new(db.pool().clone())
        .observe(
            FileObservation {
                inbox_directory_id: inbox,
                relative_path_bytes: b"movies/dune.2021.mkv".to_vec(),
                relative_path_display: "movies/dune.2021.mkv".to_owned(),
                identity_snapshot: vec![52],
                size_bytes: 101,
                modified_at_ns: 1,
            },
            ObservationSource::Watcher,
            94_000_000,
        )
        .await
        .unwrap();
    release.notify_one();
    running.await.unwrap().unwrap();

    let task = ProcessingStore::new(db.pool().clone())
        .get_by_task_id(lease.task.id)
        .await
        .unwrap();
    assert_eq!(task.status, ProcessingStatus::Cancelled);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_evidence")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identification_decisions")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

struct FallbackResult {
    status: ProcessingStatus,
    stage: ProcessingStage,
    decision_level: String,
    search: (String, Option<u16>),
    fallback_reason: String,
    enhance_calls: usize,
}

async fn run_fallback_case(failure: Option<EnhancerError>) -> FallbackResult {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/dune.2021.mkv",
        vec![32],
        "fallback-worker",
    )
    .await;
    let notifier = OutboxNotifier::new();
    let protocol = Arc::new(ScriptedEnhancer::new(
        failure.into_iter().map(Err).collect(),
    ));
    let enhancer = EnhancerService::new(db.pool().clone(), protocol.clone(), notifier.clone());
    if failure.is_some() {
        enhancer.replace(1, enhancer_config(true)).await.unwrap();
    }
    let searches = Arc::new(Mutex::new(Vec::new()));
    let handler = IdentificationStageHandler::new(
        db.pool().clone(),
        notifier.clone(),
        configured_connectors(&fixture, &db, notifier).await,
        Arc::new(DuneProvider {
            searches: searches.clone(),
        }),
        None,
        Arc::new(ManualTaskClock::new(93_000_000)),
    )
    .with_enhancer(enhancer);
    handler
        .run(&lease, ProcessingStopToken::default())
        .await
        .unwrap();
    let task = ProcessingStore::new(db.pool().clone())
        .get_by_task_id(lease.task.id)
        .await
        .unwrap();
    let decision_level = sqlx::query_scalar(
        "SELECT level FROM identification_decisions ORDER BY decided_at_us DESC LIMIT 1",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let search = searches.lock().unwrap()[0].clone();
    let fallback_reason = sqlx::query_scalar(
        "SELECT reason FROM identification_evidence
         WHERE source='enhancer' AND kind='availability'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    FallbackResult {
        status: task.status,
        stage: task.stage,
        decision_level,
        search,
        fallback_reason,
        enhance_calls: protocol.enhance_calls.load(Ordering::SeqCst),
    }
}

async fn enabled_enhancer(
    pool: sqlx::SqlitePool,
    notifier: OutboxNotifier,
    protocol: Arc<ScriptedEnhancer>,
) -> EnhancerService {
    let service = EnhancerService::new(pool, protocol, notifier);
    service.replace(1, enhancer_config(true)).await.unwrap();
    service
}

fn enhancer_config(enabled: bool) -> EnhancerConfigInput {
    EnhancerConfigInput {
        enabled,
        base_url: "http://127.0.0.1:11434".to_owned(),
        model: "test:model".to_owned(),
        timeout_ms: 100,
    }
}

fn movie_hints() -> EnhancementHints {
    EnhancementHints {
        title: Some("Dune".to_owned()),
        year: Some(2021),
        media_kind: Some(EnhancerMediaKind::Movie),
        season: None,
        episodes: Vec::new(),
    }
}

struct ScriptedEnhancer {
    outcomes: Mutex<VecDeque<Result<EnhancementHints, EnhancerError>>>,
    enhance_calls: AtomicUsize,
}

struct BlockingEnhancer {
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl IdentificationEnhancer for BlockingEnhancer {
    async fn probe(&self, _endpoint: EnhancerEndpoint<'_>) -> Result<EnhancerProbe, EnhancerError> {
        Ok(EnhancerProbe {
            adapter_version: "ollama-v1".to_owned(),
            model_available: true,
        })
    }

    async fn enhance(
        &self,
        _endpoint: EnhancerEndpoint<'_>,
        _input: &EnhancementInput,
    ) -> Result<EnhancementHints, EnhancerError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(movie_hints())
    }
}

impl ScriptedEnhancer {
    fn new(outcomes: Vec<Result<EnhancementHints, EnhancerError>>) -> Self {
        Self {
            outcomes: Mutex::new(outcomes.into()),
            enhance_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl IdentificationEnhancer for ScriptedEnhancer {
    async fn probe(&self, _endpoint: EnhancerEndpoint<'_>) -> Result<EnhancerProbe, EnhancerError> {
        Ok(EnhancerProbe {
            adapter_version: "ollama-v1".to_owned(),
            model_available: true,
        })
    }

    async fn enhance(
        &self,
        _endpoint: EnhancerEndpoint<'_>,
        _input: &EnhancementInput,
    ) -> Result<EnhancementHints, EnhancerError> {
        self.enhance_calls.fetch_add(1, Ordering::SeqCst);
        self.outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(EnhancerError::Unavailable))
    }
}

#[derive(Default)]
struct DuneProvider {
    searches: SearchLog,
}

#[async_trait]
impl MetadataProvider for DuneProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(vec![dune_candidate()])
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.searches
            .lock()
            .unwrap()
            .push((request.title.clone(), request.year));
        Ok(vec![dune_candidate()])
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Ok(dune_candidate())
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

fn dune_candidate() -> MetadataCandidate {
    MetadataCandidate {
        identity: CandidateIdentity {
            provider_id: 438_631,
            media_kind: ProviderMediaKind::Movie,
        },
        titles: vec![LocalizedField {
            value: "Dune".to_owned(),
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
        retrieved_at_us: 93_000_000,
    }
}

async fn configured_connectors(
    fixture: &common::TestConfigDir,
    db: &mediaflow_core::platform::db::Db,
    notifier: OutboxNotifier,
) -> ConnectorService {
    let connectors = ConnectorService::new_with_notifier(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(UnusedTester),
        notifier,
    );
    connectors
        .put_tmdb(
            TmdbConfigCommand {
                api_read_access_token: SecretString::new("test-token-123456".to_owned()),
                locale: "en-US".to_owned(),
                region: None,
            },
            0,
        )
        .await
        .unwrap();
    connectors
}

fn existing_connectors(
    fixture: &common::TestConfigDir,
    db: &mediaflow_core::platform::db::Db,
    notifier: OutboxNotifier,
) -> ConnectorService {
    ConnectorService::new_with_notifier(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(UnusedTester),
        notifier,
    )
}

async fn seed_real_inbox(pool: &sqlx::SqlitePool) -> uuid::Uuid {
    let id = uuid::Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',x'696e626f78','inbox',x'01',x'02','available',0,1,0,0)",
    )
    .bind(id.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
    id
}

fn file_hash(path: &std::path::Path) -> [u8; 32] {
    Sha256::digest(std::fs::read(path).unwrap()).into()
}

struct UnusedTester;

#[async_trait]
impl TmdbConnectionTester for UnusedTester {
    async fn test(
        &self,
        _token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
}
