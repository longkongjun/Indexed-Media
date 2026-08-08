use std::path::{Path, PathBuf};

use clap::Parser;
use mediaflow_core::bootstrap::config::{AppConfig, RunMode};
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::{Sqlite, Transaction};
use url::Url;
use uuid::Uuid;

const DEFAULT_ITEMS: usize = 50_000;
const MAX_ITEMS: usize = 100_000;

#[derive(Debug, Parser)]
#[command(name = "m3-catalog-fixture")]
struct Cli {
    #[arg(long)]
    database: PathBuf,
    #[arg(long, default_value_t = DEFAULT_ITEMS)]
    items: usize,
    #[arg(long, default_value_t = 11)]
    seed: u64,
}

#[derive(Serialize)]
struct FixtureSummary {
    item_count: usize,
    movie_count: usize,
    series_count: usize,
    generic_video_count: usize,
    library_count: usize,
    seed: u64,
    account_id: Uuid,
}

#[tokio::main]
async fn main() {
    if let Err(message) = run(&Cli::parse()).await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

async fn run(cli: &Cli) -> Result<(), String> {
    if cli.items == 0 || cli.items > MAX_ITEMS {
        return Err(format!("items must be in 1..={MAX_ITEMS}"));
    }
    if cli.database.file_name().and_then(|value| value.to_str()) != Some("mediaflow.db") {
        return Err("database must end with mediaflow.db".to_owned());
    }
    let config_dir = cli
        .database
        .parent()
        .ok_or_else(|| "database parent is missing".to_owned())?;
    prepare_empty_directory(config_dir)?;
    let root = config_dir.join("fixture-root");
    std::fs::create_dir(&root).map_err(|error| error.to_string())?;
    let deployment_roots_file = config_dir.join("deployment-roots.json");
    std::fs::write(
        &deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming",
            "label":"M3 Catalog capacity fixture",
            "container_path":root,
            "access":"read-only"
        }]}))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let config = AppConfig {
        mode: RunMode::Development,
        listen: "127.0.0.1:0"
            .parse()
            .map_err(|error| format!("invalid listen address: {error}"))?,
        config_dir: config_dir.to_owned(),
        public_origin: Url::parse("http://127.0.0.1:3000").map_err(|error| error.to_string())?,
        trusted_proxy_cidrs: Vec::new(),
        deployment_roots_file,
        web_dist: config_dir.join("web-dist"),
    };
    let db = migrate_with_backup(&config)
        .await
        .map_err(|error| error.safe_message().to_owned())?;
    let summary = seed_database(db.pool(), cli.items, cli.seed).await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(db.pool())
        .await
        .map_err(|error| error.to_string())?;
    db.pool().close().await;
    println!(
        "{}",
        serde_json::to_string(&summary).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn prepare_empty_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("database parent must be a real directory".to_owned());
            }
            if std::fs::read_dir(path)
                .map_err(|error| error.to_string())?
                .next()
                .is_some()
            {
                return Err("database parent must be empty".to_owned());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path).map_err(|error| error.to_string())?;
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn seed_database(
    pool: &sqlx::SqlitePool,
    item_count: usize,
    seed: u64,
) -> Result<FixtureSummary, String> {
    let account_id = deterministic_id(seed, b"account", 0);
    let inbox_id = deterministic_id(seed, b"inbox", 0);
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'catalog-fixture','Catalog Fixture','fixture-only',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',x'2e','.',?,?,'available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .bind(digest(seed, b"root", 0).as_slice())
    .bind(digest(seed, b"directory", 0).as_slice())
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO discovery_inbox_policies
         (inbox_directory_id,minimum_age_seconds,stable_observation_interval_seconds,
          reconcile_interval_seconds,watcher_enabled,config_version,created_at_us,updated_at_us)
         VALUES (?,60,30,900,1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO discovery_watch_states
         (inbox_directory_id,health,watcher_active,next_reconcile_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'healthy',1,900000000,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(&mut *tx)
    .await
    .map_err(|error| error.to_string())?;

    for index in 0..item_count {
        insert_item(&mut tx, account_id, inbox_id, seed, index).await?;
        if index > 0 && index % 2_000 == 0 {
            tx.commit().await.map_err(|error| error.to_string())?;
            tx = pool.begin().await.map_err(|error| error.to_string())?;
        }
    }
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(FixtureSummary {
        item_count,
        movie_count: item_count.div_ceil(3),
        series_count: (item_count + 1) / 3,
        generic_video_count: item_count / 3,
        library_count: 8,
        seed,
        account_id,
    })
}

#[allow(clippy::too_many_lines)]
async fn insert_item(
    tx: &mut Transaction<'_, Sqlite>,
    account_id: Uuid,
    inbox_id: Uuid,
    seed: u64,
    index: usize,
) -> Result<(), String> {
    let tracked_id = deterministic_id(seed, b"tracked", index);
    let revision_id = deterministic_id(seed, b"revision", index);
    let task_id = deterministic_id(seed, b"task", index);
    let attempt_id = deterministic_id(seed, b"attempt", index);
    let media_id = deterministic_id(seed, b"media", index);
    let version_id = deterministic_id(seed, b"version", index);
    let asset_id = deterministic_id(seed, b"asset", index);
    let result_id = deterministic_id(seed, b"result", index);
    let library_id = deterministic_id(seed, b"library", index % 8);
    let updated_at = i64::try_from(index + 1).map_err(|error| error.to_string())?;
    let path = format!("fixture/media-{index:05}.mkv");
    sqlx::query(
        "INSERT INTO discovery_tracked_files
         (id,inbox_directory_id,relative_path_bytes,relative_path_display,current_revision_id,
          last_observed_at_us,version,created_at_us,updated_at_us)
         VALUES (?,?,?,?,?,?,1,?,?)",
    )
    .bind(tracked_id.as_bytes().as_slice())
    .bind(inbox_id.as_bytes().as_slice())
    .bind(path.as_bytes())
    .bind(&path)
    .bind(revision_id.as_bytes().as_slice())
    .bind(updated_at)
    .bind(updated_at)
    .bind(updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO discovery_file_revisions
         (id,tracked_file_id,identity_snapshot,size_bytes,modified_at_ns,policy_version,
          minimum_age_seconds,stable_observation_interval_seconds,created_at_us)
         VALUES (?,?,?,?,?,1,60,30,?)",
    )
    .bind(revision_id.as_bytes().as_slice())
    .bind(tracked_id.as_bytes().as_slice())
    .bind(digest(seed, b"file-identity", index).as_slice())
    .bind(1_000_i64 + i64::try_from(index % 10_000).map_err(|error| error.to_string())?)
    .bind(updated_at)
    .bind(updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO tasks_processing_tasks
         (id,account_id,discovered_file_id,file_revision_id,inbox_directory_id,current_attempt_id,
          status,stage,checkpoint,reason,recovering,attempt_count,cancel_requested,
          config_snapshot_json,version,created_at_us,updated_at_us)
         VALUES (?,?,?,?,?,?,'cancelled','completion','cancelled',NULL,0,1,0,'{}',1,?,?)",
    )
    .bind(task_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(tracked_id.as_bytes().as_slice())
    .bind(revision_id.as_bytes().as_slice())
    .bind(inbox_id.as_bytes().as_slice())
    .bind(attempt_id.as_bytes().as_slice())
    .bind(updated_at)
    .bind(updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO tasks_processing_attempts
         (id,task_id,reason,ordinal,status,stage,finished_at_us,created_at_us)
         VALUES (?,?,'initial',1,'cancelled','completion',?,?)",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .bind(updated_at)
    .bind(updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    let (kind, title) = match index % 3 {
        0 => ("movie", format!("Fixture Movie {index:05}")),
        1 => ("series", format!("Fixture Series {index:05}")),
        _ => ("generic-video", format!("Fixture Clip Group {index:05}")),
    };
    let local_status = if index.is_multiple_of(2) {
        "complete"
    } else {
        "partial"
    };
    sqlx::query(
        "INSERT INTO catalog_media_items
         (id,account_id,library_id,kind,title,title_normalized,year,local_status,nfo_status,
          projection_version,created_at_us,updated_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,1,?,?)",
    )
    .bind(media_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(library_id.as_bytes().as_slice())
    .bind(kind)
    .bind(&title)
    .bind(title.to_lowercase())
    .bind((!index.is_multiple_of(5)).then_some(1980_i64 + i64::try_from(index % 45).unwrap()))
    .bind(local_status)
    .bind(if index.is_multiple_of(7) {
        "partial"
    } else {
        "complete"
    })
    .bind(updated_at)
    .bind(updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    let owner_node_id = if kind == "movie" {
        None
    } else if kind == "series" {
        let season_id = deterministic_id(seed, b"season", index);
        let node_id = deterministic_id(seed, b"node", index);
        sqlx::query(
            "INSERT INTO catalog_media_nodes
             (id,media_item_id,parent_id,kind,title,ordinal)
             VALUES (?,?,NULL,'season','Season 1',1)",
        )
        .bind(season_id.as_bytes().as_slice())
        .bind(media_id.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO catalog_media_nodes
             (id,media_item_id,parent_id,kind,title,ordinal)
             VALUES (?,?,?,'episode','Episode 1',1)",
        )
        .bind(node_id.as_bytes().as_slice())
        .bind(media_id.as_bytes().as_slice())
        .bind(season_id.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
        Some(node_id)
    } else {
        let node_id = deterministic_id(seed, b"node", index);
        sqlx::query(
            "INSERT INTO catalog_media_nodes
             (id,media_item_id,parent_id,kind,title,ordinal)
             VALUES (?,?,NULL,'generic-video-item','Clip 1',0)",
        )
        .bind(node_id.as_bytes().as_slice())
        .bind(media_id.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
        Some(node_id)
    };
    sqlx::query(
        "INSERT INTO catalog_media_versions(id,media_item_id,owner_node_id,label)
         VALUES (?,?,?,NULL)",
    )
    .bind(version_id.as_bytes().as_slice())
    .bind(media_id.as_bytes().as_slice())
    .bind(owner_node_id.map(|id| id.as_bytes().to_vec()))
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO catalog_file_assets
         (id,account_id,media_item_id,file_revision_id,source_relative_path,
          current_relative_path,size_bytes) VALUES (?,?,?,?,?,?,?)",
    )
    .bind(asset_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(media_id.as_bytes().as_slice())
    .bind(revision_id.as_bytes().as_slice())
    .bind(&path)
    .bind(&path)
    .bind(1_000_i64 + i64::try_from(index % 10_000).map_err(|error| error.to_string())?)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO catalog_version_files(media_item_id,version_id,file_asset_id,ordinal)
         VALUES (?,?,?,0)",
    )
    .bind(media_id.as_bytes().as_slice())
    .bind(version_id.as_bytes().as_slice())
    .bind(asset_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    if index.is_multiple_of(4) {
        let artwork_id = deterministic_id(seed, b"artwork", index);
        sqlx::query(
            "INSERT INTO catalog_artwork_refs
             (id,media_item_id,kind,state,local_relative_path)
             VALUES (?,?,'poster','available',?)",
        )
        .bind(artwork_id.as_bytes().as_slice())
        .bind(media_id.as_bytes().as_slice())
        .bind(format!("artwork/{index:05}.jpg"))
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    }
    sqlx::query(
        "INSERT INTO catalog_applied_local_results
         (result_id,account_id,task_id,media_item_id,request_sha256,applied_at_us)
         VALUES (?,?,?,?,?,?)",
    )
    .bind(result_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .bind(media_id.as_bytes().as_slice())
    .bind(digest(seed, b"verified-result", index).as_slice())
    .bind(updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query("UPDATE catalog_media_items SET updated_at_us=? WHERE id=?")
        .bind(updated_at)
        .bind(media_id.as_bytes().as_slice())
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn deterministic_id(seed: u64, domain: &[u8], index: usize) -> Uuid {
    let mut bytes: [u8; 16] = digest(seed, domain, index)[..16].try_into().unwrap();
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn digest(seed: u64, domain: &[u8], index: usize) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.m3-catalog-fixture.v1\0");
    hasher.update(seed.to_be_bytes());
    hasher.update(domain);
    hasher.update(index.to_be_bytes());
    hasher.finalize().into()
}
