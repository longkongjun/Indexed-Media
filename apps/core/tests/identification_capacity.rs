#![allow(clippy::too_many_lines)]

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use mediaflow_core::identification::review::{ReviewCaseFilter, ReviewCaseStore};
use mediaflow_core::platform::db::open_pool;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::processing::store::ProcessingStore;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::Row as _;
use uuid::Uuid;

const DEFAULT_REVISION_COUNT: usize = 100_000;
const DEFAULT_CANDIDATE_COUNT: usize = 50_000;
const DEFAULT_UNFINISHED_COUNT: usize = 1_000;
const SAMPLE_COUNT: usize = 20;

fn fixture_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_m3-identification-fixture")
        .map(Into::into)
        .expect("Cargo must expose the m3-identification-fixture binary")
}

fn run_fixture(
    root: &Path,
    config_dir: &Path,
    revisions: usize,
    candidates: usize,
    unfinished: usize,
    seed: u64,
) -> Output {
    Command::new(fixture_binary())
        .arg("--root")
        .arg(root)
        .arg("--config-dir")
        .arg(config_dir)
        .arg("--revision-count")
        .arg(revisions.to_string())
        .arg("--candidate-count")
        .arg(candidates.to_string())
        .arg("--unfinished-task-count")
        .arg(unfinished.to_string())
        .arg("--seed")
        .arg(seed.to_string())
        .output()
        .expect("M3 fixture process")
}

#[tokio::test]
async fn fixture_is_deterministic_indexed_paginated_and_recovers_one_thousand_tasks() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    let config = temporary.path().join("config");
    let output = run_fixture(&root, &config, 1_200, 200, 1_000, 7);
    assert!(
        output.status.success(),
        "fixture stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["revision_count"], 1_200);
    assert_eq!(summary["candidate_count"], 200);
    assert_eq!(summary["unfinished_task_count"], 1_000);
    assert_eq!(summary["review_case_count"], 10);
    assert_eq!(summary["seed"], 7);

    let pool = open_pool(&config.join("mediaflow.db")).await.unwrap();
    for (table, expected) in [
        ("discovery_file_revisions", 1_200_i64),
        ("identification_candidates", 200),
        ("identification_review_cases", 10),
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, expected, "fixture count for {table}");
    }
    let unfinished: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks_processing_tasks
         WHERE status='running' AND lease_expires_at_us=0",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unfinished, 1_000);
    let stages = sqlx::query(
        "SELECT stage,COUNT(*) AS count FROM tasks_processing_tasks
         WHERE status='running' GROUP BY stage ORDER BY stage",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(stages.len(), 5);
    assert!(stages.iter().all(|row| row.get::<i64, _>("count") == 200));

    assert_plan_uses(
        &pool,
        "EXPLAIN QUERY PLAN SELECT id FROM tasks_processing_tasks
         WHERE status='paused' AND stage='identification' AND next_retry_at_us<=1
         ORDER BY next_retry_at_us,id LIMIT 1",
        "tasks_processing_due_retry_idx",
    )
    .await;
    assert_plan_uses(
        &pool,
        "EXPLAIN QUERY PLAN SELECT MAX(revision)
         FROM tasks_processing_task_order_history WHERE account_id=x'00'",
        "tasks_processing_order_snapshot_idx",
    )
    .await;
    assert_plan_uses(
        &pool,
        "EXPLAIN QUERY PLAN SELECT MAX(revision)
         FROM identification_review_case_order_history WHERE account_id=x'00'",
        "identification_review_snapshot_idx",
    )
    .await;

    let recovery_started = Instant::now();
    let recovered = ProcessingStore::new(pool.clone())
        .reclaim_expired(10_000_000)
        .await
        .unwrap();
    assert_eq!(recovered, 1_000);
    assert!(recovery_started.elapsed().as_secs_f64() < 30.0);
    let recovery_attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks_processing_attempts
         WHERE reason='lease-recovery' AND status='queued'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recovery_attempts, 1_000);

    let account = account_id(&pool).await;
    let tasks = ProcessingStore::new(pool.clone());
    let mut task_cursor = None;
    let mut task_ids = HashSet::new();
    loop {
        let page = tasks
            .list_tasks(
                account,
                &PageRequest::new(task_cursor.take(), Some(200)).unwrap(),
            )
            .await
            .unwrap();
        task_ids.extend(page.items.into_iter().map(|task| task.id));
        let Some(cursor) = page.next_cursor else {
            break;
        };
        task_cursor = Some(cursor);
    }
    assert_eq!(task_ids.len(), 1_010);

    let reviews = ReviewCaseStore::new(pool.clone());
    let mut review_cursor = None;
    let mut review_ids = HashSet::new();
    loop {
        let page = reviews
            .list_active(
                account,
                &ReviewCaseFilter::default(),
                &PageRequest::new(review_cursor.take(), Some(7)).unwrap(),
            )
            .await
            .unwrap();
        review_ids.extend(page.items.into_iter().map(|case| case.id));
        let Some(cursor) = page.next_cursor else {
            break;
        };
        review_cursor = Some(cursor);
    }
    assert_eq!(review_ids.len(), 10);
    pool.close().await;

    let second = tempfile::tempdir().unwrap();
    let second_output = run_fixture(
        &second.path().join("root"),
        &second.path().join("config"),
        20,
        20,
        10,
        7,
    );
    assert!(second_output.status.success());
    let second_pool = open_pool(&second.path().join("config/mediaflow.db"))
        .await
        .unwrap();
    assert_eq!(account, account_id(&second_pool).await);
}

