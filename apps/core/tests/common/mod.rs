#![allow(dead_code)]

pub mod organization;

use std::path::{Path, PathBuf};

use mediaflow_core::bootstrap::config::{AppConfig, RunMode};
use mediaflow_core::platform::backup::BackupManifest;
use mediaflow_core::platform::db::open_pool;
use tempfile::TempDir;
use url::Url;
use uuid::Uuid;

/// 具有可变启动设置和夹具辅助方法的隔离临时 Core 配置卷。
///
/// 丢弃该值会移除临时根目录。返回路径仅在其生命周期内有效。
pub struct TestConfigDir {
    root: TempDir,
    config: AppConfig,
}

impl TestConfigDir {
    /// 使用空部署根目录文档创建开发/生产配置。
    ///
    /// 在辅助方法/测试请求前，不会创建数据库、备份目录和 Web 发行内容。
    ///
    /// # Panics
    ///
    /// 无法创建临时目录或基线夹具文件时 panic。
    pub fn new(mode: RunMode) -> Self {
        let root = tempfile::tempdir().expect("temporary test root");
        let config_dir = root.path().join("config");
        std::fs::create_dir(&config_dir).expect("config directory");
        let web_dist = root.path().join("web-dist");
        let deployment_roots_file = root.path().join("deployment-roots.json");
        std::fs::write(&deployment_roots_file, b"{\"roots\":[]}\n")
            .expect("deployment roots fixture");

        Self {
            root,
            config: AppConfig {
                mode,
                listen: "127.0.0.1:0".parse().expect("listen address"),
                config_dir,
                public_origin: Url::parse("http://127.0.0.1:3000").expect("public origin"),
                trusted_proxy_cidrs: Vec::new(),
                deployment_roots_file,
                web_dist,
            },
        }
    }

