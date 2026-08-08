use std::path::{Path, PathBuf};

use clap::Parser;
use mediaflow_core::bootstrap::config::{AppConfig, RunMode};
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use sqlx::{Sqlite, Transaction};
use url::Url;
use uuid::Uuid;

const DEFAULT_REVISION_COUNT: usize = 100_000;
const DEFAULT_CANDIDATE_COUNT: usize = 50_000;
const DEFAULT_UNFINISHED_TASK_COUNT: usize = 1_000;
const MAX_REVISION_COUNT: usize = 1_000_000;
const MAX_CANDIDATE_COUNT: usize = 1_000_000;
const MAX_UNFINISHED_TASK_COUNT: usize = 100_000;
const CANDIDATES_PER_TASK: usize = 20;

#[derive(Debug, Parser)]
#[command(name = "m3-identification-fixture")]
struct Cli {
    #[arg(long)]
    root: PathBuf,
    #[arg(long)]
    config_dir: PathBuf,
    #[arg(long, default_value_t = DEFAULT_REVISION_COUNT)]
    revision_count: usize,
    #[arg(long, default_value_t = DEFAULT_CANDIDATE_COUNT)]
    candidate_count: usize,
    #[arg(long, default_value_t = DEFAULT_UNFINISHED_TASK_COUNT)]
    unfinished_task_count: usize,
    #[arg(long, default_value_t = 3)]
    seed: u64,
}

#[derive(Serialize)]
struct FixtureSummary {
    revision_count: usize,
    candidate_count: usize,
    unfinished_task_count: usize,
    review_case_count: usize,
    processing_task_count: usize,
    stage_count: usize,
    seed: u64,
    account_id: Uuid,
    inbox_directory_id: Uuid,
}

#[tokio::main]
async fn main() {
    if let Err(message) = run(&Cli::parse()).await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

async fn run(cli: &Cli) -> Result<(), String> {
    validate_counts(cli)?;
    prepare_empty_directory(&cli.root, "fixture root")?;
    prepare_empty_directory(&cli.config_dir, "fixture config directory")?;
    let root = std::fs::canonicalize(&cli.root)
        .map_err(|error| format!("cannot resolve fixture root: {error}"))?;
    let config_dir = std::fs::canonicalize(&cli.config_dir)
        .map_err(|error| format!("cannot resolve fixture config directory: {error}"))?;
    if root.starts_with(&config_dir) || config_dir.starts_with(&root) {
        return Err("fixture root and config directory must not overlap".to_owned());
    }
    let deployment_roots_file = config_dir.join("deployment-roots.json");
    std::fs::write(
        &deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming",
            "label":"M3 identification benchmark fixture",
            "container_path":root,
            "access":"read-only"
        }]}))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write fixture deployment roots: {error}"))?;
    let config = AppConfig {
        mode: RunMode::Development,
        listen: "127.0.0.1:0"
            .parse()
            .map_err(|error| format!("invalid fixture listen address: {error}"))?,
        config_dir: config_dir.clone(),
        public_origin: Url::parse("http://127.0.0.1:3000").map_err(|error| error.to_string())?,
        trusted_proxy_cidrs: Vec::new(),
        deployment_roots_file,
        web_dist: config_dir.join("web-dist"),
    };
    let db = migrate_with_backup(&config)
        .await
        .map_err(|error| error.safe_message().to_owned())?;
    let summary = seed_database(db.pool(), cli).await?;
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

fn validate_counts(cli: &Cli) -> Result<(), String> {
    if cli.revision_count == 0 || cli.revision_count > MAX_REVISION_COUNT {
        return Err(format!(
            "revision count must be in 1..={MAX_REVISION_COUNT}"
        ));
    }
    if cli.candidate_count > MAX_CANDIDATE_COUNT {
        return Err(format!(
            "candidate count must be at most {MAX_CANDIDATE_COUNT}"
        ));
    }
    if cli.unfinished_task_count > MAX_UNFINISHED_TASK_COUNT {
        return Err(format!(
            "unfinished task count must be at most {MAX_UNFINISHED_TASK_COUNT}"
        ));
    }
    let candidate_tasks = cli.candidate_count.div_ceil(CANDIDATES_PER_TASK);
    let required_revisions = candidate_tasks
        .checked_add(cli.unfinished_task_count)
        .ok_or_else(|| "fixture task count overflow".to_owned())?;
    if required_revisions > cli.revision_count {
        return Err(format!(
            "revision count must cover {required_revisions} generated processing tasks"
        ));
    }
    Ok(())
}

