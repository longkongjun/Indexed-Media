#![allow(clippy::too_many_lines)]

mod common;

use std::io::Read as _;
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};
use std::thread::JoinHandle;
use std::time::Duration;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::DiscoveryUseCases;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::CreateInboxCommand;
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::db::open_pool;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::tasks::model::NewScanTask;
use mediaflow_core::tasks::store::TaskStore;
use uuid::Uuid;

fn fixture_binary() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_m2-fixture")
        .map(Into::into)
        .expect("Cargo must expose the m2-fixture binary to integration tests")
}

fn run_fixture(root: &Path, config_dir: &Path, replace: bool) -> Output {
    let mut command = Command::new(fixture_binary());
    command
        .arg("--root")
        .arg(root)
        .arg("--config-dir")
        .arg(config_dir);
    if replace {
        command.arg("--replace");
    }
    command.output().expect("m2 fixture process")
}

#[tokio::test]
async fn independent_core_kill_and_restart_recovers_same_task_and_batch_once() {
    const FILES: usize = 15_000;
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("kill-root");
    std::fs::create_dir(&root).unwrap();
    for index in 0..FILES {
        std::fs::write(root.join(format!("file-{index:05}.media")), b"x").unwrap();
    }
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Kill fixture","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let roots = DeploymentRootSet::load(
        &fixture.config().deployment_roots_file,
        RunMode::Development,
    )
    .unwrap();
    let fs: std::sync::Arc<dyn CapabilityFs> = std::sync::Arc::new(
        OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap(),
    );
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    let task = TaskStore::new(db.pool().clone())
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox.id,
            idempotency_key: "kill-restart".to_owned(),
            now_us: 1,
        })
        .await
        .unwrap();
    let original_batch: Vec<u8> =
        sqlx::query_scalar("SELECT scan_batch_id FROM tasks_scan_tasks WHERE id=?")
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    db.pool().close().await;

    let mut first = spawn_core(&fixture);
    let observer = open_pool(&fixture.database_path()).await.unwrap();
    let first_batch_committed = wait_for_count(
        &observer,
        "SELECT COUNT(*) FROM discovery_scan_file_observations",
        500,
        Duration::from_secs(15),
    )
    .await;
    if !first_batch_committed {
        first.reap().unwrap();
        panic!(
            "Core must commit the expected batch before timeout; stderr: {}",
            first.diagnostic_stderr()
        );
    }
    first.reap().unwrap();
    let first_stderr = first.diagnostic_stderr();
    let partial: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM discovery_scan_file_observations")
        .fetch_one(&observer)
        .await
        .unwrap();
    assert!(
        partial >= 500 && partial < i64::try_from(FILES).unwrap(),
        "first Core stderr: {first_stderr}"
    );
    sqlx::query("UPDATE tasks_scan_tasks SET lease_expires_at_us=0 WHERE id=?")
        .bind(task.id.as_bytes().as_slice())
        .execute(&observer)
        .await
        .unwrap();

    let mut second = spawn_core(&fixture);
    wait_for_status(&observer, task.id, "completed", Duration::from_secs(30)).await;
    second.reap().unwrap();
    let second_stderr = second.diagnostic_stderr();

    let recovered: (Vec<u8>, i64, i64, i64) = sqlx::query_as(
        "SELECT scan_batch_id,
          (SELECT COUNT(*) FROM tasks_scan_attempts a WHERE a.task_id=t.id AND a.reason='recovery'),
          (SELECT COUNT(*) FROM discovery_scan_file_observations o WHERE o.scan_batch_id=t.scan_batch_id),
          (SELECT COUNT(DISTINCT discovered_file_id) FROM discovery_scan_file_observations o
             WHERE o.scan_batch_id=t.scan_batch_id)
         FROM tasks_scan_tasks t WHERE id=?",
    )
    .bind(task.id.as_bytes().as_slice())
    .fetch_one(&observer)
    .await
    .unwrap();
    observer.close().await;
    assert_eq!(
        recovered.0, original_batch,
        "second Core stderr: {second_stderr}"
    );
    assert_eq!(recovered.1, 1, "restart creates one recovery attempt");
    assert_eq!(recovered.2, i64::try_from(FILES).unwrap());
    assert_eq!(recovered.3, recovered.2, "observations remain unique");
}