    /// 创建开发夹具，其中包含版本为 `version` 的旧数据库和哨兵行。
    ///
    /// # Panics
    ///
    /// 无法打开数据库或架构/哨兵语句失败时 panic。
    pub async fn with_schema_version(version: i64) -> Self {
        let fixture = Self::new(RunMode::Development);
        let pool = open_pool(&fixture.database_path())
            .await
            .expect("legacy database pool");
        sqlx::query(&format!("PRAGMA user_version = {version}"))
            .execute(&pool)
            .await
            .expect("legacy schema version");
        sqlx::query("CREATE TABLE legacy_sentinel (value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .expect("legacy table");
        sqlx::query("INSERT INTO legacy_sentinel (value) VALUES ('before-migration')")
            .execute(&pool)
            .await
            .expect("legacy row");
        pool.close().await;
        fixture
    }

    /// 借用夹具当前的应用配置。
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// 可变借用配置，以便测试注入一个无效/就绪状态条件。
    pub fn config_mut(&mut self) -> &mut AppConfig {
        &mut self.config
    }

    /// 返回 `<config_dir>/mediaflow.db`；该文件可能尚不存在。
    pub fn database_path(&self) -> PathBuf {
        self.config.config_dir.join("mediaflow.db")
    }

    /// 返回 `<config_dir>/backups`；该目录可能尚不存在。
    pub fn backups_dir(&self) -> PathBuf {
        self.config.config_dir.join("backups")
    }

    /// 使用确定性的 `index.html` 和 `app.js` 文件创建已配置的 Web 发行内容。
    ///
    /// # Panics
    ///
    /// 目录创建或文件写入失败时 panic。
    pub fn create_web_dist(&self) {
        std::fs::create_dir_all(&self.config.web_dist).expect("web dist");
        std::fs::write(
            self.config.web_dist.join("index.html"),
            b"<!doctype html><title>MediaFlow test SPA</title>",
        )
        .expect("test index");
        std::fs::write(
            self.config.web_dist.join("app.js"),
            b"console.log('mediaflow')",
        )
        .expect("test asset");
    }

    /// 在备份路径放置常规文件，以测试目录创建失败处理。
    ///
    /// # Panics
    ///
    /// 无法写入阻塞文件时 panic。
    pub fn block_backup_directory_with_file(&self) {
        std::fs::write(self.backups_dir(), b"not a directory").expect("blocking backups path file");
    }

    /// 打开夹具数据库、返回 `PRAGMA user_version` 并关闭连接池。
    ///
    /// # Panics
    ///
    /// 无法打开或查询数据库时 panic。
    pub async fn schema_version(&self) -> i64 {
        let pool = open_pool(&self.database_path())
            .await
            .expect("database pool");
        let version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
            .fetch_one(&pool)
            .await
            .expect("schema version");
        pool.close().await;
        version
    }

    /// 返回备份目录直接下方已排序的 `.db` 产物。
    ///
    /// 目录缺失/不可读时产生空向量。
    pub fn backup_files(&self) -> Vec<PathBuf> {
        read_files_with_extension(&self.backups_dir(), "db")
    }

    /// 返回备份目录直接下方已排序的 `*.manifest.json` 文件。
    ///
    /// 目录缺失/不可读时产生空向量。
    pub fn manifest_files(&self) -> Vec<PathBuf> {
        read_matching_files(&self.backups_dir(), |path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".manifest.json"))
        })
    }

    /// 返回已排序的 `*.deployment-roots.json` 备份文件。
    ///
    /// 目录缺失/不可读时产生空向量。
    pub fn backup_config_files(&self) -> Vec<PathBuf> {
        read_matching_files(&self.backups_dir(), |path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".deployment-roots.json"))
        })
    }

    /// 读取并解码夹具的单个备份清单。
    ///
    /// # Panics
    ///
    /// 除非恰有一个清单且其字节可读并为有效 JSON，否则 panic。
    pub fn read_manifest(&self) -> BackupManifest {
        let manifests = self.manifest_files();
        assert_eq!(manifests.len(), 1, "one backup manifest should exist");
        let bytes = std::fs::read(&manifests[0]).expect("backup manifest bytes");
        serde_json::from_slice(&bytes).expect("valid backup manifest")
    }

    /// 创建包含 `keep.txt` 的恢复目标并返回其路径。
    ///
    /// # Panics
    ///
    /// 无法创建目录或哨兵时 panic。
    pub fn non_empty_target(&self) -> PathBuf {
        let target = self.root.path().join("restore-target");
        std::fs::create_dir(&target).expect("restore target");
        std::fs::write(target.join("keep.txt"), b"do not overwrite").expect("sentinel file");
        target
    }

    /// 创建并返回空恢复目标目录。
    ///
    /// # Panics
    ///
    /// 无法创建目录时 panic。
    pub fn empty_target(&self) -> PathBuf {
        let target = self.root.path().join("empty-restore-target");
        std::fs::create_dir(&target).expect("empty restore target");
        target
    }
}

fn read_files_with_extension(directory: &Path, extension: &str) -> Vec<PathBuf> {
    read_matching_files(directory, |path| {
        path.extension().is_some_and(|value| value == extension)
    })
}

fn read_matching_files(directory: &Path, predicate: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| predicate(path))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// 插入一条最小的可用收件箱记录，并返回其 UUID。
pub async fn seed_inbox(pool: &sqlx::SqlitePool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',x'2e','.',x'01',x'02','available',0,1,0,0)",
    )
    .bind(id.as_bytes().as_slice())
    .execute(pool)
    .await
    .expect("seed inbox");
    id
}

/// 插入系统拥有的 M3 处理任务所使用的单例管理员。
pub async fn seed_account(pool: &sqlx::SqlitePool) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(id.as_bytes().as_slice())
    .execute(pool)
    .await
    .expect("seed account");
    id
}

