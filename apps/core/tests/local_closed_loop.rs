//! 真实 HTTP、扫描、SQLite 与 OS 文件的本机闭环；仅元数据提供方使用确定性夹具。
#![cfg(unix)]

mod common;

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::NaiveDate;
use mediaflow_core::bootstrap::app::assemble_processing_runtime;
use mediaflow_core::bootstrap::config::{AppConfig, RunMode};
use mediaflow_core::catalog::store::CatalogStore;
use mediaflow_core::connectors::model::{
    CacheStatus, CandidateIdentity, EpisodeIdentity, FieldLanguageSource, LocalizedField,
    MetadataCandidate, ProviderError, ProviderMediaKind, SecretString, TmdbConfigCommand,
    TmdbExternalIdRequest, TmdbSearchRequest,
};
use mediaflow_core::connectors::service::{
    ConnectorService, MetadataProvider, TmdbConnectionTester,
};
use mediaflow_core::discovery::capability::DeploymentRootSet;
use mediaflow_core::discovery::scanner::BoundedScanner;
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::identification::organization_port::SqliteOrganizationIdentityPort;
use mediaflow_core::identification::service::IdentificationStageHandler;
use mediaflow_core::organization::coordinator::{
    OrganizationCompletionHandler, OrganizationFileOperationHandler, OrganizationNfoHandler,
    OrganizationPlanningHandler, SqliteOrganizationPlanningPort,
};
use mediaflow_core::organization::executor::OrganizationExecutor;
use mediaflow_core::organization::journal_store::JournalStore;
use mediaflow_core::organization::plan_service::OrganizationPlanService;
use mediaflow_core::organization::plan_store::OrganizationPlanStore;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::db::Db;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::secrets::SecretBytes;
use mediaflow_core::platform::task_runtime::SystemTaskClock;
use mediaflow_core::shared::error::AppError;
use mediaflow_core::tasks::processing::model::{ProcessingLease, ProcessingStage};
use mediaflow_core::tasks::processing::store::ProcessingStore;
use mediaflow_core::tasks::processing::worker::{
    ProcessingHandlerOutcome, ProcessingStageHandler, ProcessingStopToken, ProcessingWorker,
};
use mediaflow_core::tasks::store::TaskStore;
use mediaflow_core::tasks::worker::ScanWorker;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::task::JoinHandle;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:3000";
const MEDIA: &[u8] = b"local-closed-loop-non-playable-test-media";
const SOURCE_NFO: &[u8] = b"<movie><title>Local Dune</title><year>2021</year></movie>";
const EPISODE_NFO: &[u8] = b"<episodedetails><title>Local Episode</title><showtitle>Example Show</showtitle><season>1</season><episode>2</episode></episodedetails>";

struct Journey {
    fixture: common::TestConfigDir,
    db: Db,
    client: Client,
    server: Option<JoinHandle<()>>,
    base: String,
    cookie: String,
    csrf: String,
    incoming: PathBuf,
    library: PathBuf,
    task_id: Uuid,
    episode: bool,
}

impl Drop for Journey {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}

impl Journey {
    async fn new() -> Self {
        Self::with_episode(false).await
    }

