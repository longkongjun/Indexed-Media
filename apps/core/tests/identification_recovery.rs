mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use async_trait::async_trait;
use chrono::NaiveDate;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{
    CacheStatus, CandidateIdentity, EpisodeIdentity, FieldLanguageSource, LocalizedField,
    MetadataCandidate, ProviderError, ProviderMediaKind, SecretString, TmdbConfigCommand,
    TmdbExternalIdRequest, TmdbSearchRequest,
};
use mediaflow_core::connectors::service::{
    ConnectorService, MetadataProvider, TmdbConnectionTester,
};
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::identification::service::IdentificationStageHandler;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::secrets::SecretBytes;
use mediaflow_core::platform::task_runtime::{ManualTaskClock, ProcessingTaskRuntime};
use mediaflow_core::tasks::processing::model::{ProcessingStage, ProcessingStatus};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::ProcessingStageHandler;
use sha2::{Digest as _, Sha256};

#[tokio::test]
async fn stable_revision_recovers_from_provider_pause_and_restart_to_confirmed_checkpoint() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision =
        common::seed_stable_revision(db.pool(), inbox, b"movies/dune.2021.mkv", vec![44]).await;
    let notifier = OutboxNotifier::new();
    let connectors = ConnectorService::new_with_notifier(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(UnusedTester),
        notifier.clone(),
    );
    connectors
        .put_tmdb(
            TmdbConfigCommand {
                api_read_access_token: SecretString::new("test-token-123456".to_owned()),
                locale: "zh-CN".to_owned(),
                region: Some("CN".to_owned()),
            },
            0,
        )
        .await
        .unwrap();
    let unavailable = Arc::new(AtomicBool::new(true));
    let provider: Arc<dyn MetadataProvider> = Arc::new(RecoveringProvider {
        unavailable: Arc::clone(&unavailable),
    });
    let clock = Arc::new(ManualTaskClock::new(100_000_000));

    let first = runtime(
        db.pool().clone(),
        notifier.clone(),
        connectors.clone(),
        Arc::clone(&provider),
        clock.clone(),
    );
    let prepared = first.prepare().await.unwrap();
    assert_eq!(prepared.ensured_tasks, 1);
    let first_handle = first.start();
    let store = ProcessingStore::new(db.pool().clone());
    let task = wait_for_status(db.pool(), &store, revision, ProcessingStatus::Paused).await;
    wait_for_integration_health(db.pool(), "unavailable").await;
    first_handle.abort();
    let _ = first_handle.await;
    assert_eq!(task.stage, ProcessingStage::Identification);
    assert!(task.next_retry_at.is_some());
    assert_eq!(task.attempt_count, 1);

    unavailable.store(false, Ordering::SeqCst);
    clock.set(161_000_000);
    let restarted = runtime(db.pool().clone(), notifier, connectors, provider, clock);
    assert_eq!(restarted.prepare().await.unwrap().recovered_leases, 0);
    let restart_handle = restarted.start();
    let confirmed = wait_for_stage(db.pool(), &store, revision, ProcessingStage::Planning).await;
    restart_handle.abort();
    let _ = restart_handle.await;
    assert_eq!(confirmed.status, ProcessingStatus::Queued);
    assert_eq!(confirmed.attempt_count, 2);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM identification_decisions d
             JOIN identification_attempts a ON a.id=d.attempt_id
             WHERE a.file_revision_id=?"
        )
        .bind(revision.as_bytes().as_slice())
        .fetch_one(db.pool())
        .await
        .unwrap(),
        2
    );
}

#[tokio::test]
async fn stable_file_and_capability_read_nfo_flow_to_external_id_confirmation_without_writes() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let inbox_path = fixture.config().config_dir.join("inbox");
    let movies = inbox_path.join("movies");
    std::fs::create_dir_all(&movies).unwrap();
    let media = movies.join("dune.2021.mkv");
    let nfo = movies.join("dune.2021.nfo");
    std::fs::write(&media, b"immutable-media-sentinel").unwrap();
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
    let revision =
        common::seed_stable_revision(db.pool(), inbox, b"movies/dune.2021.mkv", vec![77]).await;
    let notifier = OutboxNotifier::new();
    let connectors = configured_connectors(&fixture, &db, notifier.clone()).await;
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> =
        Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
    let details_calls = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn MetadataProvider> = Arc::new(NfoProvider {
        details_calls: Arc::clone(&details_calls),
    });
    let clock = Arc::new(ManualTaskClock::new(100_000_000));
    let handler: Arc<dyn ProcessingStageHandler> = Arc::new(IdentificationStageHandler::new(
        db.pool().clone(),
        notifier.clone(),
        connectors,
        provider,
        Some(fs),
        clock.clone(),
    ));
    let runtime = ProcessingTaskRuntime::new_with_concurrency(
        db.pool().clone(),
        notifier,
        &[handler],
        clock,
        1,
    )
    .unwrap();
    runtime.prepare().await.unwrap();
    let handle = runtime.start();
    let store = ProcessingStore::new(db.pool().clone());
    let task = wait_for_stage(db.pool(), &store, revision, ProcessingStage::Planning).await;
    handle.abort();
    let _ = handle.await;

    assert_eq!(task.status, ProcessingStatus::Queued);
    assert_eq!(details_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT reason FROM identification_evidence WHERE source='nfo' AND kind='external-id'"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "nfo.external-id"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT reason FROM identification_decisions ORDER BY decided_at_us DESC LIMIT 1"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        "identification.confirmed-external-id"
    );
    assert_eq!((file_hash(&media), file_hash(&nfo)), before);
}

