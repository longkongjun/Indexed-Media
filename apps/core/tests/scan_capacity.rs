#![allow(clippy::too_many_lines)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mediaflow_core::bootstrap::config::{AppConfig, RunMode};
use mediaflow_core::platform::db::open_pool;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::tasks::store::TaskStore;
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

const SAMPLE_COUNT: usize = 20;
const FILE_COUNT: usize = 100_000;
const TASK_COUNT: i64 = 1_000;

#[derive(Serialize)]
struct DatasetEvidence {
    file_count: usize,
    unfinished_task_count: i64,
    generator: &'static str,
}

#[derive(Serialize)]
struct RawSamples {
    idle_rss_mb: Vec<f64>,
    active_rss_mb: Vec<f64>,
    list_api_ms: Vec<f64>,
    write_ms: Vec<f64>,
    sse_ms: Vec<f64>,
    recovery_ms: Vec<f64>,
}

#[derive(Serialize)]
struct BenchmarkEvidence {
    hardware: String,
    os: String,
    filesystem: String,
    mount: String,
    dataset: DatasetEvidence,
    sample_count: usize,
    idle_rss_mb: f64,
    active_rss_mb: f64,
    list_api_p95_ms: f64,
    write_p95_ms: f64,
    sse_p95_ms: f64,
    recovery_ms: f64,
    command: &'static str,
    toolchain: String,
    recorded_at: String,
    environment: &'static str,
    raw_samples: RawSamples,
}

impl BenchmarkEvidence {
    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("hardware", self.hardware.as_str()),
            ("os", self.os.as_str()),
            ("filesystem", self.filesystem.as_str()),
            ("mount", self.mount.as_str()),
            ("dataset.generator", self.dataset.generator),
            ("command", self.command),
            ("toolchain", self.toolchain.as_str()),
            ("recorded_at", self.recorded_at.as_str()),
            ("environment", self.environment),
        ] {
            validate_metadata(name, value)?;
        }
        chrono::DateTime::parse_from_rfc3339(&self.recorded_at)
            .map_err(|error| format!("recorded_at must be RFC 3339: {error}"))?;
        if self.dataset.file_count != FILE_COUNT
            || self.dataset.unfinished_task_count != TASK_COUNT
            || self.dataset.generator != "m2-fixture real files and migrated SQLite"
        {
            return Err("dataset metadata must describe the exact M2 fixture".to_owned());
        }
        if self.sample_count < SAMPLE_COUNT {
            return Err(format!("sample_count must be at least {SAMPLE_COUNT}"));
        }
        for (name, samples) in [
            ("idle_rss_mb", self.raw_samples.idle_rss_mb.as_slice()),
            ("list_api_ms", self.raw_samples.list_api_ms.as_slice()),
            ("write_ms", self.raw_samples.write_ms.as_slice()),
            ("sse_ms", self.raw_samples.sse_ms.as_slice()),
        ] {
            if samples.len() != self.sample_count {
                return Err(format!(
                    "raw {name} requires exactly {} samples, got {}",
                    self.sample_count,
                    samples.len()
                ));
            }
            validate_samples(name, samples)?;
        }
        if self.raw_samples.active_rss_mb.is_empty() {
            return Err("raw active_rss_mb must not be empty".to_owned());
        }
        validate_samples("active_rss_mb", &self.raw_samples.active_rss_mb)?;
        if self.raw_samples.recovery_ms.len() != 1 {
            return Err(format!(
                "raw recovery_ms requires exactly one sample, got {}",
                self.raw_samples.recovery_ms.len()
            ));
        }
        validate_samples("recovery_ms", &self.raw_samples.recovery_ms)?;
        for (name, recorded, derived, budget) in [
            (
                "idle_rss_mb",
                self.idle_rss_mb,
                maximum(&self.raw_samples.idle_rss_mb),
                150.0,
            ),
            (
                "active_rss_mb",
                self.active_rss_mb,
                maximum(&self.raw_samples.active_rss_mb),
                300.0,
            ),
            (
                "list_api_p95_ms",
                self.list_api_p95_ms,
                percentile_95(&self.raw_samples.list_api_ms),
                200.0,
            ),
            (
                "write_p95_ms",
                self.write_p95_ms,
                percentile_95(&self.raw_samples.write_ms),
                500.0,
            ),
            (
                "sse_p95_ms",
                self.sse_p95_ms,
                percentile_95(&self.raw_samples.sse_ms),
                1_000.0,
            ),
            (
                "recovery_ms",
                self.recovery_ms,
                self.raw_samples.recovery_ms[0],
                30_000.0,
            ),
        ] {
            validate_derived_metric(name, recorded, derived, budget)?;
        }
        let provenance = self.environment.to_ascii_lowercase();
        if !(provenance.contains("host") || provenance.contains("container"))
            || !provenance.contains("nas")
            || !provenance.contains("ugreen")
        {
            return Err(
                "environment must explicitly state host/container, NAS, and UGREEN provenance"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

fn validate_metadata(name: &str, value: &str) -> Result<(), String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if [
        "unknown",
        "unavailable",
        "not available",
        "not measured",
        "placeholder",
        "tbd",
        "todo",
    ]
    .iter()
    .any(|sentinel| normalized.contains(sentinel))
        || matches!(normalized.as_str(), "n/a" | "na" | "none" | "null" | "-")
    {
        return Err(format!("{name} contains placeholder metadata"));
    }
    Ok(())
}

fn validate_samples(name: &str, samples: &[f64]) -> Result<(), String> {
    if let Some(value) = samples
        .iter()
        .copied()
        .find(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(format!("raw {name} contains invalid sample {value}"));
    }
    Ok(())
}

fn validate_metric(name: &str, value: f64, budget: f64) -> Result<(), String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{name} must be finite and positive, got {value}"));
    }
    if value > budget {
        return Err(format!("{name} measured {value:.3}, budget is {budget:.3}"));
    }
    Ok(())
}