fn prepare_empty_directory(path: &Path, label: &str) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(format!("{label} must be a real directory"));
            }
            if std::fs::read_dir(path)
                .map_err(|error| format!("cannot inspect {label}: {error}"))?
                .next()
                .is_some()
            {
                return Err(format!("{label} must be empty"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path).map_err(|error| format!("cannot create {label}: {error}"))?;
        }
        Err(error) => return Err(format!("cannot inspect {label}: {error}")),
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn seed_database(pool: &sqlx::SqlitePool, cli: &Cli) -> Result<FixtureSummary, String> {
    let account_id = deterministic_id(cli.seed, b"account", 0);
    let inbox_id = deterministic_id(cli.seed, b"inbox", 0);
    let candidate_task_count = cli.candidate_count.div_ceil(CANDIDATES_PER_TASK);
    let processing_task_count = candidate_task_count + cli.unfinished_task_count;
    let tracked_file_count = processing_task_count.max(1);
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'m3-fixture','M3 Fixture','fixture-only',1,1)",
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
    .bind(digest(cli.seed, b"root-identity", 0).as_slice())
    .bind(digest(cli.seed, b"directory-identity", 0).as_slice())
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

    for index in 0..tracked_file_count {
        let tracked_id = deterministic_id(cli.seed, b"tracked-file", index);
        let revision_id = deterministic_id(cli.seed, b"revision", index);
        let path = format!("fixture/media-{index:06}.mkv");
        sqlx::query(
            "INSERT INTO discovery_tracked_files
             (id,inbox_directory_id,relative_path_bytes,relative_path_display,current_revision_id,
              last_observed_at_us,version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,1,1,1,1)",
        )
        .bind(tracked_id.as_bytes().as_slice())
        .bind(inbox_id.as_bytes().as_slice())
        .bind(path.as_bytes())
        .bind(&path)
        .bind(revision_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    }
    for index in 0..cli.revision_count {
        let tracked_index = index % tracked_file_count;
        let tracked_id = deterministic_id(cli.seed, b"tracked-file", tracked_index);
        let revision_id = deterministic_id(cli.seed, b"revision", index);
        let created_at = i64::try_from(index + 1).map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO discovery_file_revisions
             (id,tracked_file_id,identity_snapshot,size_bytes,modified_at_ns,policy_version,
              minimum_age_seconds,stable_observation_interval_seconds,created_at_us)
             VALUES (?,?,?,?,?,1,60,30,?)",
        )
        .bind(revision_id.as_bytes().as_slice())
        .bind(tracked_id.as_bytes().as_slice())
        .bind(digest(cli.seed, b"revision-identity", index).as_slice())
        .bind(i64::try_from(1_000 + index % 10_000).map_err(|error| error.to_string())?)
        .bind(created_at)
        .bind(created_at)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO discovery_file_revision_states
             (revision_id,status,matching_observations,first_observed_at_us,
              last_counted_observed_at_us,last_observed_at_us,next_check_at_us,stable_at_us,
              missing_at_us,last_observation_source,skip_reason,version,updated_at_us)
             VALUES (?,'stable',2,1,2,2,NULL,2,NULL,'reconcile',NULL,1,2)",
        )
        .bind(revision_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    }

    for task_index in 0..candidate_task_count {
        insert_candidate_task(&mut tx, cli, account_id, inbox_id, task_index).await?;
    }
    for task_offset in 0..cli.unfinished_task_count {
        insert_unfinished_task(
            &mut tx,
            cli,
            account_id,
            inbox_id,
            candidate_task_count + task_offset,
            task_offset,
        )
        .await?;
    }
    tx.commit().await.map_err(|error| error.to_string())?;
    Ok(FixtureSummary {
        revision_count: cli.revision_count,
        candidate_count: cli.candidate_count,
        unfinished_task_count: cli.unfinished_task_count,
        review_case_count: candidate_task_count,
        processing_task_count,
        stage_count: 5,
        seed: cli.seed,
        account_id,
        inbox_directory_id: inbox_id,
    })
}