fn runtime(
    pool: sqlx::SqlitePool,
    notifier: OutboxNotifier,
    connectors: ConnectorService,
    provider: Arc<dyn MetadataProvider>,
    clock: Arc<ManualTaskClock>,
) -> ProcessingTaskRuntime {
    let handler: Arc<dyn ProcessingStageHandler> = Arc::new(IdentificationStageHandler::new(
        pool.clone(),
        notifier.clone(),
        connectors,
        provider,
        None,
        clock.clone(),
    ));
    ProcessingTaskRuntime::new_with_concurrency(pool, notifier, &[handler], clock, 1).unwrap()
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
                locale: "zh-CN".to_owned(),
                region: Some("CN".to_owned()),
            },
            0,
        )
        .await
        .unwrap();
    connectors
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

async fn wait_for_status(
    pool: &sqlx::SqlitePool,
    store: &ProcessingStore,
    revision: uuid::Uuid,
    expected: ProcessingStatus,
) -> mediaflow_core::tasks::processing::model::ProcessingTaskView {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let task_id = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT id FROM tasks_processing_tasks WHERE file_revision_id=?",
            )
            .bind(revision.as_bytes().as_slice())
            .fetch_one(pool)
            .await
            .unwrap();
            let task = store
                .get_by_task_id(uuid::Uuid::from_slice(&task_id).unwrap())
                .await
                .unwrap();
            if task.status == expected {
                break task;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("processing status")
}

async fn wait_for_stage(
    pool: &sqlx::SqlitePool,
    store: &ProcessingStore,
    revision: uuid::Uuid,
    expected: ProcessingStage,
) -> mediaflow_core::tasks::processing::model::ProcessingTaskView {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let task_id = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT id FROM tasks_processing_tasks WHERE file_revision_id=?",
            )
            .bind(revision.as_bytes().as_slice())
            .fetch_one(pool)
            .await
            .unwrap();
            let task = store
                .get_by_task_id(uuid::Uuid::from_slice(&task_id).unwrap())
                .await
                .unwrap();
            if task.stage == expected {
                break task;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("processing stage")
}

async fn wait_for_integration_health(pool: &sqlx::SqlitePool, expected: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let health: String =
                sqlx::query_scalar("SELECT health FROM connectors_integrations WHERE kind='tmdb'")
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if health == expected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("integration health checkpoint");
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

struct RecoveringProvider {
    unavailable: Arc<AtomicBool>,
}

#[async_trait]
impl MetadataProvider for RecoveringProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.result()
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        self.result()
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        _media_kind: ProviderMediaKind,
        _provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        self.result()?.pop().ok_or(ProviderError::InvalidResponse)
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

impl RecoveringProvider {
    fn result(&self) -> Result<Vec<MetadataCandidate>, ProviderError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(ProviderError::TemporarilyUnavailable {
                retry_at_us: 160_000_000,
            });
        }
        Ok(vec![MetadataCandidate {
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
            retrieved_at_us: 160_000_000,
        }])
    }
}

struct NfoProvider {
    details_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl MetadataProvider for NfoProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        panic!("tmdb external ID should use details directly")
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        panic!("NFO external ID should precede title search")
    }

    async fn details(
        &self,
        _token: &SecretBytes,
        media_kind: ProviderMediaKind,
        provider_id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        assert_eq!(media_kind, ProviderMediaKind::Movie);
        assert_eq!(provider_id, 438_631);
        self.details_calls.fetch_add(1, Ordering::SeqCst);
        RecoveringProvider {
            unavailable: Arc::new(AtomicBool::new(false)),
        }
        .result()
        .map(|mut values| values.remove(0))
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