    #[allow(clippy::too_many_lines)]
    async fn with_episode(episode: bool) -> Self {
        let fixture = common::TestConfigDir::new(RunMode::Development);
        let incoming = fixture.config().config_dir.join("incoming");
        let library = fixture.config().config_dir.join("library");
        std::fs::create_dir_all(incoming.join("ready")).unwrap();
        std::fs::create_dir_all(library.join("Movies")).unwrap();
        let filename = if episode {
            "Example.Show.S01E02"
        } else {
            "Dune.2021"
        };
        std::fs::write(incoming.join(format!("ready/{filename}.mkv")), MEDIA).unwrap();
        std::fs::write(
            incoming.join(format!("ready/{filename}.nfo")),
            if episode { EPISODE_NFO } else { SOURCE_NFO },
        )
        .unwrap();
        std::fs::write(library.join("outside-canary"), b"untouched").unwrap();
        std::fs::write(
            &fixture.config().deployment_roots_file,
            serde_json::to_vec(&json!({"roots":[
                {"id":"incoming","label":"Incoming","container_path":std::fs::canonicalize(&incoming).unwrap(),"access":"read-only"},
                {"id":"library","label":"Library","container_path":std::fs::canonicalize(&library).unwrap(),"access":"read-write"}
            ]})).unwrap(),
        ).unwrap();
        let secret_path = fixture.config().config_dir.join("bootstrap.secret");
        std::fs::write(&secret_path, b"local-closed-loop-bootstrap").unwrap();
        std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let db = migrate_with_backup(fixture.config()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = build_router(fixture.config().clone(), Some(db.clone()));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap();
        let bootstrap = client.post(format!("{base}/api/v1/system/bootstrap"))
            .header("origin", ORIGIN).header("sec-fetch-site", "same-origin")
            .json(&json!({"bootstrap_secret":"local-closed-loop-bootstrap","administrator_name":"admin","password":"local isolated test password"}))
            .send().await.unwrap();
        assert_eq!(bootstrap.status(), StatusCode::CREATED);
        let login = client
            .post(format!("{base}/api/v1/sessions"))
            .header("origin", ORIGIN)
            .header("sec-fetch-site", "same-origin")
            .json(&json!({"administrator_name":"admin","password":"local isolated test password"}))
            .send()
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::CREATED);
        let cookie = login.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let csrf = login.json::<Value>().await.unwrap()["csrf_token"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut journey = Self {
            fixture,
            db,
            client,
            server: Some(server),
            base,
            cookie,
            csrf,
            incoming,
            library,
            task_id: Uuid::nil(),
            episode,
        };
        let connectors = ConnectorService::new_with_notifier(
            journey.db.pool().clone(),
            &journey.fixture.config().config_dir,
            Arc::new(LocalMetadata),
            OutboxNotifier::new(),
        );
        connectors
            .put_tmdb(
                TmdbConfigCommand {
                    api_read_access_token: SecretString::new(
                        "synthetic-local-provider-token".to_owned(),
                    ),
                    locale: "en-US".to_owned(),
                    region: None,
                },
                0,
            )
            .await
            .unwrap();
        let inbox = journey
            .request(
                Method::POST,
                "/api/v1/inbox-directories",
                Some(json!({"root_id":"incoming","relative_path":"ready"})),
                None,
                None,
                StatusCode::CREATED,
            )
            .await;
        let inbox_id = inbox["id"].as_str().unwrap();
        journey.request(Method::PUT, &format!("/api/v1/inbox-directories/{inbox_id}/discovery-policy"), Some(json!({"minimum_age_seconds":0,"stable_observation_interval_seconds":1,"reconcile_interval_seconds":60,"watcher_enabled":false})), None, Some(1), StatusCode::OK).await;
        let media_kind = if episode { "series" } else { "movie" };
        journey.request(Method::POST, "/api/v1/organization-targets", Some(json!({"kind":media_kind,"display_name":"Local films","root_id":"library","relative_path":"Movies","operation":"copy","naming_pattern":media_kind,"nfo_policy":"generate-missing","automatic":false,"enabled":true,"rules":[{"media_kind":media_kind,"inbox_directory_id":inbox_id,"explicit_tag":null,"enabled":true}]})), None, None, StatusCode::CREATED).await;
        let roots = journey.roots();
        let fs =
            Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
        let discovery = DiscoveryService::new(roots.view_map(), fs, journey.db.pool().clone());
        let scanner = ScanWorker::new(
            "local-journey-scan".to_owned(),
            TaskStore::new(journey.db.pool().clone()),
            discovery,
            Arc::new(BoundedScanner::default()),
            Arc::new(SystemTaskClock),
        );
        for ordinal in 0..2 {
            let task = journey
                .request(
                    Method::POST,
                    &format!("/api/v1/inbox-directories/{inbox_id}/scan-tasks"),
                    None,
                    Some(&format!("scan-{ordinal}")),
                    None,
                    StatusCode::ACCEPTED,
                )
                .await;
            assert_eq!(
                scanner.run_once().await.unwrap(),
                Some(Uuid::parse_str(task["id"].as_str().unwrap()).unwrap())
            );
            let scan_view = journey
                .request(
                    Method::GET,
                    &format!("/api/v1/scan-tasks/{}", task["id"].as_str().unwrap()),
                    None,
                    None,
                    None,
                    StatusCode::OK,
                )
                .await;
            assert_eq!(
                scan_view["status"], "completed",
                "scan did not complete: {scan_view}"
            );
            if ordinal == 0 {
                tokio::time::sleep(Duration::from_millis(1100)).await;
            }
        }
        journey.prepare().await;
        let observed: Vec<(String,)> =
            sqlx::query_as("SELECT relative_path_display FROM discovery_tracked_files")
                .fetch_all(journey.db.pool())
                .await
                .unwrap();
        let task: Vec<u8> = sqlx::query_scalar("SELECT t.id FROM tasks_processing_tasks t JOIN discovery_tracked_files f ON f.id=t.discovered_file_id WHERE f.relative_path_display LIKE '%.mkv'").fetch_optional(journey.db.pool()).await.unwrap().unwrap_or_else(|| panic!("missing processing task; tracked files: {observed:?}"));
        journey.task_id = Uuid::from_slice(&task).unwrap();
        journey
    }

    fn roots(&self) -> DeploymentRootSet {
        DeploymentRootSet::load(
            &self.fixture.config().deployment_roots_file,
            RunMode::Development,
        )
        .unwrap()
    }

    async fn prepare(&self) {
        assemble_processing_runtime(self.fixture.config(), &self.db, OutboxNotifier::new())
            .unwrap()
            .prepare()
            .await
            .unwrap();
    }

    async fn stop_http(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            if let Err(error) = server.await {
                assert!(error.is_cancelled(), "HTTP test server failed: {error}");
            }
        }
    }