async fn assert_plan_uses(pool: &sqlx::SqlitePool, sql: &str, index: &str) {
    let details = sqlx::query(sql)
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        details.contains(index),
        "query plan did not use {index}:\n{details}"
    );
}

async fn account_id(pool: &sqlx::SqlitePool) -> Uuid {
    let bytes: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM identity_accounts WHERE singleton_key=1")
            .fetch_one(pool)
            .await
            .unwrap();
    Uuid::from_slice(&bytes).unwrap()
}

#[derive(Serialize)]
struct BenchmarkEvidence {
    hardware: String,
    os: String,
    filesystem: String,
    rust_profile: &'static str,
    toolchain: String,
    command: &'static str,
    recorded_at: String,
    environment: &'static str,
    external_provider_latency_included: bool,
    dataset: DatasetEvidence,
    fixture_generation_ms: f64,
    database_size_mb: f64,
    rss_mb: f64,
    processing_list_p95_ms: f64,
    review_list_p95_ms: f64,
    cache_hit_p95_ms: f64,
    cache_miss_p95_ms: f64,
    recovery_ms: f64,
    query_plans: Vec<String>,
    raw_samples: RawSamples,
}

#[derive(Serialize)]
struct DatasetEvidence {
    revision_count: usize,
    candidate_count: usize,
    unfinished_task_count: usize,
    review_case_count: i64,
    seed: u64,
}