fn spawn_core(fixture: &common::TestConfigDir) -> CoreChild {
    let binary = std::env::var_os("CARGO_BIN_EXE_mediaflow-core")
        .expect("Cargo must expose mediaflow-core to integration tests");
    let mut command = Command::new(binary);
    command
        .arg("serve")
        .env("MEDIAFLOW_MODE", "development")
        .env("MEDIAFLOW_LISTEN", "127.0.0.1:0")
        .env("MEDIAFLOW_PUBLIC_ORIGIN", "http://127.0.0.1:3000")
        .env("MEDIAFLOW_CONFIG_DIR", &fixture.config().config_dir)
        .env(
            "MEDIAFLOW_DEPLOYMENT_ROOTS_FILE",
            &fixture.config().deployment_roots_file,
        )
        .env("MEDIAFLOW_WEB_DIST", &fixture.config().web_dist)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    CoreChild::spawn_command(&mut command).unwrap()
}

const STDERR_DIAGNOSTIC_LIMIT: usize = 16 * 1_024;

struct CoreChild {
    child: Option<Child>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
    stderr: Vec<u8>,
}

impl CoreChild {
    fn spawn_command(command: &mut Command) -> std::io::Result<Self> {
        let mut child = command.stderr(Stdio::piped()).spawn()?;
        let mut stderr = child
            .stderr
            .take()
            .expect("piped child stderr must be available");
        let stderr_reader = std::thread::spawn(move || {
            let mut diagnostic = Vec::with_capacity(STDERR_DIAGNOSTIC_LIMIT);
            let mut buffer = [0_u8; 4_096];
            loop {
                match stderr.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let remaining = STDERR_DIAGNOSTIC_LIMIT.saturating_sub(diagnostic.len());
                        diagnostic.extend_from_slice(&buffer[..read.min(remaining)]);
                    }
                }
            }
            diagnostic
        });
        Ok(Self {
            child: Some(child),
            stderr_reader: Some(stderr_reader),
            stderr: Vec::new(),
        })
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().expect("child has been reaped").id()
    }

    fn reap(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let mut child = self.child.take().expect("child must only be reaped once");
        let _ = child.kill();
        let status = child.wait();
        if let Some(reader) = self.stderr_reader.take() {
            self.stderr = reader.join().unwrap_or_default();
        }
        status
    }

    fn diagnostic_stderr(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

impl Drop for CoreChild {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = self.reap();
        }
    }
}

#[cfg(unix)]
#[test]
fn core_child_guard_reaps_process_when_scope_unwinds() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let pid = Arc::new(AtomicU32::new(0));
    let unwind_pid = Arc::clone(&pid);
    let result = std::panic::catch_unwind(move || {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 30"]);
        let child = CoreChild::spawn_command(&mut command).unwrap();
        unwind_pid.store(child.pid(), Ordering::SeqCst);
        panic!("exercise RAII cleanup");
    });
    assert!(result.is_err());
    let pid = pid.load(Ordering::SeqCst);
    assert_ne!(pid, 0);
    let status = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success(), "child {pid} survived scope unwind");
}

