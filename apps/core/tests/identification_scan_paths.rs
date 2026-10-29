//! Real scanner paths must be consumed relative to the inbox exactly once.
mod common;

use std::sync::Arc;
use std::time::Duration;

use mediaflow_core::bootstrap::app::assemble_processing_runtime;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::DiscoveryUseCases;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::CreateInboxCommand;
use mediaflow_core::discovery::policy::PutDiscoveryPolicyCommand;
use mediaflow_core::discovery::scanner::BoundedScanner;
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::db::Db;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::task_runtime::{SystemTaskClock, TaskClock};
use mediaflow_core::tasks::model::{NewScanTask, ScanStatus};
use mediaflow_core::tasks::store::TaskStore;
use mediaflow_core::tasks::worker::ScanWorker;

#[tokio::test]
async fn real_scan_root_inbox_reaches_provider_pause() {
    assert_scanned_evidence(".").await;
}

#[tokio::test]
async fn real_scan_subdirectory_inbox_reaches_provider_pause() {
    assert_scanned_evidence("inbox").await;
}

#[tokio::test]
async fn real_scan_nested_inbox_reaches_provider_pause() {
    assert_scanned_evidence("collection/inbox").await;
}

async fn scanned_fixture(inbox_path: &str) -> (common::TestConfigDir, Db) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let root = fixture.config().config_dir.join("media");
    let movies = root.join(inbox_path).join("movies");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::write(movies.join("Dune.2021.mkv"), b"scan-path-media").unwrap();
    std::fs::write(
        movies.join("Dune.2021.nfo"),
        b"<movie><title>Local Dune</title><year>2021</year></movie>",
    )
    .unwrap();
    std::fs::write(&fixture.config().deployment_roots_file, serde_json::to_vec(&serde_json::json!({"roots":[{
        "id":"incoming","label":"Incoming","container_path":std::fs::canonicalize(&root).unwrap(),"access":"read-only"
    }]})).unwrap()).unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = common::seed_account(db.pool()).await;
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> =
        Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: inbox_path.to_owned(),
        })
        .await
        .unwrap();
    discovery
        .put_policy(
            inbox.id,
            PutDiscoveryPolicyCommand {
                minimum_age_seconds: 0,
                stable_observation_interval_seconds: 1,
                reconcile_interval_seconds: 60,
                watcher_enabled: false,
            },
            1,
        )
        .await
        .unwrap();
    let scans = TaskStore::new(db.pool().clone());
    let clock = Arc::new(SystemTaskClock);
    let worker = ScanWorker::new(
        "real-scan-path".to_owned(),
        scans.clone(),
        discovery,
        Arc::new(BoundedScanner::default()),
        clock.clone(),
    );
    for ordinal in 0..2 {
        let task = scans
            .create(NewScanTask {
                account_id,
                inbox_directory_id: inbox.id,
                idempotency_key: format!("scan-path-{ordinal}"),
                now_us: clock.now_us(),
            })
            .await
            .unwrap();
        assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
        assert_eq!(
            scans.get(account_id, task.id).await.unwrap().status,
            ScanStatus::Completed
        );
        if ordinal == 0 {
            tokio::time::sleep(Duration::from_millis(1100)).await;
        }
    }
    (fixture, db)
}

async fn assert_scanned_evidence(inbox_path: &str) {
    let (fixture, db) = scanned_fixture(inbox_path).await;
    let expected = if inbox_path == "." {
        "movies/Dune.2021.mkv".to_owned()
    } else {
        format!("{inbox_path}/movies/Dune.2021.mkv")
    };
    let path: Vec<u8> = sqlx::query_scalar("SELECT relative_path_bytes FROM discovery_tracked_files WHERE relative_path_display LIKE '%.mkv'").fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        path,
        expected.as_bytes(),
        "the producer contract is root-relative"
    );
    let runtime =
        assemble_processing_runtime(fixture.config(), &db, OutboxNotifier::new()).unwrap();
    runtime.prepare().await.unwrap();
    let failures = runtime.subscribe_failures();
    let handle = runtime.start();
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(failure) = failures.borrow().clone() {return Err(failure.code);}
            let paused: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks_processing_tasks WHERE discovered_file_id IN (SELECT id FROM discovery_tracked_files WHERE relative_path_display LIKE '%.mkv') AND status='paused'").fetch_one(db.pool()).await.unwrap();
            if paused == 1 {return Ok(());}
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await;
    handle.abort();
    let _ = handle.await;
    assert_eq!(
        result.unwrap(),
        Ok(()),
        "real scan of {inbox_path} must reach a normal checkpoint"
    );
    let local_title: String = sqlx::query_scalar(
        "SELECT normalized_value FROM identification_evidence WHERE source='nfo' AND kind='title'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(local_title, "local dune");
    let attempts: i64 = sqlx::query_scalar("SELECT attempt_count FROM tasks_processing_tasks WHERE discovered_file_id IN (SELECT id FROM discovery_tracked_files WHERE relative_path_display LIKE '%.mkv')").fetch_one(db.pool()).await.unwrap();
    assert_eq!(attempts, 1);
    assert_eq!(
        std::fs::read(
            fixture
                .config()
                .config_dir
                .join("media")
                .join(inbox_path)
                .join("movies/Dune.2021.mkv")
        )
        .unwrap(),
        b"scan-path-media"
    );
    assert_eq!(
        std::fs::read(
            fixture
                .config()
                .config_dir
                .join("media")
                .join(inbox_path)
                .join("movies/Dune.2021.nfo")
        )
        .unwrap(),
        b"<movie><title>Local Dune</title><year>2021</year></movie>"
    );
}

