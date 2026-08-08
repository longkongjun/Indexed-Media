mod common;

use std::fmt::Write as _;
use std::sync::Arc;

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
use mediaflow_core::identification::review::{ReviewCaseFilter, ReviewCaseStore};
use mediaflow_core::identification::service::IdentificationStageHandler;
use mediaflow_core::identification::store::IdentificationStore;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{OutboxNotifier, OutboxReader};
use mediaflow_core::platform::secrets::{InstanceKey, SecretBytes};
use mediaflow_core::platform::task_runtime::{ManualTaskClock, TaskClock};
use mediaflow_core::shared::error::{AppError, ErrorCode};
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::{
    ProcessingHandlerOutcome, ProcessingStageHandler, ProcessingStopToken,
};
use sha2::{Digest as _, Sha256};

const TOKEN_SENTINEL: &str = "TOKEN_SENTINEL_M3_DO_NOT_LEAK_7e4f";
const NFO_SENTINEL: &str = "NFO_RAW_SENTINEL_M3_DO_NOT_LEAK_19ab";
const DIAGNOSTIC_SENTINEL: &str = "DIAGNOSTIC_SENTINEL_M3_DO_NOT_LEAK_02cf";
const HOST_SENTINEL: &str = "HOST_PATH_SENTINEL_M3_DO_NOT_LEAK_8d51";

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn public_projections_events_diagnostics_logs_and_database_do_not_leak_m3_secrets() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let host_root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join(HOST_SENTINEL);
    let movies = host_root.join("movies");
    std::fs::create_dir_all(&movies).unwrap();
    let media = movies.join("leak.safe.2024.mkv");
    let nfo = movies.join("leak.safe.2024.nfo");
    std::fs::write(&media, b"immutable-media-security-sentinel").unwrap();
    std::fs::write(
        &nfo,
        format!(
            "<movie><title>Leak Safe</title><year>2024</year><plot>{NFO_SENTINEL}</plot>\
             <uniqueid type=\"tmdb\" default=\"true\">438631</uniqueid></movie>"
        ),
    )
    .unwrap();
    let original_hashes = (file_hash(&media), file_hash(&nfo));
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming",
            "label":"Security",
            "container_path":std::fs::canonicalize(&host_root).unwrap(),
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"movies/leak.safe.2024.mkv",
        vec![91],
        "security-worker",
    )
    .await;
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
                api_read_access_token: SecretString::new(TOKEN_SENTINEL.to_owned()),
                locale: "en-US".to_owned(),
                region: Some("US".to_owned()),
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
    let clock: Arc<dyn TaskClock> = Arc::new(ManualTaskClock::new(100_000_000));
    let handler = IdentificationStageHandler::new(
        db.pool().clone(),
        notifier,
        connectors.clone(),
        Arc::new(SecurityProvider),
        Some(fs),
        clock,
    );
    assert_eq!(
        handler
            .run(&lease, ProcessingStopToken::default())
            .await
            .unwrap(),
        ProcessingHandlerOutcome::Committed
    );
    assert_eq!((file_hash(&media), file_hash(&nfo)), original_hashes);

    let mut external = String::new();
    external.push_str(&serde_json::to_string(&connectors.get_tmdb().await.unwrap()).unwrap());
    external.push_str(
        &serde_json::to_string(
            &ProcessingStore::new(db.pool().clone())
                .list_tasks(account, &PageRequest::new(None, Some(200)).unwrap())
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    external.push_str(
        &serde_json::to_string(
            &IdentificationStore::new(db.pool().clone())
                .detail(account, lease.task.id)
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    external.push_str(
        &serde_json::to_string(
            &ReviewCaseStore::new(db.pool().clone())
                .list_active(
                    account,
                    &ReviewCaseFilter::default(),
                    &PageRequest::new(None, Some(200)).unwrap(),
                )
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    external.push_str(
        &serde_json::to_string(
            &OutboxReader::new(db.pool().clone())
                .after(0, 200)
                .await
                .unwrap(),
        )
        .unwrap(),
    );
    let safe_error = AppError::with_source(
        ErrorCode::Internal,
        std::io::Error::other(DIAGNOSTIC_SENTINEL),
    );
    write!(external, "{safe_error:?}|{safe_error}").unwrap();
    write!(
        external,
        "{:?}|{:?}|{:?}",
        SecretString::new(TOKEN_SENTINEL.to_owned()),
        SecretBytes::new(TOKEN_SENTINEL.as_bytes().to_vec()),
        InstanceKey::load_or_create(&fixture.config().config_dir).unwrap()
    )
    .unwrap();

    let ciphertext: Vec<u8> = sqlx::query_scalar(
        "SELECT secret_ciphertext FROM connectors_integrations WHERE kind='tmdb'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let key = std::fs::read(fixture.config().config_dir.join("instance.key")).unwrap();
    external.push_str(&captured_safe_startup_failure(&fixture));
    for forbidden in [
        TOKEN_SENTINEL.to_owned(),
        NFO_SENTINEL.to_owned(),
        DIAGNOSTIC_SENTINEL.to_owned(),
        HOST_SENTINEL.to_owned(),
        host_root.display().to_string(),
        hex::encode(ciphertext),
        hex::encode(key),
    ] {
        assert!(
            !external.contains(&forbidden),
            "external surface leaked forbidden sentinel"
        );
    }

    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(db.pool())
        .await
        .unwrap();
    let database = std::fs::read(fixture.database_path()).unwrap();
    for forbidden in [
        TOKEN_SENTINEL.as_bytes(),
        NFO_SENTINEL.as_bytes(),
        host_root.as_os_str().as_encoded_bytes(),
    ] {
        assert!(!contains_bytes(&database, forbidden));
    }
}

fn captured_safe_startup_failure(fixture: &common::TestConfigDir) -> String {
    let failing_config = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join(format!("{HOST_SENTINEL}-failed-start"));
    let missing_roots = failing_config.join(format!("{DIAGNOSTIC_SENTINEL}.json"));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mediaflow-core"))
        .arg("serve")
        .env("MEDIAFLOW_MODE", "development")
        .env("MEDIAFLOW_LISTEN", "127.0.0.1:0")
        .env("MEDIAFLOW_PUBLIC_ORIGIN", "http://127.0.0.1:3000")
        .env("MEDIAFLOW_CONFIG_DIR", &failing_config)
        .env("MEDIAFLOW_DEPLOYMENT_ROOTS_FILE", missing_roots)
        .env("MEDIAFLOW_WEB_DIST", failing_config.join("web"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn file_hash(path: &std::path::Path) -> [u8; 32] {
    Sha256::digest(std::fs::read(path).unwrap()).into()
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
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

struct SecurityProvider;

#[async_trait]
impl MetadataProvider for SecurityProvider {
    async fn find_external(
        &self,
        _token: &SecretBytes,
        _request: &TmdbExternalIdRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        panic!("TMDB numeric ID must use details")
    }

    async fn search(
        &self,
        _token: &SecretBytes,
        _request: &TmdbSearchRequest,
    ) -> Result<Vec<MetadataCandidate>, ProviderError> {
        panic!("NFO external ID must precede search")
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
        Ok(MetadataCandidate {
            identity: CandidateIdentity {
                provider_id,
                media_kind,
            },
            titles: vec![LocalizedField {
                value: "leak safe".to_owned(),
                language: "en-US".to_owned(),
                source: FieldLanguageSource::English,
            }],
            summaries: Vec::new(),
            release_dates: vec![LocalizedField {
                value: NaiveDate::from_ymd_opt(2024, 1, 1).unwrap(),
                language: "en-US".to_owned(),
                source: FieldLanguageSource::English,
            }],
            aliases: Vec::new(),
            episodes: Vec::new(),
            provider_version: 1,
            cache_status: CacheStatus::Miss,
            retrieved_at_us: 100_000_000,
        })
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