fn validate_derived_metric(
    name: &str,
    recorded: f64,
    derived: f64,
    budget: f64,
) -> Result<(), String> {
    if recorded.to_bits() != derived.to_bits() {
        return Err(format!(
            "{name} must equal its raw-sample derivation: recorded {recorded}, derived {derived}"
        ));
    }
    validate_metric(name, derived, budget)
}

fn write_validated_evidence(output: &Path, evidence: &BenchmarkEvidence) -> Result<(), String> {
    publish_validated_evidence_with(
        output,
        evidence,
        std::io::Write::write_all,
        |temporary_file| temporary_file.as_file().sync_all(),
    )
}

fn publish_validated_evidence_with(
    output: &Path,
    evidence: &BenchmarkEvidence,
    write_temporary: impl FnOnce(&mut tempfile::NamedTempFile, &[u8]) -> std::io::Result<()>,
    sync_temporary: impl FnOnce(&tempfile::NamedTempFile) -> std::io::Result<()>,
) -> Result<(), String> {
    evidence.validate()?;
    let json = serde_json::to_vec_pretty(evidence).map_err(|error| error.to_string())?;
    let parent = output
        .parent()
        .ok_or_else(|| "benchmark success artifact requires a parent directory".to_owned())?;
    let mut temporary_file = tempfile::Builder::new()
        .prefix(".mediaflow-benchmark-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| format!("cannot create benchmark sibling temporary file: {error}"))?;
    write_temporary(&mut temporary_file, &json)
        .map_err(|error| format!("cannot write benchmark sibling temporary file: {error}"))?;
    sync_temporary(&temporary_file)
        .map_err(|error| format!("cannot sync benchmark sibling temporary file: {error}"))?;
    let published = temporary_file
        .persist_noclobber(output)
        .map_err(|error| format!("cannot publish benchmark success artifact: {error}"))?;
    if let Err(error) = std::fs::File::open(parent).and_then(|directory| directory.sync_all()) {
        drop(published);
        let cleanup = std::fs::remove_file(output);
        let _ = std::fs::File::open(parent).and_then(|directory| directory.sync_all());
        return match cleanup {
            Ok(()) => Err(format!("cannot sync benchmark artifact directory: {error}")),
            Err(cleanup_error) => Err(format!(
                "cannot sync benchmark artifact directory: {error}; cannot remove unpublished artifact: {cleanup_error}"
            )),
        };
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs the real 100,000-file M2 host benchmark"]
async fn m2_host_capacity_benchmark_records_real_samples_and_enforces_budgets() {
    let output = benchmark_output_path();
    let idle_temporary = tempfile::tempdir().unwrap();
    let idle_root = idle_temporary.path().join("idle-root");
    let idle_config = idle_temporary.path().join("idle-config");
    std::fs::create_dir(&idle_root).unwrap();
    std::fs::create_dir(&idle_config).unwrap();
    let idle_app = initialize_config(&idle_root, &idle_config).await;
    let idle_port = available_port();
    let idle_process = CoreProcess::spawn(&idle_app, idle_port);
    wait_ready(idle_port);
    let mut idle_samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        idle_samples.push(process_rss_mb(idle_process.pid()));
        std::thread::sleep(Duration::from_millis(10));
    }
    let idle_rss_mb = maximum(&idle_samples);
    drop(idle_process);

    let temporary = tempfile::tempdir().unwrap();
    let temporary_root = std::fs::canonicalize(temporary.path()).unwrap();
    let root = temporary_root.join("dataset");
    let config_dir = temporary_root.join("config");
    run_fixture(&root, &config_dir);
    assert_eq!(count_fixture_files(&root), FILE_COUNT);
    let pool = open_pool(&config_dir.join("mediaflow.db")).await.unwrap();
    let unfinished_before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks_scan_tasks
         WHERE status='running' AND lease_expires_at_us<=0",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unfinished_before, TASK_COUNT);

    let recovery_started = Instant::now();
    let reclaimed = TaskStore::new(pool.clone())
        .reclaim_expired(chrono::Utc::now().timestamp_micros())
        .await
        .unwrap();
    let recovery_ms = recovery_started.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(reclaimed, u64::try_from(TASK_COUNT).unwrap());
    let recovery_attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks_scan_attempts WHERE reason='recovery' AND status='queued'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recovery_attempts, TASK_COUNT);
    let (cookie, csrf) = seed_session(&pool).await;
    let task_ids = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT id FROM tasks_scan_tasks WHERE status='queued' ORDER BY id LIMIT 41",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|bytes| Uuid::from_slice(&bytes).unwrap())
    .collect::<Vec<_>>();
    assert_eq!(task_ids.len(), 41);
    let active_task = task_ids[0];
    let last_event_id: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(id),0) FROM platform_outbox_events")
            .fetch_one(&pool)
            .await
            .unwrap();

    let port = available_port();
    let app = benchmark_config(&root, &config_dir, port);
    let process = CoreProcess::spawn(&app, port);
    wait_ready(port);
    let mut active_samples = vec![process_rss_mb(process.pid())];
    let origin = format!("http://127.0.0.1:{port}");

    let warmup = http_request(
        port,
        "GET",
        "/api/v1/scan-tasks?limit=200",
        &[("Cookie", cookie.as_str())],
    );
    assert_eq!(warmup.status, 200);
    let mut list_samples = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let started = Instant::now();
        let response = http_request(
            port,
            "GET",
            "/api/v1/scan-tasks?limit=200",
            &[("Cookie", cookie.as_str())],
        );
        list_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(response.status, 200);
        active_samples.push(process_rss_mb(process.pid()));
    }

    let mut write_samples = Vec::with_capacity(SAMPLE_COUNT);
    for (sample, task_id) in task_ids[1..=SAMPLE_COUNT].iter().enumerate() {
        let path = format!("/api/v1/scan-tasks/{task_id}/cancel");
        let key = format!("benchmark-write-{sample}");
        let started = Instant::now();
        let response = http_request(
            port,
            "POST",
            &path,
            &[
                ("Cookie", cookie.as_str()),
                ("Origin", origin.as_str()),
                ("Sec-Fetch-Site", "same-origin"),
                ("X-CSRF-Token", csrf.as_str()),
                ("Idempotency-Key", key.as_str()),
            ],
        );
        write_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(response.status, 202, "{}", response.body);
        active_samples.push(process_rss_mb(process.pid()));
    }

    let mut sse = SseClient::connect(port, &cookie, &origin, last_event_id);
    let mut sse_samples = Vec::with_capacity(SAMPLE_COUNT);
    for (sample, task_id) in task_ids[SAMPLE_COUNT + 1..].iter().enumerate() {
        let path = format!("/api/v1/scan-tasks/{task_id}/cancel");
        let key = format!("benchmark-sse-{sample}");
        let started = Instant::now();
        let response = http_request(
            port,
            "POST",
            &path,
            &[
                ("Cookie", cookie.as_str()),
                ("Origin", origin.as_str()),
                ("Sec-Fetch-Site", "same-origin"),
                ("X-CSRF-Token", csrf.as_str()),
                ("Idempotency-Key", key.as_str()),
            ],
        );
        assert_eq!(response.status, 202, "{}", response.body);
        sse.wait_for_task(*task_id);
        sse_samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        active_samples.push(process_rss_mb(process.pid()));
    }
    drop(sse);

    tokio::time::timeout(Duration::from_mins(15), async {
        loop {
            let status: String =
                sqlx::query_scalar("SELECT status FROM tasks_scan_tasks WHERE id=?")
                    .bind(active_task.as_bytes().as_slice())
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            active_samples.push(process_rss_mb(process.pid()));
            if status == "completed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("one real 100,000-file scan must finish");
    let observations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM discovery_scan_file_observations o
         JOIN tasks_scan_tasks t ON t.scan_batch_id=o.scan_batch_id WHERE t.id=?",
    )
    .bind(active_task.as_bytes().as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(observations, i64::try_from(FILE_COUNT).unwrap());
    let active_rss_mb = maximum(&active_samples);
    drop(process);
    pool.close().await;

    let evidence = BenchmarkEvidence {
        hardware: hardware_label(),
        os: command_output("uname", &["-srvmp"]),
        filesystem: filesystem_label(&root),
        mount: mount_label(&root),
        dataset: DatasetEvidence {
            file_count: FILE_COUNT,
            unfinished_task_count: unfinished_before,
            generator: "m2-fixture real files and migrated SQLite",
        },
        sample_count: SAMPLE_COUNT,
        idle_rss_mb,
        active_rss_mb,
        list_api_p95_ms: percentile_95(&list_samples),
        write_p95_ms: percentile_95(&write_samples),
        sse_p95_ms: percentile_95(&sse_samples),
        recovery_ms,
        command: "just bench-m2",
        toolchain: format!(
            "{}; {}",
            command_output("rustc", &["--version"]),
            command_output("cargo", &["--version"])
        ),
        recorded_at: chrono::Utc::now().to_rfc3339(),
        environment: "host process; not a container, Linux/NAS, or UGREEN result",
        raw_samples: RawSamples {
            idle_rss_mb: idle_samples,
            active_rss_mb: active_samples,
            list_api_ms: list_samples,
            write_ms: write_samples,
            sse_ms: sse_samples,
            recovery_ms: vec![recovery_ms],
        },
    };
    write_validated_evidence(&output, &evidence).unwrap();

    assert!(evidence.sample_count >= SAMPLE_COUNT);
    assert_budget("idle_rss_mb", evidence.idle_rss_mb, 150.0);
    assert_budget("active_rss_mb", evidence.active_rss_mb, 300.0);
    assert_budget("list_api_p95_ms", evidence.list_api_p95_ms, 200.0);
    assert_budget("write_p95_ms", evidence.write_p95_ms, 500.0);
    assert_budget("sse_p95_ms", evidence.sse_p95_ms, 1_000.0);
    assert_budget("recovery_ms", evidence.recovery_ms, 30_000.0);
}

fn benchmark_output_path() -> PathBuf {
    let output = std::env::var_os("MEDIAFLOW_BENCH_OUTPUT")
        .map(PathBuf::from)
        .expect("MEDIAFLOW_BENCH_OUTPUT is required");
    assert!(
        output.is_absolute(),
        "benchmark output must be an absolute path"
    );
    assert!(output.parent().is_some_and(Path::is_dir));
    output
}

async fn initialize_config(root: &Path, config_dir: &Path) -> AppConfig {
    let root = std::fs::canonicalize(root).unwrap();
    let deployment_roots_file = config_dir.join("deployment-roots.json");
    std::fs::write(
        &deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Idle fixture","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let config = AppConfig {
        mode: RunMode::Development,
        listen: "127.0.0.1:0".parse().unwrap(),
        config_dir: config_dir.to_owned(),
        public_origin: Url::parse("http://127.0.0.1:3000").unwrap(),
        trusted_proxy_cidrs: Vec::new(),
        deployment_roots_file,
        web_dist: config_dir.join("web-dist"),
    };
    let db = migrate_with_backup(&config).await.unwrap();
    db.pool().close().await;
    config
}

fn benchmark_config(root: &Path, config_dir: &Path, port: u16) -> AppConfig {
    assert!(root.is_dir());
    AppConfig {
        mode: RunMode::Development,
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        config_dir: config_dir.to_owned(),
        public_origin: Url::parse(&format!("http://127.0.0.1:{port}")).unwrap(),
        trusted_proxy_cidrs: Vec::new(),
        deployment_roots_file: config_dir.join("deployment-roots.json"),
        web_dist: config_dir.join("web-dist"),
    }
}

fn run_fixture(root: &Path, config_dir: &Path) {
    let binary = std::env::var_os("CARGO_BIN_EXE_m2-fixture").expect("m2-fixture binary");
    let output = Command::new(binary)
        .arg("--root")
        .arg(root)
        .arg("--config-dir")
        .arg(config_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixture stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn seed_session(pool: &sqlx::SqlitePool) -> (String, String) {
    let account: Vec<u8> = sqlx::query_scalar("SELECT id FROM identity_accounts")
        .fetch_one(pool)
        .await
        .unwrap();
    let token = "benchmark-session-token";
    let csrf = "benchmark-csrf-token";
    let now = chrono::Utc::now().timestamp_micros();
    sqlx::query(
        "INSERT INTO identity_sessions
         (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,
          credential_version,last_used_at_us,created_at_us)
         VALUES (?,?,?,?,?,?,1,?,?)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account)
    .bind(Sha256::digest(token.as_bytes()).as_slice())
    .bind(Sha256::digest(csrf.as_bytes()).as_slice())
    .bind(now + 3_600_000_000_i64)
    .bind(now + 3_600_000_000_i64)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    (format!("__Host-mediaflow_session={token}"), csrf.to_owned())
}

struct CoreProcess(Child);

impl CoreProcess {
    fn spawn(config: &AppConfig, port: u16) -> Self {
        let binary =
            std::env::var_os("CARGO_BIN_EXE_mediaflow-core").expect("mediaflow-core binary");
        Self(
            Command::new(binary)
                .arg("serve")
                .env("MEDIAFLOW_MODE", "development")
                .env("MEDIAFLOW_LISTEN", format!("127.0.0.1:{port}"))
                .env("MEDIAFLOW_PUBLIC_ORIGIN", config.public_origin.as_str())
                .env("MEDIAFLOW_CONFIG_DIR", &config.config_dir)
                .env(
                    "MEDIAFLOW_DEPLOYMENT_ROOTS_FILE",
                    &config.deployment_roots_file,
                )
                .env("MEDIAFLOW_WEB_DIST", &config.web_dist)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }
}

impl Drop for CoreProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct HttpResponse {
    status: u16,
    body: String,
}

fn http_request(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> HttpResponse {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n"
    );
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    let response = String::from_utf8_lossy(&bytes);
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .unwrap();
    let body = response.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    HttpResponse {
        status,
        body: body.to_owned(),
    }
}

struct SseClient {
    stream: TcpStream,
    buffered: Vec<u8>,
}

impl SseClient {
    fn connect(port: u16, cookie: &str, origin: &str, last_event_id: i64) -> Self {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = format!(
            "GET /api/v1/events HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nCookie: {cookie}\r\nOrigin: {origin}\r\nSec-Fetch-Site: same-origin\r\nLast-Event-ID: {last_event_id}\r\nConnection: keep-alive\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).unwrap();
        let mut buffered = Vec::new();
        let mut chunk = [0_u8; 4_096];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0, "SSE connection closed during handshake");
            buffered.extend_from_slice(&chunk[..read]);
            if let Some(position) = find_bytes(&buffered, b"\r\n\r\n") {
                let status = String::from_utf8_lossy(&buffered[..position]);
                assert!(status.starts_with("HTTP/1.1 200"), "{status}");
                buffered.drain(..position + 4);
                break;
            }
        }
        Self { stream, buffered }
    }

    fn wait_for_task(&mut self, task_id: Uuid) {
        let needle = task_id.to_string();
        let mut chunk = [0_u8; 8_192];
        loop {
            if find_bytes(&self.buffered, needle.as_bytes()).is_some() {
                self.buffered.clear();
                return;
            }
            let read = self.stream.read(&mut chunk).unwrap();
            assert!(read > 0, "SSE connection closed before target event");
            self.buffered.extend_from_slice(&chunk[..read]);
            if self.buffered.len() > 2_000_000 {
                let keep_from = self.buffered.len() - 1_000_000;
                self.buffered.drain(..keep_from);
            }
        }
    }
}

fn wait_ready(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            let response = http_request(port, "GET", "/health/ready", &[]);
            if response.status == 200 {
                return;
            }
        }
        assert!(Instant::now() < deadline, "Core readiness timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn available_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[cfg(target_os = "linux")]
fn process_rss_mb(pid: u32) -> f64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let kb: f64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .expect("VmRSS");
    kb / 1_024.0
}

#[cfg(not(target_os = "linux"))]
fn process_rss_mb(pid: u32) -> f64 {
    let output = command_output("ps", &["-o", "rss=", "-p", &pid.to_string()]);
    output.trim().parse::<f64>().unwrap() / 1_024.0
}

fn hardware_label() -> String {
    #[cfg(target_os = "macos")]
    {
        return command_output("sysctl", &["-n", "machdep.cpu.brand_string"]);
    }
    #[cfg(target_os = "linux")]
    {
        return std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|contents| {
                contents.lines().find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.trim() == "model name")
                        .map(|(_, value)| value.trim().to_owned())
                })
            })
            .unwrap_or_else(|| "Linux CPU model unavailable".to_owned());
    }
    #[allow(unreachable_code)]
    command_output("uname", &["-m"])
}

fn filesystem_label(path: &Path) -> String {
    #[cfg(target_os = "linux")]
    {
        return command_output("stat", &["-f", "-c", "%T", path.to_str().unwrap()]);
    }
    #[cfg(target_os = "macos")]
    {
        let mount = mount_label(path);
        let prefix = format!(" on {mount} (");
        return command_output("mount", &[])
            .lines()
            .find_map(|line| {
                line.split_once(&prefix)
                    .and_then(|(_, details)| details.split(',').next())
                    .map(str::to_owned)
            })
            .expect("mounted filesystem type");
    }
    #[allow(unreachable_code)]
    "filesystem type unavailable".to_owned()
}

fn mount_label(path: &Path) -> String {
    let output = command_output("df", &["-P", path.to_str().unwrap()]);
    output
        .lines()
        .last()
        .and_then(|line| line.split_whitespace().last())
        .unwrap_or("unknown mount")
        .to_owned()
}

fn command_output(command: &str, args: &[&str]) -> String {
    let output = Command::new(command).args(args).output().unwrap();
    assert!(output.status.success(), "{command} failed");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn count_fixture_files(root: &Path) -> usize {
    let mut count = 0;
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else if entry.file_name() != ".mediaflow-m2-fixture.json" {
                count += 1;
            }
        }
    }
    count
}