async fn wait_for_count(
    pool: &sqlx::SqlitePool,
    query: &str,
    minimum: i64,
    timeout: Duration,
) -> bool {
    tokio::time::timeout(timeout, async {
        loop {
            let count: i64 = sqlx::query_scalar(query).fetch_one(pool).await.unwrap();
            if count >= minimum {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .is_ok()
}

async fn wait_for_status(
    pool: &sqlx::SqlitePool,
    task_id: Uuid,
    expected: &str,
    timeout: Duration,
) {
    tokio::time::timeout(timeout, async {
        loop {
            let status: String =
                sqlx::query_scalar("SELECT status FROM tasks_scan_tasks WHERE id=?")
                    .bind(task_id.as_bytes().as_slice())
                    .fetch_one(pool)
                    .await
                    .unwrap();
            if status == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Core restart must reach the expected terminal status");
}

#[test]
fn fixture_rejects_overlapping_targets_before_writing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("dataset");
    let config_dir = root.join("config");

    let output = run_fixture(&root, &config_dir, false);

    assert!(!output.status.success());
    assert!(!root.exists(), "invalid targets must have no side effects");
}

#[test]
fn fixture_rolls_back_case_alias_materialization_before_writing() {
    let temporary = tempfile::tempdir().unwrap();
    let temporary_root = std::fs::canonicalize(temporary.path()).unwrap();
    let probe = temporary_root.join("CaseProbe");
    std::fs::create_dir(&probe).unwrap();
    let case_insensitive = temporary_root.join("caseprobe").is_dir();
    std::fs::remove_dir(&probe).unwrap();
    if !case_insensitive {
        eprintln!("skipped: test filesystem is case-sensitive");
        return;
    }
    let root = temporary_root.join("FreshFixtureAlias");
    let config_dir = temporary_root.join("freshfixturealias");

    let output = run_fixture(&root, &config_dir, false);

    assert!(!output.status.success());
    assert!(
        !root.exists(),
        "only invocation-created directories are rolled back"
    );
    assert!(!config_dir.join(".mediaflow-m2-fixture.json").exists());
    assert!(!config_dir.join("mediaflow.db").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn fixture_rejects_bind_mount_aliases_before_writing_under_explicit_harness() {
    use std::os::unix::fs::MetadataExt as _;

    let (Some(root), Some(config_dir)) = (
        std::env::var_os("MEDIAFLOW_TEST_BIND_ALIAS_ROOT").map(std::path::PathBuf::from),
        std::env::var_os("MEDIAFLOW_TEST_BIND_ALIAS_CONFIG").map(std::path::PathBuf::from),
    ) else {
        eprintln!(
            "skipped: set MEDIAFLOW_TEST_BIND_ALIAS_ROOT and MEDIAFLOW_TEST_BIND_ALIAS_CONFIG \
             to two empty bind mounts of the same directory; mount setup requires Linux permission"
        );
        return;
    };
    assert!(root.is_absolute() && config_dir.is_absolute());
    assert_ne!(
        std::fs::canonicalize(&root).unwrap(),
        std::fs::canonicalize(&config_dir).unwrap()
    );
    let root_metadata = std::fs::metadata(&root).unwrap();
    let config_metadata = std::fs::metadata(&config_dir).unwrap();
    assert_eq!(
        (root_metadata.dev(), root_metadata.ino()),
        (config_metadata.dev(), config_metadata.ino()),
        "Linux harness paths must be aliases of the same directory"
    );
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);

    let output = run_fixture(&root, &config_dir, false);

    assert!(!output.status.success());
    assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
    assert!(!root.join(".mediaflow-m2-fixture.json").exists());
    assert!(!root.join("mediaflow.db").exists());
}

#[tokio::test]
#[ignore = "creates the real M2 100,000-file/1,000-task capacity fixture"]
async fn fixture_creates_exact_dataset_and_requires_explicit_safe_replacement() {
    let temporary = tempfile::tempdir().unwrap();
    let temporary_root = std::fs::canonicalize(temporary.path()).unwrap();
    let root = temporary_root.join("dataset");
    let config_dir = temporary_root.join("config");

    let first = run_fixture(&root, &config_dir, false);
    assert!(
        first.status.success(),
        "fixture stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(count_fixture_files(&root), 100_000);

    let pool = open_pool(&config_dir.join("mediaflow.db")).await.unwrap();
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks_scan_tasks
         WHERE status IN ('queued','running') AND lease_expires_at_us < 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks_scan_attempts WHERE reason='initial' AND status='running'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    pool.close().await;
    assert_eq!(unfinished, 1_000);
    assert_eq!(attempts, 1_000);

    let implicit = run_fixture(&root, &config_dir, false);
    assert!(!implicit.status.success(), "repeat must require --replace");
    assert_eq!(count_fixture_files(&root), 100_000);

    let unrelated = temporary_root.join("unrelated");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("keep.txt"), b"owned by caller").unwrap();
    let rejected = run_fixture(&unrelated, &config_dir, true);
    assert!(!rejected.status.success());
    assert_eq!(
        std::fs::read(unrelated.join("keep.txt")).unwrap(),
        b"owned by caller"
    );

    let replaced = run_fixture(&root, &config_dir, true);
    assert!(
        replaced.status.success(),
        "explicit safe replacement stderr: {}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(count_fixture_files(&root), 100_000);
}

fn count_fixture_files(root: &Path) -> usize {
    let mut count = 0;
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(entry.path());
            } else if entry.file_name() != ".mediaflow-m2-fixture.json" {
                count += 1;
            }
        }
    }
    count
}