#[tokio::test]
async fn real_scan_reaches_review_case_when_configured_provider_has_no_candidate() {
    use mediaflow_core::connectors::model::{SecretString, TmdbConfigCommand};
    use mediaflow_core::connectors::service::ConnectorService;
    use mediaflow_core::identification::service::IdentificationStageHandler;
    use mediaflow_core::platform::task_runtime::ProcessingTaskRuntime;
    use mediaflow_core::tasks::processing::worker::ProcessingStageHandler;

    let (fixture, db) = scanned_fixture("collection/inbox").await;
    let notifier = OutboxNotifier::new();
    let provider = Arc::new(NoCandidateProvider);
    let connectors = ConnectorService::new_with_notifier(
        db.pool().clone(),
        &fixture.config().config_dir,
        provider.clone(),
        notifier.clone(),
    );
    connectors
        .put_tmdb(
            TmdbConfigCommand {
                api_read_access_token: SecretString::new("scan-path-test-token".to_owned()),
                locale: "en-US".to_owned(),
                region: None,
            },
            0,
        )
        .await
        .unwrap();
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> =
        Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
    let handler: Arc<dyn ProcessingStageHandler> = Arc::new(IdentificationStageHandler::new(
        db.pool().clone(),
        notifier.clone(),
        connectors,
        provider,
        Some(fs),
        Arc::new(SystemTaskClock),
    ));
    let runtime = ProcessingTaskRuntime::new(
        db.pool().clone(),
        notifier,
        &[handler],
        Arc::new(SystemTaskClock),
    )
    .unwrap();
    runtime.prepare().await.unwrap();
    let failures = runtime.subscribe_failures();
    let handle = runtime.start();
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(failure) = failures.borrow().clone() {
                return Err(failure.code);
            }
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM identification_review_cases r
                     JOIN tasks_processing_tasks t ON t.id=r.task_id
                     JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
                     WHERE f.relative_path_display LIKE '%.mkv' AND t.status='waiting-confirmation'")
                .fetch_one(db.pool())
                .await
                .unwrap();
            if count == 1 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    handle.abort();
    let _ = handle.await;
    assert_eq!(result.unwrap(), Ok(()));
    let status:String=sqlx::query_scalar("SELECT status FROM tasks_processing_tasks WHERE discovered_file_id IN (SELECT id FROM discovery_tracked_files WHERE relative_path_display LIKE '%.mkv')").fetch_one(db.pool()).await.unwrap();
    assert_eq!(status, "waiting-confirmation");
}

use mediaflow_core::connectors::model::{
    EpisodeIdentity, MetadataCandidate, ProviderError, ProviderMediaKind, TmdbExternalIdRequest,
    TmdbSearchRequest,
};
use mediaflow_core::connectors::service::{MetadataProvider, TmdbConnectionTester};
use mediaflow_core::platform::secrets::SecretBytes;

struct NoCandidateProvider;

#[async_trait::async_trait]
impl TmdbConnectionTester for NoCandidateProvider {
    async fn test(
        &self,
        _token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl MetadataProvider for NoCandidateProvider {
    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(Vec::new())
    }
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        Ok(Vec::new())
    }
    async fn details(
        &self,
        _token: &SecretBytes,
        _kind: ProviderMediaKind,
        _id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        Err(ProviderError::InvalidResponse)
    }
    async fn verify_episodes(
        &self,
        _token: &SecretBytes,
        _id: i64,
        _episodes: &[(u16, u16)],
        _locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        Ok(Vec::new())
    }
}