fn percentile_95(samples: &[f64]) -> f64 {
    assert!(samples.len() >= SAMPLE_COUNT);
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (sorted.len() * 95).div_ceil(100);
    sorted[rank - 1]
}

fn maximum(samples: &[f64]) -> f64 {
    samples.iter().copied().reduce(f64::max).unwrap()
}

fn assert_budget(name: &str, measured: f64, maximum: f64) {
    assert!(
        measured.is_finite() && measured > 0.0,
        "{name} must be measured, got {measured}"
    );
    assert!(
        measured <= maximum,
        "{name} measured {measured:.3}, budget is {maximum:.3}"
    );
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod evidence_validation_tests {
    use super::{
        BenchmarkEvidence, DatasetEvidence, RawSamples, SAMPLE_COUNT,
        publish_validated_evidence_with, write_validated_evidence,
    };

    fn valid_evidence() -> BenchmarkEvidence {
        BenchmarkEvidence {
            hardware: "Test CPU".to_owned(),
            os: "Test OS 1.0".to_owned(),
            filesystem: "apfs".to_owned(),
            mount: "/tmp".to_owned(),
            dataset: DatasetEvidence {
                file_count: 100_000,
                unfinished_task_count: 1_000,
                generator: "m2-fixture real files and migrated SQLite",
            },
            sample_count: SAMPLE_COUNT,
            idle_rss_mb: 1.0,
            active_rss_mb: 2.0,
            list_api_p95_ms: 3.0,
            write_p95_ms: 4.0,
            sse_p95_ms: 5.0,
            recovery_ms: 6.0,
            command: "just bench-m2",
            toolchain: "rustc test; cargo test".to_owned(),
            recorded_at: "2026-07-18T00:00:00Z".to_owned(),
            environment: "host process; not a container, Linux/NAS, or UGREEN result",
            raw_samples: RawSamples {
                idle_rss_mb: vec![1.0; SAMPLE_COUNT],
                active_rss_mb: vec![2.0],
                list_api_ms: vec![3.0; SAMPLE_COUNT],
                write_ms: vec![4.0; SAMPLE_COUNT],
                sse_ms: vec![5.0; SAMPLE_COUNT],
                recovery_ms: vec![6.0],
            },
        }
    }

    #[test]
    fn rejects_empty_required_metadata() {
        let mut evidence = valid_evidence();
        evidence.hardware = "   ".to_owned();
        assert!(evidence.validate().unwrap_err().contains("hardware"));
    }

    #[test]
    fn rejects_placeholder_metadata() {
        for (field, value) in [
            ("hardware", "Linux CPU model unavailable"),
            ("filesystem", "filesystem type unavailable"),
            ("mount", "unknown mount"),
        ] {
            let mut evidence = valid_evidence();
            match field {
                "hardware" => evidence.hardware = value.to_owned(),
                "filesystem" => evidence.filesystem = value.to_owned(),
                "mount" => evidence.mount = value.to_owned(),
                _ => unreachable!(),
            }
            assert!(evidence.validate().unwrap_err().contains(field));
        }
    }

    #[test]
    fn rejects_inexact_dataset_and_undersized_samples() {
        let mut evidence = valid_evidence();
        evidence.dataset.file_count -= 1;
        assert!(evidence.validate().unwrap_err().contains("dataset"));

        let mut evidence = valid_evidence();
        evidence.sample_count -= 1;
        assert!(evidence.validate().unwrap_err().contains("sample_count"));

        let mut evidence = valid_evidence();
        evidence.raw_samples.list_api_ms.pop();
        assert!(evidence.validate().unwrap_err().contains("list_api_ms"));
    }

    #[test]
    fn rejects_zero_metric_and_partial_raw_artifact() {
        let mut evidence = valid_evidence();
        evidence.write_p95_ms = 0.0;
        assert!(evidence.validate().unwrap_err().contains("write_p95_ms"));

        let mut evidence = valid_evidence();
        evidence.raw_samples.active_rss_mb.clear();
        assert!(evidence.validate().unwrap_err().contains("active_rss_mb"));

        let mut evidence = valid_evidence();
        evidence.raw_samples.recovery_ms.clear();
        assert!(evidence.validate().unwrap_err().contains("recovery_ms"));
    }

    #[test]
    fn rejects_underreported_over_budget_raw_metric_and_wrong_maximum() {
        let mut evidence = valid_evidence();
        evidence.raw_samples.active_rss_mb = vec![2.0, 500.0];
        assert!(evidence.validate().unwrap_err().contains("active_rss_mb"));

        let mut evidence = valid_evidence();
        evidence.raw_samples.idle_rss_mb[0] = 2.0;
        assert!(evidence.validate().unwrap_err().contains("idle_rss_mb"));

        let mut evidence = valid_evidence();
        evidence.active_rss_mb = 3.0;
        assert!(evidence.validate().unwrap_err().contains("active_rss_mb"));
    }

    #[test]
    fn rejects_wrong_nearest_rank_p95_even_when_both_values_are_under_budget() {
        let mut evidence = valid_evidence();
        evidence.raw_samples.list_api_ms = (1..=SAMPLE_COUNT)
            .map(|value| f64::from(u32::try_from(value).unwrap()))
            .collect();
        evidence.list_api_p95_ms = 18.0;
        assert!(evidence.validate().unwrap_err().contains("list_api_p95_ms"));
    }

    #[test]
    fn rejects_multiple_or_mismatched_recovery_samples() {
        let mut evidence = valid_evidence();
        evidence.raw_samples.recovery_ms.push(6.0);
        assert!(evidence.validate().unwrap_err().contains("recovery_ms"));

        let mut evidence = valid_evidence();
        evidence.raw_samples.recovery_ms[0] = 7.0;
        assert!(evidence.validate().unwrap_err().contains("recovery_ms"));
    }

    #[test]
    fn invalid_or_stale_evidence_path_never_becomes_a_success_artifact() {
        let temporary = tempfile::tempdir().unwrap();
        let output = temporary.path().join("benchmark.json");
        let mut invalid = valid_evidence();
        invalid.environment = "host process";
        assert!(
            write_validated_evidence(&output, &invalid)
                .unwrap_err()
                .contains("environment")
        );
        assert!(!output.exists(), "invalid evidence must not be written");

        std::fs::write(&output, b"stale artifact").unwrap();
        assert!(write_validated_evidence(&output, &valid_evidence()).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"stale artifact");
        assert_eq!(
            std::fs::read_dir(temporary.path()).unwrap().count(),
            1,
            "failed no-clobber publish must remove its sibling temporary file"
        );
    }

    #[test]
    fn writer_and_sync_failures_leave_no_final_or_sibling_temporary_file() {
        use std::io::{Error, Write as _};

        for fail_during_sync in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let output = temporary.path().join("benchmark.json");
            let result = publish_validated_evidence_with(
                &output,
                &valid_evidence(),
                |temporary_file, json| {
                    temporary_file.write_all(&json[..json.len().min(8)])?;
                    if fail_during_sync {
                        temporary_file.write_all(&json[json.len().min(8)..])
                    } else {
                        Err(Error::other("injected write failure"))
                    }
                },
                |_| {
                    if fail_during_sync {
                        Err(Error::other("injected sync failure"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            assert!(!output.exists());
            assert_eq!(
                std::fs::read_dir(temporary.path()).unwrap().count(),
                0,
                "failed writer must clean its sibling temporary file"
            );
        }
    }
}
