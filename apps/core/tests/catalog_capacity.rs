#![allow(clippy::too_many_lines)]

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use mediaflow_core::catalog::model::{LocalStatus, MediaItemFilter, MediaItemKind};
use mediaflow_core::catalog::store::CatalogStore;
use mediaflow_core::platform::db::open_pool;
use mediaflow_core::shared::page::PageRequest;
use serde::Serialize;
use sqlx::Row as _;
use uuid::Uuid;

const REFERENCE_ITEMS: i64 = 50_000;
const SAMPLE_COUNT: usize = 20;

fn fixture_binary() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_m3-catalog-fixture")
        .map(Into::into)
        .expect("Cargo must expose the m3-catalog-fixture binary")
}

fn run_fixture(database: &Path, items: usize, seed: u64) -> Output {
    Command::new(fixture_binary())
        .arg("--database")
        .arg(database)
        .arg("--items")
        .arg(items.to_string())
        .arg("--seed")
        .arg(seed.to_string())
        .output()
        .expect("M3 Catalog fixture process")
}

#[tokio::test]
async fn deterministic_fixture_is_indexed_and_every_store_page_remains_bounded() {
    let temporary = tempfile::tempdir().unwrap();
    let database = temporary.path().join("catalog/mediaflow.db");
    let output = run_fixture(&database, 1_200, 17);
    assert!(
        output.status.success(),
        "fixture stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["item_count"], 1_200);
    assert_eq!(summary["movie_count"], 400);
    assert_eq!(summary["series_count"], 400);
    assert_eq!(summary["generic_video_count"], 400);
    assert_eq!(summary["seed"], 17);

    let pool = open_pool(&database).await.unwrap();
    let account = account_id(&pool).await;
    let store = CatalogStore::new(pool.clone());
    let mut cursor = None;
    let mut ids = HashSet::new();
    loop {
        let page = store
            .list(
                account,
                &MediaItemFilter::default(),
                &PageRequest::new(cursor.take(), Some(200)).unwrap(),
            )
            .await
            .unwrap();
        assert!(page.items.len() <= 200);
        ids.extend(page.items.into_iter().map(|item| item.id));
        let Some(next) = page.next_cursor else {
            break;
        };
        cursor = Some(next);
    }
    assert_eq!(ids.len(), 1_200);
    let filtered = store
        .list(
            account,
            &MediaItemFilter {
                kind: Some(MediaItemKind::Movie),
                library_id: None,
                local_status: Some(LocalStatus::Complete),
                query: Some("Fixture Movie 00".to_owned()),
            },
            &PageRequest::new(None, Some(25)).unwrap(),
        )
        .await
        .unwrap();
    assert!(!filtered.items.is_empty());
    assert!(filtered.items.len() <= 25);
    assert!(filtered.items.iter().all(|item| {
        item.kind == MediaItemKind::Movie && item.local_status == LocalStatus::Complete
    }));

    let plans = query_plans(&pool).await;
    for (plan, index) in plans.iter().zip([
        "catalog_order_snapshot_idx",
        "catalog_media_type_list_idx",
        "catalog_media_title_idx",
        "sqlite_autoindex_catalog_media_items_1",
    ]) {
        assert!(plan.contains(index), "missing {index}: {plan}");
    }
    pool.close().await;

    let second = tempfile::tempdir().unwrap();
    let second_database = second.path().join("catalog/mediaflow.db");
    let second_output = run_fixture(&second_database, 12, 17);
    assert!(second_output.status.success());
    let second_pool = open_pool(&second_database).await.unwrap();
    assert_eq!(account, account_id(&second_pool).await);
}

#[derive(Serialize)]
struct BenchmarkEvidence {
    hardware: String,
    os: String,
    filesystem: String,
    toolchain: String,
    command: &'static str,
    recorded_at: String,
    environment: &'static str,
    dataset_items: i64,
    database_size_mb: f64,
    rss_mb: f64,
    all_list_p95_ms: f64,
    filtered_list_p95_ms: f64,
    detail_p95_ms: f64,
    query_plans: Vec<String>,
    raw_samples: RawSamples,
}

#[derive(Serialize)]
#[allow(clippy::struct_field_names)]
struct RawSamples {
    all_list_ms: Vec<f64>,
    filtered_list_ms: Vec<f64>,
    detail_ms: Vec<f64>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs the real 50,000-item M3 Catalog host benchmark"]
async fn m3_catalog_host_benchmark_records_actual_bounds_without_fabricating_a_budget_pass() {
    let database = required_existing_path("MEDIAFLOW_CATALOG_BENCH_DATABASE");
    let output = required_new_path("MEDIAFLOW_BENCHMARK_OUTPUT");
    let pool = open_pool(&database).await.unwrap();
    let account = account_id(&pool).await;
    let item_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM catalog_media_items")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(item_count, REFERENCE_ITEMS);
    let store = CatalogStore::new(pool.clone());
    let page = PageRequest::new(None, Some(200)).unwrap();
    let filtered = MediaItemFilter {
        kind: Some(MediaItemKind::Movie),
        library_id: None,
        local_status: Some(LocalStatus::Complete),
        query: Some("Fixture Movie 4".to_owned()),
    };
    let first = store
        .list(account, &MediaItemFilter::default(), &page)
        .await
        .unwrap();
    let detail_id = first.items[0].id;
    store.detail(account, detail_id).await.unwrap();
    let all_list_ms = samples(|| async {
        store
            .list(account, &MediaItemFilter::default(), &page)
            .await
            .unwrap();
    })
    .await;
    let filtered_list_ms = samples(|| async {
        store.list(account, &filtered, &page).await.unwrap();
    })
    .await;
    let detail_ms = samples(|| async {
        store.detail(account, detail_id).await.unwrap();
    })
    .await;
    let plans = query_plans(&pool).await;
    let evidence = BenchmarkEvidence {
        hardware: command_output("uname", &["-m"]),
        os: command_output("uname", &["-a"]),
        filesystem: command_output("df", &["-P", database.to_str().unwrap()]),
        toolchain: command_output("rustc", &["--version"]),
        command: "MEDIAFLOW_BENCHMARK_OUTPUT=<new-json> just bench-m3-catalog",
        recorded_at: chrono::Utc::now().to_rfc3339(),
        environment: "local host benchmark; target NAS and browser rendering are excluded",
        dataset_items: item_count,
        database_size_mb: f64::from(
            u32::try_from(std::fs::metadata(&database).unwrap().len()).unwrap(),
        ) / 1_048_576.0,
        rss_mb: process_rss_mb(),
        all_list_p95_ms: percentile_95(&all_list_ms),
        filtered_list_p95_ms: percentile_95(&filtered_list_ms),
        detail_p95_ms: percentile_95(&detail_ms),
        query_plans: plans,
        raw_samples: RawSamples {
            all_list_ms,
            filtered_list_ms,
            detail_ms,
        },
    };
    assert_eq!(evidence.dataset_items, REFERENCE_ITEMS);
    for value in [
        evidence.database_size_mb,
        evidence.rss_mb,
        evidence.all_list_p95_ms,
        evidence.filtered_list_p95_ms,
        evidence.detail_p95_ms,
    ] {
        assert!(value.is_finite() && value > 0.0);
    }
    publish(&output, &evidence);
}

async fn query_plans(pool: &sqlx::SqlitePool) -> Vec<String> {
    let queries = [
        "EXPLAIN QUERY PLAN SELECT MAX(revision) FROM catalog_media_order_history
         WHERE account_id=x'00000000000000000000000000000000'",
        "EXPLAIN QUERY PLAN SELECT id FROM catalog_media_items
         WHERE account_id=x'00000000000000000000000000000000' AND kind='movie'
         ORDER BY updated_at_us DESC,id DESC LIMIT 200",
        "EXPLAIN QUERY PLAN SELECT id FROM catalog_media_items
         INDEXED BY catalog_media_title_idx
         WHERE account_id=x'00000000000000000000000000000000'
           AND title_normalized LIKE 'fixture movie%'
         ORDER BY title_normalized,updated_at_us DESC,id DESC LIMIT 200",
        "EXPLAIN QUERY PLAN SELECT * FROM catalog_media_items
         WHERE id=x'00000000000000000000000000000000'",
    ];
    let mut plans = Vec::new();
    for query in queries {
        plans.push(
            sqlx::query(query)
                .fetch_all(pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.get::<String, _>("detail"))
                .collect::<Vec<_>>()
                .join(" | "),
        );
    }
    plans
}

async fn account_id(pool: &sqlx::SqlitePool) -> Uuid {
    let bytes: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM identity_accounts WHERE singleton_key=1")
            .fetch_one(pool)
            .await
            .unwrap();
    Uuid::from_slice(&bytes).unwrap()
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

fn percentile_95(samples: &[f64]) -> f64 {
    let mut values = samples.to_vec();
    values.sort_by(f64::total_cmp);
    values[(values.len() * 95).div_ceil(100).saturating_sub(1)]
}

fn required_existing_path(name: &str) -> PathBuf {
    let path =
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is required")));
    assert!(path.is_absolute() && path.is_file());
    path
}

fn required_new_path(name: &str) -> PathBuf {
    let path =
        PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} is required")));
    assert!(path.is_absolute());
    assert!(path.parent().is_some_and(Path::is_dir));
    assert!(!path.exists());
    path
}

fn publish(output: &Path, evidence: &BenchmarkEvidence) {
    let parent = output.parent().unwrap();
    let mut temporary = tempfile::Builder::new()
        .prefix(".mediaflow-m3-catalog-")
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
    let output = Command::new(command).args(args).output().unwrap();
    assert!(output.status.success());
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