#[derive(Serialize)]
#[allow(clippy::struct_field_names)]
struct RawSamples {
    processing_list_ms: Vec<f64>,
    review_list_ms: Vec<f64>,
    cache_hit_ms: Vec<f64>,
    cache_miss_ms: Vec<f64>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs the real 100k revision / 50k candidate / 1000 unfinished M3 host benchmark"]
async fn m3_identification_host_benchmark_records_real_samples_and_enforces_budgets() {
    let output = benchmark_output_path();
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("root");
    let config = temporary.path().join("config");
    let fixture_started = Instant::now();
    let fixture = run_fixture(
        &root,
        &config,
        DEFAULT_REVISION_COUNT,
        DEFAULT_CANDIDATE_COUNT,
        DEFAULT_UNFINISHED_COUNT,
        3,
    );
    assert!(
        fixture.status.success(),
        "fixture stderr: {}",
        String::from_utf8_lossy(&fixture.stderr)
    );
    let fixture_generation_ms = fixture_started.elapsed().as_secs_f64() * 1_000.0;
    let pool = open_pool(&config.join("mediaflow.db")).await.unwrap();
    let account = account_id(&pool).await;
    let review_case_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM identification_review_cases")
            .fetch_one(&pool)
            .await
            .unwrap();
    let tasks = ProcessingStore::new(pool.clone());
    let reviews = ReviewCaseStore::new(pool.clone());
    let page = PageRequest::new(None, Some(200)).unwrap();
    tasks.list_tasks(account, &page).await.unwrap();
    reviews
        .list_active(account, &ReviewCaseFilter::default(), &page)
        .await
        .unwrap();
    let processing_list_ms = samples(|| async {
        tasks.list_tasks(account, &page).await.unwrap();
    })
    .await;
    let review_list_ms = samples(|| async {
        reviews
            .list_active(account, &ReviewCaseFilter::default(), &page)
            .await
            .unwrap();
    })
    .await;

    let cache_key: [u8; 32] = Sha256::digest(b"m3-benchmark-cache-hit").into();
    sqlx::query(
        "INSERT INTO connectors_tmdb_cache
         (query_key,provider_schema_version,outcome,response_json,fresh_until_us,stale_until_us,
          created_at_us,updated_at_us) VALUES (?,1,'found',x'5b5d',9,10,1,1)",
    )
    .bind(cache_key.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let missing_key: [u8; 32] = Sha256::digest(b"m3-benchmark-cache-miss").into();
    let cache_hit_ms = sql_samples(&pool, &cache_key).await;
    let cache_miss_ms = sql_samples(&pool, &missing_key).await;

    let recovery_started = Instant::now();
    let recovered = ProcessingStore::new(pool.clone())
        .reclaim_expired(10_000_000)
        .await
        .unwrap();
    let recovery_ms = recovery_started.elapsed().as_secs_f64() * 1_000.0;
    assert_eq!(recovered, DEFAULT_UNFINISHED_COUNT as u64);
    let query_plans = vec![
        explain(
            &pool,
            "EXPLAIN QUERY PLAN SELECT id FROM tasks_processing_tasks
             WHERE status='paused' AND stage='identification' AND next_retry_at_us<=1
             ORDER BY next_retry_at_us,id LIMIT 1",
        )
        .await,
        explain(
            &pool,
            "EXPLAIN QUERY PLAN SELECT MAX(revision)
             FROM tasks_processing_task_order_history WHERE account_id=x'00'",
        )
        .await,
        explain(
            &pool,
            "EXPLAIN QUERY PLAN SELECT MAX(revision)
             FROM identification_review_case_order_history WHERE account_id=x'00'",
        )
        .await,
    ];
    for (plan, index) in query_plans.iter().zip([
        "tasks_processing_due_retry_idx",
        "tasks_processing_order_snapshot_idx",
        "identification_review_snapshot_idx",
    ]) {
        assert!(plan.contains(index), "missing {index}: {plan}");
    }
    let evidence = BenchmarkEvidence {
        hardware: command_output("uname", &["-m"]),
        os: command_output("uname", &["-a"]),
        filesystem: command_output("df", &["-P", config.to_str().unwrap()]),
        rust_profile: "release",
        toolchain: command_output("rustc", &["--version"]),
        command: "MEDIAFLOW_M3_BENCH_OUTPUT=<absolute-json> just bench-m3-identification",
        recorded_at: chrono::Utc::now().to_rfc3339(),
        environment: "local host benchmark; real TMDB and target NAS are excluded",
        external_provider_latency_included: false,
        dataset: DatasetEvidence {
            revision_count: DEFAULT_REVISION_COUNT,
            candidate_count: DEFAULT_CANDIDATE_COUNT,
            unfinished_task_count: DEFAULT_UNFINISHED_COUNT,
            review_case_count,
            seed: 3,
        },
        fixture_generation_ms,
        database_size_mb: f64::from(
            u32::try_from(
                std::fs::metadata(config.join("mediaflow.db"))
                    .unwrap()
                    .len(),
            )
            .unwrap(),
        ) / 1_048_576.0,
        rss_mb: process_rss_mb(),
        processing_list_p95_ms: percentile_95(&processing_list_ms),
        review_list_p95_ms: percentile_95(&review_list_ms),
        cache_hit_p95_ms: percentile_95(&cache_hit_ms),
        cache_miss_p95_ms: percentile_95(&cache_miss_ms),
        recovery_ms,
        query_plans,
        raw_samples: RawSamples {
            processing_list_ms,
            review_list_ms,
            cache_hit_ms,
            cache_miss_ms,
        },
    };
    validate_benchmark(&evidence);
    publish(&output, &evidence);
}

async fn samples<F, Fut>(mut operation: F) -> Vec<f64>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut values = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let started = Instant::now();
        operation().await;
        values.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    values
}

async fn sql_samples(pool: &sqlx::SqlitePool, key: &[u8; 32]) -> Vec<f64> {
    samples(|| async {
        let _: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT response_json FROM connectors_tmdb_cache WHERE query_key=?")
                .bind(key.as_slice())
                .fetch_optional(pool)
                .await
                .unwrap()
                .flatten();
    })
    .await
}

async fn explain(pool: &sqlx::SqlitePool, sql: &str) -> String {
    sqlx::query(sql)
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join(" | ")
}

fn percentile_95(samples: &[f64]) -> f64 {
    let mut values = samples.to_vec();
    values.sort_by(f64::total_cmp);
    values[(values.len() * 95).div_ceil(100).saturating_sub(1)]
}

fn validate_benchmark(evidence: &BenchmarkEvidence) {
    assert_eq!(evidence.dataset.revision_count, DEFAULT_REVISION_COUNT);
    assert_eq!(evidence.dataset.candidate_count, DEFAULT_CANDIDATE_COUNT);
    assert_eq!(
        evidence.dataset.unfinished_task_count,
        DEFAULT_UNFINISHED_COUNT
    );
    assert_eq!(evidence.dataset.review_case_count, 2_500);
    for (name, value, budget) in [
        (
            "fixture_generation_ms",
            evidence.fixture_generation_ms,
            120_000.0,
        ),
        ("database_size_mb", evidence.database_size_mb, 1_024.0),
        ("rss_mb", evidence.rss_mb, 700.0),
        (
            "processing_list_p95_ms",
            evidence.processing_list_p95_ms,
            250.0,
        ),
        ("review_list_p95_ms", evidence.review_list_p95_ms, 250.0),
        ("cache_hit_p95_ms", evidence.cache_hit_p95_ms, 25.0),
        ("cache_miss_p95_ms", evidence.cache_miss_p95_ms, 25.0),
        ("recovery_ms", evidence.recovery_ms, 30_000.0),
    ] {
        assert!(value.is_finite() && value > 0.0, "invalid {name}: {value}");
        assert!(
            value <= budget,
            "{name} measured {value:.3}, budget {budget:.3}"
        );
    }
    for values in [
        &evidence.raw_samples.processing_list_ms,
        &evidence.raw_samples.review_list_ms,
        &evidence.raw_samples.cache_hit_ms,
        &evidence.raw_samples.cache_miss_ms,
    ] {
        assert_eq!(values.len(), SAMPLE_COUNT);
        assert!(values.iter().all(|value| value.is_finite() && *value > 0.0));
    }
}

fn benchmark_output_path() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("MEDIAFLOW_M3_BENCH_OUTPUT")
            .expect("MEDIAFLOW_M3_BENCH_OUTPUT is required"),
    );
    assert!(path.is_absolute());
    assert!(path.parent().is_some_and(Path::is_dir));
    assert!(!path.exists(), "benchmark artifact must not already exist");
    path
}

fn publish(output: &Path, evidence: &BenchmarkEvidence) {
    let parent = output.parent().unwrap();
    let mut temporary = tempfile::Builder::new()
        .prefix(".mediaflow-m3-benchmark-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .unwrap();
    temporary
        .write_all(&serde_json::to_vec_pretty(evidence).unwrap())
        .unwrap();
    temporary.as_file().sync_all().unwrap();
    temporary.persist_noclobber(output).unwrap();
    std::fs::File::open(parent).unwrap().sync_all().unwrap();
}

fn command_output(command: &str, args: &[&str]) -> String {
    let output = Command::new(command)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("cannot run benchmark metadata command {command}: {error}"));
    assert!(
        output.status.success(),
        "benchmark metadata command failed: {command}"
    );
    let value = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    assert!(!value.is_empty());
    value
}

fn process_rss_mb() -> f64 {
    let pid = std::process::id().to_string();
    let kib: f64 = command_output("ps", &["-o", "rss=", "-p", &pid])
        .trim()
        .parse()
        .unwrap();
    kib / 1_024.0
}