    async fn restart(&mut self) {
        self.stop_http().await;
        self.db.pool().close().await;
        self.db = migrate_with_backup(self.fixture.config()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.base = format!("http://{}", listener.local_addr().unwrap());
        let app = build_router(self.fixture.config().clone(), Some(self.db.clone()));
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }));
        self.prepare().await;
    }

    async fn shutdown(mut self) {
        self.stop_http().await;
        self.db.pool().close().await;
        let owned_config = self.fixture.config().config_dir.clone();
        drop(self);
        assert!(
            !owned_config.exists(),
            "owned temporary instance must be removed"
        );
    }

    fn worker(&self) -> ProcessingWorker {
        worker_for(self.fixture.config(), &self.db, None)
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        key: Option<&str>,
        version: Option<i64>,
        expected: StatusCode,
    ) -> Value {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .header("cookie", &self.cookie)
            .header("origin", ORIGIN)
            .header("sec-fetch-site", "same-origin")
            .header("x-csrf-token", &self.csrf);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(key) = key {
            request = request.header("idempotency-key", key);
        }
        if let Some(version) = version {
            request = request.header("if-match", version.to_string());
        }
        let response = request.send().await.unwrap();
        let actual = response.status();
        let value = response.json::<Value>().await.unwrap();
        assert_eq!(actual, expected, "{path}: {value}");
        value
    }

    async fn organization(&self) -> Value {
        self.request(
            Method::GET,
            &format!("/api/v1/processing-tasks/{}/organization", self.task_id),
            None,
            None,
            None,
            StatusCode::OK,
        )
        .await
    }

    async fn step(&self, worker: &ProcessingWorker) {
        for _ in 0..4 {
            let id = worker
                .run_once()
                .await
                .unwrap()
                .expect("expected pending production work");
            if id == self.task_id {
                return;
            }
            let task = self
                .request(
                    Method::GET,
                    &format!("/api/v1/processing-tasks/{id}"),
                    None,
                    None,
                    None,
                    StatusCode::OK,
                )
                .await;
            assert!(
                std::path::Path::new(task["relative_path"].as_str().unwrap())
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("nfo")),
                "unexpected additional task: {task}"
            );
        }
        panic!("media task did not run after bounded companion-file work");
    }

    async fn plan(&self) -> Value {
        let worker = self.worker();
        self.step(&worker).await;
        let title: String = sqlx::query_scalar("SELECT normalized_value FROM identification_evidence WHERE source='nfo' AND kind='title'").fetch_one(self.db.pool()).await.unwrap();
        assert_eq!(
            title,
            if self.episode {
                "example show"
            } else {
                "local dune"
            }
        );
        let case = self
            .request(
                Method::GET,
                "/api/v1/review-cases",
                None,
                None,
                None,
                StatusCode::OK,
            )
            .await;
        let case = case["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["task_id"] == self.task_id.to_string())
            .expect("scanned movie must have a review case");
        let case_id = case["id"].as_str().unwrap();
        let version = case["version"].as_i64().unwrap();
        let decision = json!({"kind":"select-provider-candidate","provider":"tmdb","media_type":if self.episode { "tv" } else { "movie" },"provider_id":if self.episode { "1399" } else { "438631" },"save_feedback":false});
        let path = format!("/api/v1/review-cases/{case_id}/decisions");
        let accepted = self
            .request(
                Method::POST,
                &path,
                Some(decision.clone()),
                Some("local-decision"),
                Some(version),
                StatusCode::ACCEPTED,
            )
            .await;
        let replay = self
            .request(
                Method::POST,
                &path,
                Some(decision),
                Some("local-decision"),
                Some(version),
                StatusCode::ACCEPTED,
            )
            .await;
        assert_eq!(accepted, replay);
        self.prepare().await;
        self.step(&worker).await;
        let account = ProcessingStore::new(self.db.pool().clone())
            .account_id(self.task_id)
            .await
            .unwrap();
        let source =
            mediaflow_core::discovery::organization_projection::OrganizationInboxProjection::new(
                self.db.pool().clone(),
            )
            .source_for_task(account, self.task_id)
            .await
            .unwrap();
        assert_eq!(
            source.relative_path.as_str(),
            self.source_path("mkv"),
            "the scanner stores a deployment-root-relative path, not an inbox-relative path"
        );
        self.step(&worker).await;
        let plan = self.organization().await;
        assert_eq!(
            plan["plan"]["authorization"], "paused",
            "unexpected organization projection: {plan}"
        );
        assert!(
            plan["allowed_actions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action == "execute")
        );
        assert!(
            !self.destination(&plan).exists(),
            "no write before approval"
        );
        plan
    }

    fn destination(&self, plan: &Value) -> PathBuf {
        self.library.join(
            plan["plan"]["destination"]["relative_path"]
                .as_str()
                .unwrap(),
        )
    }

    async fn approve(&self, plan: &Value) {
        let path = format!(
            "/api/v1/processing-tasks/{}/organization/executions",
            self.task_id
        );
        let version = plan["plan"]["version"].as_i64().unwrap();
        self.request(
            Method::POST,
            &path,
            Some(json!({"plan_version":version+1})),
            Some("stale-approval"),
            None,
            StatusCode::CONFLICT,
        )
        .await;
        let (accepted, concurrent_replay) = tokio::join!(
            self.request(
                Method::POST,
                &path,
                Some(json!({"plan_version":version})),
                Some("local-approval"),
                None,
                StatusCode::ACCEPTED
            ),
            self.request(
                Method::POST,
                &path,
                Some(json!({"plan_version":version})),
                Some("local-approval"),
                None,
                StatusCode::ACCEPTED
            ),
        );
        assert_eq!(accepted, concurrent_replay);
    }

    async fn assert_catalog(&self) -> Value {
        let task = self
            .request(
                Method::GET,
                &format!("/api/v1/processing-tasks/{}", self.task_id),
                None,
                None,
                None,
                StatusCode::OK,
            )
            .await;
        assert_eq!(task["status"], "completed");
        let organization = self.organization().await;
        assert_eq!(organization["local_result"]["status"], "completed");
        assert_eq!(organization["local_result"]["nfo_status"], "generated");
        let items = self
            .request(
                Method::GET,
                "/api/v1/media-items",
                None,
                None,
                None,
                StatusCode::OK,
            )
            .await;
        assert_eq!(items["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            items["items"][0]["title"],
            if self.episode { "Example Show" } else { "Dune" }
        );
        let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM catalog_applied_local_results")
            .fetch_one(self.db.pool())
            .await
            .unwrap();
        assert_eq!(applied, 1);
        self.assert_canaries();
        organization
    }

    fn source_path(&self, extension: &str) -> String {
        format!(
            "ready/{}.{}",
            if self.episode {
                "Example.Show.S01E02"
            } else {
                "Dune.2021"
            },
            extension
        )
    }

    fn assert_canaries(&self) {
        assert_eq!(
            std::fs::read(self.incoming.join(self.source_path("mkv"))).unwrap(),
            MEDIA
        );
        assert_eq!(
            std::fs::read(self.incoming.join(self.source_path("nfo"))).unwrap(),
            if self.episode {
                EPISODE_NFO
            } else {
                SOURCE_NFO
            }
        );
        assert_eq!(
            std::fs::read(self.library.join("outside-canary")).unwrap(),
            b"untouched"
        );
    }

    async fn assert_catalog_retracted(&self, result: &Value) {
        let media_id = result["local_result"]["catalog_media_item_id"]
            .as_str()
            .unwrap();
        let items = self
            .request(
                Method::GET,
                "/api/v1/media-items",
                None,
                None,
                None,
                StatusCode::OK,
            )
            .await;
        assert_eq!(
            items["items"],
            json!([]),
            "rolled back current projection must leave the visible Catalog"
        );
        self.request(
            Method::GET,
            &format!("/api/v1/media-items/{media_id}"),
            None,
            None,
            None,
            StatusCode::NOT_FOUND,
        )
        .await;
        let libraries = self
            .request(
                Method::GET,
                "/api/v1/media-libraries",
                None,
                None,
                None,
                StatusCode::OK,
            )
            .await;
        assert_eq!(
            libraries["items"],
            json!([]),
            "libraries without visible media must not remain discoverable"
        );
        // 幂等 receipt 留作历史；撤销可见投影不应删除这一事实。
        let receipts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM catalog_applied_local_results")
                .fetch_one(self.db.pool())
                .await
                .unwrap();
        assert_eq!(receipts, 1);
        eprintln!(
            "local Catalog compensation: media_id={media_id} list=empty detail=404 libraries=empty receipt=preserved"
        );
    }

    async fn rollback(&self, result: &Value, key: &str, status: StatusCode) -> Value {
        self.request(
            Method::POST,
            &format!(
                "/api/v1/processing-tasks/{}/organization/rollbacks",
                self.task_id
            ),
            Some(json!({"result_version":result["local_result"]["version"]})),
            Some(key),
            None,
            status,
        )
        .await
    }
}

#[tokio::test]
async fn local_http_movie_reconstructs_from_approved_plan_and_safely_rolls_back() {
    let mut journey = Journey::new().await;
    let plan = journey.plan().await;
    journey.approve(&plan).await;
    journey.restart().await;
    // 重建 worker 重新打开根能力和仓库，不向任务或 journal 注入任何状态。
    let restarted = journey.worker();
    for _ in 0..4 {
        journey.step(&restarted).await;
    }
    let result = journey.assert_catalog().await;
    let destination = journey.destination(&plan);
    assert_eq!(std::fs::read(&destination).unwrap(), MEDIA);
    let nfo = destination.with_extension("nfo");
    let xml = std::fs::read_to_string(&nfo).unwrap();
    assert!(xml.contains("438631"));
    assert!(xml.contains("<title>Dune</title>"));
    let mut stale = result.clone();
    stale["local_result"]["version"] =
        json!(result["local_result"]["version"].as_i64().unwrap() + 1);
    journey
        .rollback(&stale, "stale-rollback", StatusCode::CONFLICT)
        .await;
    assert!(destination.exists());
    let compensated = journey
        .rollback(&result, "local-rollback", StatusCode::ACCEPTED)
        .await;
    let replay = journey
        .rollback(&result, "local-rollback", StatusCode::ACCEPTED)
        .await;
    assert_eq!(compensated, replay);
    assert_eq!(
        journey.organization().await["local_result"]["status"],
        "compensated"
    );
    assert!(!destination.exists());
    assert!(!nfo.exists());
    journey.assert_catalog_retracted(&result).await;
    journey.assert_canaries();
    eprintln!(
        "local closed loop: HTTP bootstrap/scan/NFO/review/approval/reconstructed worker/catalog/stale and idempotent rollback passed; metadata fixture only"
    );
    journey.shutdown().await;
}

#[tokio::test]
async fn local_http_partial_nfo_failure_retries_only_remaining_work_after_reconstruction() {
    let mut journey = Journey::new().await;
    let plan = journey.plan().await;
    journey.approve(&plan).await;
    let worker = journey.worker();
    journey.step(&worker).await;
    journey.step(&worker).await;
    let destination = journey.destination(&plan);
    let before = std::fs::metadata(&destination).unwrap();
    let journal_before = journey.organization().await["journals"].clone();
    let leaf = destination.parent().unwrap();
    let permissions = LeafPermissionRestore::deny_write(leaf);
    let probe = std::fs::write(leaf.join("permission-probe"), b"must fail");
    assert!(
        probe.is_err(),
        "this test requires an OS identity for which leaf write permissions are enforced"
    );
    let outcome = worker.run_once().await;
    permissions.restore();
    assert_eq!(outcome.unwrap(), Some(journey.task_id));
    let partial = journey.organization().await;
    assert_eq!(partial["local_result"]["status"], "partial-success");
    assert_eq!(partial["local_result"]["remaining_actions"], json!(["nfo"]));
    assert!(!destination.with_extension("nfo").exists());
    assert_eq!(std::fs::metadata(&destination).unwrap().ino(), before.ino());
    drop(worker);
    journey.restart().await;
    assert_eq!(
        journey.organization().await["local_result"]["status"],
        "partial-success"
    );
    journey
        .request(
            Method::POST,
            &format!("/api/v1/processing-tasks/{}/attempts", journey.task_id),
            None,
            Some("partial-nfo-retry"),
            None,
            StatusCode::ACCEPTED,
        )
        .await;
    let restarted = journey.worker();
    journey.step(&restarted).await;
    journey.step(&restarted).await;
    let result = journey.assert_catalog().await;
    assert_eq!(std::fs::metadata(&destination).unwrap().ino(), before.ino());
    assert_eq!(std::fs::read(&destination).unwrap(), MEDIA);
    assert_eq!(
        result["journals"][0]["operation_id"],
        journal_before[0]["operation_id"]
    );
    assert_eq!(
        result["journals"][0]["projection_version"]
            .as_i64()
            .unwrap(),
        journal_before[0]["projection_version"].as_i64().unwrap()
    );
    let copies: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM organization_file_operation_journals WHERE kind='copy'",
    )
    .fetch_one(journey.db.pool())
    .await
    .unwrap();
    assert_eq!(copies, 1);
    journey
        .rollback(&result, "partial-then-rollback", StatusCode::ACCEPTED)
        .await;
    assert!(!destination.exists());
    assert!(!destination.with_extension("nfo").exists());
    journey.assert_catalog_retracted(&result).await;
    journey.assert_canaries();
    eprintln!(
        "local closed loop: real OS permission failure -> partial-success -> HTTP retry -> reconstructed worker -> Catalog; copied inode/operation unchanged; rollback passed"
    );
    journey.shutdown().await;
}

#[tokio::test]
async fn local_http_rollback_preserves_an_external_change_after_catalog_completion() {
    let journey = Journey::new().await;
    let plan = journey.plan().await;
    journey.approve(&plan).await;
    let worker = journey.worker();
    for _ in 0..4 {
        journey.step(&worker).await;
    }
    let result = journey.assert_catalog().await;
    let destination = journey.destination(&plan);
    std::fs::write(&destination, b"external owned replacement").unwrap();
    journey
        .rollback(&result, "external-change-rollback", StatusCode::ACCEPTED)
        .await;
    assert_eq!(
        journey.organization().await["local_result"]["status"],
        "manual-review"
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"external owned replacement"
    );
    journey.assert_catalog_retracted(&result).await;
    journey.assert_canaries();
    eprintln!(
        "local closed loop: external changed destination preserved and rollback marked manual-review"
    );
    journey.shutdown().await;
}

#[tokio::test]
async fn local_http_episode_builds_series_season_episode_catalog_and_rolls_back() {
    let journey = Journey::with_episode(true).await;
    let plan = journey.plan().await;
    journey.approve(&plan).await;
    let worker = journey.worker();
    for _ in 0..4 {
        journey.step(&worker).await;
    }
    let result = journey.assert_catalog().await;
    let items = journey
        .request(
            Method::GET,
            "/api/v1/media-items",
            None,
            None,
            None,
            StatusCode::OK,
        )
        .await;
    let item = &items["items"][0];
    assert_eq!(item["type"], "series");
    let detail = journey
        .request(
            Method::GET,
            &format!("/api/v1/media-items/{}", item["id"].as_str().unwrap()),
            None,
            None,
            None,
            StatusCode::OK,
        )
        .await;
    assert_eq!(detail["versions"], json!([]));
    let children = detail["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    let season = children
        .iter()
        .find(|node| node["type"] == "season")
        .unwrap();
    let episode = children
        .iter()
        .find(|node| node["type"] == "episode")
        .unwrap();
    assert_eq!(season["ordinal"], 1);
    assert_eq!(episode["ordinal"], 2);
    assert_eq!(episode["parent_id"], season["id"]);
    assert_eq!(season["versions"], json!([]));
    let versions = episode["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0]["files"].as_array().unwrap().len(), 1);
    assert_eq!(
        versions[0]["files"][0]["source_relative_path"],
        journey.source_path("mkv")
    );
    assert_eq!(
        versions[0]["files"][0]["current_relative_path"],
        plan["plan"]["destination"]["relative_path"]
    );
    assert_eq!(detail["related_task_ids"], json!([journey.task_id]));
    let destination = journey.destination(&plan);
    let nfo = std::fs::read_to_string(destination.with_extension("nfo")).unwrap();
    assert!(nfo.contains("<episodedetails>"));
    assert!(nfo.contains("<season>1</season>"));
    assert!(nfo.contains("<episode>2</episode>"));
    journey
        .rollback(&result, "episode-rollback", StatusCode::ACCEPTED)
        .await;
    assert!(!destination.exists());
    assert!(!destination.with_extension("nfo").exists());
    journey.assert_catalog_retracted(&result).await;
    journey.assert_canaries();
    eprintln!(
        "local closed loop: real scan -> NFO -> manual TV selection with fixture episode verification -> approval -> copy/NFO -> series/season1/episode2 Catalog -> rollback passed"
    );
    journey.shutdown().await;
}

#[tokio::test]
async fn local_http_file_stage_preserves_a_destination_created_after_approval() {
    let journey = Journey::new().await;
    let plan = journey.plan().await;
    journey.approve(&plan).await;
    let worker = journey.worker();
    journey.step(&worker).await;
    let destination = journey.destination(&plan);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::write(&destination, b"external winner before file execution").unwrap();
    journey.step(&worker).await;
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"external winner before file execution"
    );
    let organization = journey.organization().await;
    assert_eq!(organization["state"], "manual-review", "{organization}");
    assert!(organization["local_result"].is_null());
    assert_eq!(organization["journals"][0]["status"], "manual-review");
    let items = journey
        .request(
            Method::GET,
            "/api/v1/media-items",
            None,
            None,
            None,
            StatusCode::OK,
        )
        .await;
    assert_eq!(items["items"], json!([]));
    assert!(!destination.with_extension("nfo").exists());
    journey.assert_canaries();
    eprintln!(
        "local closed loop: destination created after approval preserved; manual review and empty Catalog passed"
    );
    journey.shutdown().await;
}

/// 空搜索实际生成 ReviewCase，明确人工选择后才返回有界测试候选。
struct LocalMetadata;

#[async_trait]
impl TmdbConnectionTester for LocalMetadata {
    async fn test(
        &self,
        _token: &SecretBytes,
        _locale: &str,
        _region: Option<&str>,
    ) -> Result<(), ProviderError> {
        Ok(())
    }
}

#[async_trait]
impl MetadataProvider for LocalMetadata {
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
        kind: ProviderMediaKind,
        id: i64,
        _locale: &str,
    ) -> Result<MetadataCandidate, ProviderError> {
        assert!(matches!(
            (kind, id),
            (ProviderMediaKind::Movie, 438_631) | (ProviderMediaKind::Tv, 1399)
        ));
        Ok(MetadataCandidate {
            identity: CandidateIdentity {
                provider_id: id,
                media_kind: kind,
            },
            titles: vec![LocalizedField {
                value: if kind == ProviderMediaKind::Movie {
                    "Dune"
                } else {
                    "Example Show"
                }
                .to_owned(),
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
            retrieved_at_us: 1,
        })
    }
    async fn verify_episodes(
        &self,
        _token: &SecretBytes,
        id: i64,
        episodes: &[(u16, u16)],
        _locale: &str,
    ) -> Result<Vec<EpisodeIdentity>, ProviderError> {
        assert_eq!(id, 1399);
        assert_eq!(episodes, &[(1, 2)]);
        Ok(vec![EpisodeIdentity {
            season: 1,
            episode: 2,
        }])
    }
}

fn worker_for(config: &AppConfig, db: &Db, interrupt: Option<PathBuf>) -> ProcessingWorker {
    let roots = DeploymentRootSet::load(&config.deployment_roots_file, config.mode).unwrap();
    let fs = Arc::new(OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap());
    let clock = Arc::new(SystemTaskClock);
    let notifier = OutboxNotifier::new();
    let pool = db.pool().clone();
    let tasks = ProcessingStore::new_with_notifier(pool.clone(), notifier.clone());
    let identity = Arc::new(SqliteOrganizationIdentityPort::new(pool.clone()));
    let planning = Arc::new(SqliteOrganizationPlanningPort::new(
        pool.clone(),
        identity.clone(),
        fs.clone(),
        roots
            .views()
            .iter()
            .map(|root| (root.id.clone(), root.access))
            .collect(),
    ));
    let executor = OrganizationExecutor::unscoped(
        OrganizationPlanStore::new(pool.clone()),
        JournalStore::new_with_notifier(pool.clone(), notifier.clone()),
        fs.clone(),
        clock.clone(),
    );
    let connectors = ConnectorService::new_with_notifier(
        pool.clone(),
        &config.config_dir,
        Arc::new(LocalMetadata),
        notifier.clone(),
    );
    let handlers: Vec<Arc<dyn ProcessingStageHandler>> = vec![
        Arc::new(IdentificationStageHandler::new(
            pool.clone(),
            notifier.clone(),
            connectors,
            Arc::new(LocalMetadata),
            Some(fs),
            clock.clone(),
        )),
        Arc::new(OrganizationPlanningHandler::new_dynamic(
            OrganizationPlanService::new(planning, pool.clone()),
            tasks.clone(),
            clock.clone(),
        )),
        match interrupt {
            Some(marker) => Arc::new(InterruptAfterFileCommit {
                executor: executor.clone(),
                tasks: tasks.clone(),
                marker,
            }),
            None => Arc::new(OrganizationFileOperationHandler::new(
                executor.clone(),
                tasks.clone(),
                clock.clone(),
            )),
        },
        Arc::new(OrganizationNfoHandler::new(
            executor,
            tasks.clone(),
            clock.clone(),
        )),
        Arc::new(OrganizationCompletionHandler::new_dynamic(
            pool.clone(),
            JournalStore::new_with_notifier(pool.clone(), notifier.clone()),
            identity,
            Arc::new(CatalogStore::new_with_notifier(pool, notifier)),
            tasks.clone(),
            clock.clone(),
        )),
    ];
    ProcessingWorker::new(
        format!("local-journey-{}", Uuid::now_v7()),
        tasks,
        handlers,
        clock,
    )
    .unwrap()
}

/// 仅测试使用：正式 executor 已提交文件 journal/LocalResult 后、任务推进 NFO 前挂起。
/// 子进程被 SIGKILL 后不会走 Rust 析构或协作停止；恢复完全交给新进程的正式 worker。
struct InterruptAfterFileCommit {
    executor: OrganizationExecutor,
    tasks: ProcessingStore,
    marker: PathBuf,
}

#[async_trait]
impl ProcessingStageHandler for InterruptAfterFileCommit {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::FileOperation
    }

    async fn run(
        &self,
        lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        let account = self.tasks.account_id(lease.task.id).await?;
        let outcome = self
            .executor
            .for_account(account)
            .run_file_stage(
                lease,
                mediaflow_core::organization::fs::ProcessingStopToken::default(),
            )
            .await?;
        assert!(
            matches!(
                outcome,
                mediaflow_core::organization::executor::ExecutionOutcome::Completed(_)
            ),
            "{outcome:?}"
        );
        std::fs::write(&self.marker, serde_json::to_vec(&json!({"task_id":lease.task.id,"lease_expires_at_us":lease.expires_at_us,"point":"executor.run_file_stage committed; before finish_organization_file"})).unwrap()).unwrap();
        std::future::pending().await
    }
}

/// 独立进程入口：普通 suite 无子进程环境时只验证入口可用，不创建夹具。
#[tokio::test]
async fn local_process_worker_fixture() {
    let Ok(mode) = std::env::var("MEDIAFLOW_LOCAL_TEST_CHILD") else {
        return;
    };
    let config = AppConfig::load().unwrap();
    let db = migrate_with_backup(&config).await.unwrap();
    let task_id = Uuid::parse_str(&std::env::var("MEDIAFLOW_LOCAL_TEST_TASK").unwrap()).unwrap();
    let marker = config.config_dir.join("file-stage-interruption.json");
    let worker = worker_for(&config, &db, (mode == "interrupt").then_some(marker));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(100);
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "child recovery deadline exceeded"
        );
        let readiness = assemble_processing_runtime(&config, &db, OutboxNotifier::new())
            .unwrap()
            .prepare()
            .await
            .unwrap();
        if readiness.recovered_leases > 0 {
            eprintln!(
                "local process child: recovered_leases={}",
                readiness.recovered_leases
            );
        }
        worker.run_once().await.unwrap();
        let status: String =
            sqlx::query_scalar("SELECT status FROM tasks_processing_tasks WHERE id=?")
                .bind(task_id.as_bytes().as_slice())
                .fetch_one(db.pool())
                .await
                .unwrap();
        if status == "completed" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    db.pool().close().await;
}

/// 父测试异常退出时也只终止并回收自己创建的进程。
struct OwnedChild(std::process::Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Journey {
    fn child(&self, mode: &str) -> OwnedChild {
        OwnedChild(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "local_process_worker_fixture", "--nocapture"])
                .env("MEDIAFLOW_LOCAL_TEST_CHILD", mode)
                .env("MEDIAFLOW_LOCAL_TEST_TASK", self.task_id.to_string())
                .env("MEDIAFLOW_MODE", "development")
                .env("MEDIAFLOW_LISTEN", "127.0.0.1:0")
                .env("MEDIAFLOW_CONFIG_DIR", &self.fixture.config().config_dir)
                .env(
                    "MEDIAFLOW_DEPLOYMENT_ROOTS_FILE",
                    &self.fixture.config().deployment_roots_file,
                )
                .env("MEDIAFLOW_PUBLIC_ORIGIN", ORIGIN)
                .env("MEDIAFLOW_WEB_DIST", &self.fixture.config().web_dist)
                .env_remove("MEDIAFLOW_CAPABILITY_TOKEN")
                .stdin(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    }

    async fn persisted_task(&self) -> Value {
        use sqlx::Row as _;
        let row = sqlx::query("SELECT hex(id) AS id,hex(file_revision_id) AS revision,status,stage,attempt_count,lease_owner,lease_expires_at_us FROM tasks_processing_tasks WHERE id=?").bind(self.task_id.as_bytes().as_slice()).fetch_one(self.db.pool()).await.unwrap();
        json!({"id":row.get::<String,_>("id"),"revision":row.get::<String,_>("revision"),"status":row.get::<String,_>("status"),"stage":row.get::<String,_>("stage"),"attempt":row.get::<i64,_>("attempt_count"),"lease_owner":row.get::<Option<String>,_>("lease_owner"),"lease_expires_at_us":row.get::<Option<i64>,_>("lease_expires_at_us")})
    }
}

#[tokio::test]
async fn local_process_sigkill_after_file_commit_recovers_original_operation_over_75_seconds() {
    let mut journey = Journey::new().await;
    let plan = journey.plan().await;
    journey.approve(&plan).await;
    journey.step(&journey.worker()).await;
    let mut interrupted = journey.child("interrupt");
    let marker = journey
        .fixture
        .config()
        .config_dir
        .join("file-stage-interruption.json");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "file commit interruption point not reached"
        );
        assert!(
            interrupted.0.try_wait().unwrap().is_none(),
            "child exited before interruption"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let before = journey.persisted_task().await;
    assert_eq!(before["status"], "running");
    assert_eq!(before["stage"], "file-operation");
    let destination = journey.destination(&plan);
    let inode = std::fs::metadata(&destination).unwrap().ino();
    let checksum = hex::encode(Sha256::digest(std::fs::read(&destination).unwrap()));
    let journal = journey.organization().await["journals"][0].clone();
    assert_eq!(journal["status"], "verified");
    interrupted.0.kill().unwrap();
    let exit = interrupted.0.wait().unwrap();
    assert_eq!(exit.signal(), Some(9));
    eprintln!(
        "local process SIGKILL: pid={} signal={:?} before={} journal={} inode={} sha256={} marker={}",
        interrupted.0.id(),
        exit.signal(),
        before,
        journal,
        inode,
        checksum,
        std::fs::read_to_string(marker).unwrap()
    );
    journey.restart().await;
    let started = tokio::time::Instant::now();
    let mut recovered = journey.child("recover");
    for sample in 0..=15 {
        tokio::time::sleep_until(started + Duration::from_secs(sample * 5)).await;
        let observed = journey.persisted_task().await;
        assert_eq!(observed["id"], before["id"]);
        assert_eq!(observed["revision"], before["revision"]);
        eprintln!(
            "local process recovery timeline: elapsed_ms={} task={observed}",
            started.elapsed().as_millis()
        );
        if sample == 0 {
            assert_eq!(observed["status"], "running");
            assert_eq!(observed["lease_owner"], before["lease_owner"]);
        }
    }
    assert!(started.elapsed() >= Duration::from_secs(75));
    let recovered_exit = recovered
        .0
        .try_wait()
        .unwrap()
        .expect("recovery child must exit after completion within observation window");
    assert!(recovered_exit.success());
    let after = journey.persisted_task().await;
    assert_eq!(after["status"], "completed");
    assert!(after["lease_owner"].is_null());
    let result = journey.assert_catalog().await;
    assert_eq!(
        result["journals"][0]["operation_id"],
        journal["operation_id"]
    );
    assert_eq!(result["journals"][0]["id"], journal["id"]);
    assert_eq!(std::fs::metadata(&destination).unwrap().ino(), inode);
    assert_eq!(
        hex::encode(Sha256::digest(std::fs::read(&destination).unwrap())),
        checksum
    );
    let copy_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM organization_file_operation_journals WHERE kind='copy'",
    )
    .fetch_one(journey.db.pool())
    .await
    .unwrap();
    assert_eq!(copy_count, 1);
    eprintln!(
        "local process recovery result: after={} unchanged_operation={} unchanged_inode={} sha256={} copies={copy_count}",
        after, journal["operation_id"], inode, checksum
    );
    journey
        .rollback(&result, "sigkill-recovery-rollback", StatusCode::ACCEPTED)
        .await;
    assert!(!destination.exists());
    journey.assert_catalog_retracted(&result).await;
    journey.assert_canaries();
    journey.shutdown().await;
}

/// 仅管理本次临时叶目录权限；handler panic/unwind 时同样恢复，避免夹具清理遗留。
struct LeafPermissionRestore {
    path: PathBuf,
    original: std::fs::Permissions,
    restored: bool,
}

impl LeafPermissionRestore {
    fn deny_write(path: &std::path::Path) -> Self {
        let guard = Self {
            path: path.to_owned(),
            original: std::fs::metadata(path).unwrap().permissions(),
            restored: false,
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o500)).unwrap();
        guard
    }

    fn restore(mut self) {
        std::fs::set_permissions(&self.path, self.original.clone()).unwrap();
        self.restored = true;
    }
}

impl Drop for LeafPermissionRestore {
    fn drop(&mut self) {
        if !self.restored
            && let Err(error) = std::fs::set_permissions(&self.path, self.original.clone())
        {
            eprintln!("temporary test leaf permission restoration failed: {error}");
        }
    }
}