#[allow(clippy::too_many_lines)]
async fn insert_candidate_task(
    tx: &mut Transaction<'_, Sqlite>,
    cli: &Cli,
    account_id: Uuid,
    inbox_id: Uuid,
    task_index: usize,
) -> Result<(), String> {
    let tracked_id = deterministic_id(cli.seed, b"tracked-file", task_index);
    let revision_id = deterministic_id(cli.seed, b"revision", task_index);
    let task_id = deterministic_id(cli.seed, b"candidate-task", task_index);
    let processing_attempt_id =
        deterministic_id(cli.seed, b"candidate-processing-attempt", task_index);
    let identification_attempt_id =
        deterministic_id(cli.seed, b"identification-attempt", task_index);
    let decision_id = deterministic_id(cli.seed, b"decision", task_index);
    let review_id = deterministic_id(cli.seed, b"review", task_index);
    let now = 2_000_000_i64 + i64::try_from(task_index).map_err(|error| error.to_string())?;
    insert_processing_task(
        tx,
        task_id,
        account_id,
        tracked_id,
        revision_id,
        inbox_id,
        processing_attempt_id,
        "waiting-confirmation",
        "identification",
        "waiting-confirmation",
        Some("identification.ambiguous"),
        None,
        None,
        now,
    )
    .await?;
    sqlx::query(
        "INSERT INTO tasks_processing_attempts
         (id,task_id,reason,ordinal,status,stage,started_at_us,finished_at_us,created_at_us)
         VALUES (?,?,'initial',1,'succeeded','identification',?,?,?)",
    )
    .bind(processing_attempt_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO identification_attempts
         (id,task_id,processing_attempt_id,file_revision_id,ordinal,parser_version,
          provider_version,rule_version,status,started_at_us,finished_at_us)
         VALUES (?,?,?,?,1,'filename-v1','tmdb-v1',1,'decided',?,?)",
    )
    .bind(identification_attempt_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .bind(processing_attempt_id.as_bytes().as_slice())
    .bind(revision_id.as_bytes().as_slice())
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    let first_candidate = task_index * CANDIDATES_PER_TASK;
    let candidate_end = (first_candidate + CANDIDATES_PER_TASK).min(cli.candidate_count);
    for (ordinal, candidate_index) in (first_candidate..candidate_end).enumerate() {
        let candidate_id = deterministic_id(cli.seed, b"candidate", candidate_index);
        let title = format!("Fixture Candidate {candidate_index:06}");
        sqlx::query(
            "INSERT INTO identification_candidates
             (id,attempt_id,ordinal,provider,media_type,provider_id,year,locale,original_title,
              ranking_score,provider_version,source_hash,created_at_us)
             VALUES (?,? ,?,'tmdb','movie',?,2024,'en-US',?,?,1,?,?)",
        )
        .bind(candidate_id.as_bytes().as_slice())
        .bind(identification_attempt_id.as_bytes().as_slice())
        .bind(i64::try_from(ordinal + 1).map_err(|error| error.to_string())?)
        .bind(i64::try_from(candidate_index + 1).map_err(|error| error.to_string())?)
        .bind(&title)
        .bind(i64::try_from(candidate_index % 101).map_err(|error| error.to_string())?)
        .bind(digest(cli.seed, b"candidate-source", candidate_index).as_slice())
        .bind(now)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO identification_candidate_titles(candidate_id,kind,ordinal,value)
             VALUES (?,'title',1,?)",
        )
        .bind(candidate_id.as_bytes().as_slice())
        .bind(title)
        .execute(&mut **tx)
        .await
        .map_err(|error| error.to_string())?;
    }
    sqlx::query(
        "INSERT INTO identification_decisions
         (id,attempt_id,level,reason,selected_candidate_id,retry_at_us,rule_version,decided_at_us)
         VALUES (?,?,'ambiguous','identification.ambiguous',NULL,NULL,1,?)",
    )
    .bind(decision_id.as_bytes().as_slice())
    .bind(identification_attempt_id.as_bytes().as_slice())
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    sqlx::query(
        "INSERT INTO identification_review_cases
         (id,account_id,task_id,attempt_id,decision_id,file_revision_id,inbox_directory_id,
          level,reason,title_hint,status,created_at_us,updated_at_us,closed_at_us)
         VALUES (?,?,?,?,?,?,?,'ambiguous','identification.ambiguous',?,'active',?,?,NULL)",
    )
    .bind(review_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .bind(identification_attempt_id.as_bytes().as_slice())
    .bind(decision_id.as_bytes().as_slice())
    .bind(revision_id.as_bytes().as_slice())
    .bind(inbox_id.as_bytes().as_slice())
    .bind(format!("Fixture {task_index:06}"))
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

async fn insert_unfinished_task(
    tx: &mut Transaction<'_, Sqlite>,
    cli: &Cli,
    account_id: Uuid,
    inbox_id: Uuid,
    revision_index: usize,
    task_offset: usize,
) -> Result<(), String> {
    const STAGES: [&str; 5] = [
        "identification",
        "planning",
        "file-operation",
        "nfo",
        "completion",
    ];
    let stage = STAGES[task_offset % STAGES.len()];
    let checkpoint = if stage == "identification" {
        "pending"
    } else {
        "identification-complete"
    };
    let tracked_id = deterministic_id(cli.seed, b"tracked-file", revision_index);
    let revision_id = deterministic_id(cli.seed, b"revision", revision_index);
    let task_id = deterministic_id(cli.seed, b"unfinished-task", task_offset);
    let attempt_id = deterministic_id(cli.seed, b"unfinished-attempt", task_offset);
    let now = 4_000_000_i64 + i64::try_from(task_offset).map_err(|error| error.to_string())?;
    insert_processing_task(
        tx,
        task_id,
        account_id,
        tracked_id,
        revision_id,
        inbox_id,
        attempt_id,
        "running",
        stage,
        checkpoint,
        None,
        Some("expired-m3-fixture"),
        Some(0),
        now,
    )
    .await?;
    sqlx::query(
        "INSERT INTO tasks_processing_attempts
         (id,task_id,reason,ordinal,status,stage,lease_owner,started_at_us,created_at_us)
         VALUES (?,?,'initial',1,'running',?,'expired-m3-fixture',?,?)",
    )
    .bind(attempt_id.as_bytes().as_slice())
    .bind(task_id.as_bytes().as_slice())
    .bind(stage)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_processing_task(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: Uuid,
    account_id: Uuid,
    tracked_id: Uuid,
    revision_id: Uuid,
    inbox_id: Uuid,
    attempt_id: Uuid,
    status: &str,
    stage: &str,
    checkpoint: &str,
    reason: Option<&str>,
    lease_owner: Option<&str>,
    lease_expires_at_us: Option<i64>,
    now: i64,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO tasks_processing_tasks
         (id,account_id,discovered_file_id,file_revision_id,inbox_directory_id,current_attempt_id,
          status,stage,checkpoint,reason,recovering,attempt_count,cancel_requested,lease_owner,
          lease_expires_at_us,next_retry_at_us,config_snapshot_json,version,created_at_us,updated_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,?,0,1,0,?,?,NULL,
                 '{\"discovery_policy_version\":1,\"processing_schema_version\":1}',1,?,?)",
    )
    .bind(task_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(tracked_id.as_bytes().as_slice())
    .bind(revision_id.as_bytes().as_slice())
    .bind(inbox_id.as_bytes().as_slice())
    .bind(attempt_id.as_bytes().as_slice())
    .bind(status)
    .bind(stage)
    .bind(checkpoint)
    .bind(reason)
    .bind(lease_owner)
    .bind(lease_expires_at_us)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn deterministic_id(seed: u64, namespace: &[u8], index: usize) -> Uuid {
    let bytes = digest(seed, namespace, index);
    let mut id = [0_u8; 16];
    id.copy_from_slice(&bytes[..16]);
    id[6] = (id[6] & 0x0f) | 0x70;
    id[8] = (id[8] & 0x3f) | 0x80;
    Uuid::from_bytes(id)
}

fn digest(seed: u64, namespace: &[u8], index: usize) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.m3-identification-fixture.v1\0");
    hasher.update(seed.to_be_bytes());
    hasher.update(namespace);
    hasher.update([0]);
    hasher.update(index.to_be_bytes());
    hasher.finalize().into()
}