/// Seed one encrypted pending create-download automation event for store/worker tests.
pub async fn seed_automation_event(pool: &sqlx::SqlitePool, config_dir: &Path) -> Uuid {
    use mediaflow_core::automation::model::{AutomationSourceInput, WebhookAction};
    use mediaflow_core::automation::source_store::AutomationSourceStore;
    use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
    use mediaflow_core::connectors::downloader::model::{
        DownloaderConnectionInput, DownloaderKind,
    };
    use mediaflow_core::connectors::model::SecretString;
    use mediaflow_core::platform::secrets::{
        InstanceKey, IntegrationKind, SecretAad, SecretCipher as _,
    };
    use sha2::{Digest as _, Sha256};

    let downloader_id = Uuid::now_v7();
    DownloaderConnectionStore::open(pool.clone(), config_dir)
        .unwrap()
        .insert(
            downloader_id,
            DownloaderConnectionInput {
                kind: DownloaderKind::Qbittorrent,
                display_name: "Automation worker qBit".to_owned(),
                base_url: "https://download.example.test".to_owned(),
                username: SecretString::new(String::new()),
                password: SecretString::new(String::new()),
                enabled: true,
            },
            1,
        )
        .await
        .unwrap();
    let source_id = Uuid::now_v7();
    AutomationSourceStore::open(pool.clone(), config_dir)
        .unwrap()
        .insert(
            source_id,
            AutomationSourceInput::Webhook {
                display_name: "Worker webhook".to_owned(),
                enabled: true,
                allowed_actions: vec![WebhookAction::DownloadCreate],
            },
            Some(SecretString::new(
                "automation-worker-webhook-secret-01".to_owned(),
            )),
            1,
        )
        .await
        .unwrap();
    let event_id = Uuid::now_v7();
    let body = serde_json::json!({
        "kind":"download.create",
        "downloader_connection_id":downloader_id,
        "source":"magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
        "display_name":"Automation worker download"
    })
    .to_string()
    .into_bytes();
    let key = InstanceKey::load_or_create(config_dir).unwrap();
    let sealed = key
        .seal(
            &SecretAad::new(event_id, IntegrationKind::AutomationEventPayload, 1, 1),
            &body,
        )
        .unwrap();
    let digest: [u8; 32] = Sha256::digest(&body).into();
    sqlx::query(
        "INSERT INTO automation_events
         (id,source_id,source_display_name,source_config_version,action,dedup_key_sha256,
          request_sha256,payload_schema_version,payload_nonce,payload_ciphertext,status,
          downstream_kind,downstream_id,result_count,failure_code,attempt_count,retry_at_us,
          lease_token,lease_expires_at_us,projection_version,created_at_us,updated_at_us)
         VALUES (?,?,?,1,'create-download',?,?,?,?,?,'pending',NULL,NULL,NULL,NULL,0,NULL,NULL,NULL,1,1,1)",
    )
    .bind(event_id.as_bytes().as_slice())
    .bind(source_id.as_bytes().as_slice())
    .bind("Worker webhook")
    .bind(digest.as_slice())
    .bind(digest.as_slice())
    .bind(i64::from(sealed.schema_version()))
    .bind(sealed.nonce().as_slice())
    .bind(sealed.ciphertext())
    .execute(pool)
    .await
    .unwrap();
    event_id
}

/// 创建一个稳定 revision 及其持久化下游处理请求。
pub async fn seed_stable_revision(
    pool: &sqlx::SqlitePool,
    inbox: Uuid,
    path: &[u8],
    identity: Vec<u8>,
) -> Uuid {
    use mediaflow_core::discovery::observations::FileObservation;
    use mediaflow_core::discovery::revisions::{
        ObservationSource, RevisionObserver, RevisionService,
    };

    let observation = FileObservation {
        inbox_directory_id: inbox,
        relative_path_bytes: path.to_vec(),
        relative_path_display: String::from_utf8_lossy(path).into_owned(),
        identity_snapshot: identity,
        size_bytes: 100,
        modified_at_ns: 0,
    };
    let revisions = RevisionService::new(pool.clone());
    revisions
        .observe(observation.clone(), ObservationSource::Watcher, 60_000_000)
        .await
        .expect("first revision observation");
    revisions
        .observe(observation, ObservationSource::Reconcile, 90_000_000)
        .await
        .expect("stable revision observation")
        .revision_id
}

/// 为稳定 revision 创建并领取一个识别阶段处理任务。
pub async fn seed_processing_lease(
    pool: &sqlx::SqlitePool,
    inbox: Uuid,
    path: &[u8],
    identity: Vec<u8>,
    owner: &str,
) -> mediaflow_core::tasks::processing::model::ProcessingLease {
    use mediaflow_core::tasks::processing::model::ProcessingStage;
    use mediaflow_core::tasks::processing::store::ProcessingStore;

    let revision = seed_stable_revision(pool, inbox, path, identity).await;
    let store = ProcessingStore::new(pool.clone());
    store.ensure_revision(revision, 91_000_000).await.unwrap();
    store
        .claim_next(owner, &[ProcessingStage::Identification], 92_000_000)
        .await
        .unwrap()
        .unwrap()
}
