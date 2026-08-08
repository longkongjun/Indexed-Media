use std::path::{Component, Path, PathBuf};

use clap::Parser;
use mediaflow_core::bootstrap::config::{AppConfig, RunMode};
use mediaflow_core::discovery::DiscoveryUseCases;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::CreateInboxCommand;
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

const MARKER_NAME: &str = ".mediaflow-m2-fixture.json";
const FILE_COUNT: usize = 100_000;
const TASK_COUNT: usize = 1_000;

#[derive(Debug, Parser)]
#[command(name = "m2-fixture")]
struct Cli {
    #[arg(long)]
    root: PathBuf,
    #[arg(long)]
    config_dir: PathBuf,
    #[arg(long)]
    replace: bool,
}

#[derive(Serialize)]
struct FixtureSummary {
    file_count: usize,
    unfinished_task_count: usize,
    lease_state: &'static str,
}

#[derive(Deserialize, Serialize)]
struct FixtureMarker {
    fixture: String,
    schema_version: u32,
    root: PathBuf,
    config_dir: PathBuf,
}

#[tokio::main]
async fn main() {
    if let Err(message) = run(&Cli::parse()).await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

async fn run(cli: &Cli) -> Result<(), String> {
    validate_target_spelling(&cli.root, &cli.config_dir)?;
    let mut created = CreatedDirectories::default();
    materialize_target(&cli.root, "root", &mut created)?;
    materialize_target(&cli.config_dir, "config directory", &mut created)?;
    let identity = validate_target_pair(&cli.root, &cli.config_dir)?;
    let (root_state, config_state) = inspect_targets(cli, &identity)?;
    created.keep();
    prepare_target(&cli.root, root_state)?;
    prepare_target(&cli.config_dir, config_state)?;
    write_marker(&cli.config_dir, &identity)?;
    create_files(&cli.root)?;
    seed_database(&cli.root, &cli.config_dir).await?;
    let summary = FixtureSummary {
        file_count: FILE_COUNT,
        unfinished_task_count: TASK_COUNT,
        lease_state: "expired",
    };
    println!(
        "{}",
        serde_json::to_string(&summary).map_err(|error| error.to_string())?
    );
    Ok(())
}

#[derive(Clone, Copy)]
enum TargetState {
    Missing,
    Empty,
    ReplaceFixture,
}

fn inspect_targets(
    cli: &Cli,
    identity: &TargetPairIdentity,
) -> Result<(TargetState, TargetState), String> {
    let root_state = inspect_target(&cli.root, "root")?;
    let config_state = inspect_target(&cli.config_dir, "config directory")?;
    if !cli.replace {
        if matches!(root_state, TargetState::ReplaceFixture) {
            return Err("root already contains data; use --replace explicitly".to_owned());
        }
        if matches!(config_state, TargetState::ReplaceFixture) {
            return Err(
                "config directory already contains data; use --replace explicitly".to_owned(),
            );
        }
        return Ok((root_state, config_state));
    }
    if matches!(root_state, TargetState::ReplaceFixture)
        || matches!(config_state, TargetState::ReplaceFixture)
    {
        let marker_path = cli.config_dir.join(MARKER_NAME);
        let metadata = std::fs::symlink_metadata(&marker_path)
            .map_err(|_| "refusing to replace targets without their config marker".to_owned())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("refusing to replace targets without a regular config marker".to_owned());
        }
        let marker: FixtureMarker = serde_json::from_slice(
            &std::fs::read(&marker_path)
                .map_err(|error| format!("cannot read fixture marker: {error}"))?,
        )
        .map_err(|_| "refusing to replace targets with a foreign marker".to_owned())?;
        if marker.fixture != "mediaflow-m2"
            || marker.schema_version != 1
            || marker.root != identity.root
            || marker.config_dir != identity.config_dir
        {
            return Err("refusing to replace targets not bound by their marker".to_owned());
        }
    }
    Ok((root_state, config_state))
}

fn inspect_target(path: &Path, label: &str) -> Result<TargetState, String> {
    let Some(metadata) = std::fs::symlink_metadata(path)
        .map(Some)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(error)
            }
        })
        .map_err(|error: std::io::Error| format!("cannot inspect {label}: {error}"))?
    else {
        return Ok(TargetState::Missing);
    };
    if !metadata.is_dir() {
        return Err(format!("{label} must be a directory"));
    }
    let mut entries =
        std::fs::read_dir(path).map_err(|error| format!("cannot inspect {label}: {error}"))?;
    if entries.next().is_none() {
        return Ok(TargetState::Empty);
    }
    Ok(TargetState::ReplaceFixture)
}

fn prepare_target(path: &Path, state: TargetState) -> Result<(), String> {
    if matches!(state, TargetState::ReplaceFixture) {
        std::fs::remove_dir_all(path)
            .map_err(|error| format!("cannot replace fixture target: {error}"))?;
    }
    std::fs::create_dir_all(path).map_err(|error| format!("cannot create fixture target: {error}"))
}

fn write_marker(config_dir: &Path, identity: &TargetPairIdentity) -> Result<(), String> {
    let marker = FixtureMarker {
        fixture: "mediaflow-m2".to_owned(),
        schema_version: 1,
        root: identity.root.clone(),
        config_dir: identity.config_dir.clone(),
    };
    std::fs::write(
        config_dir.join(MARKER_NAME),
        serde_json::to_vec(&marker).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write fixture marker: {error}"))
}

fn create_files(root: &Path) -> Result<(), String> {
    for bucket in 0..100 {
        let directory = root.join(format!("bucket-{bucket:03}"));
        std::fs::create_dir(&directory)
            .map_err(|error| format!("cannot create fixture bucket: {error}"))?;
        for item in 0..1_000 {
            let sequence = bucket * 1_000 + item;
            std::fs::File::create(directory.join(format!("file-{sequence:06}.media")))
                .map_err(|error| format!("cannot create fixture file: {error}"))?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn seed_database(root: &Path, config_dir: &Path) -> Result<(), String> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve fixture root: {error}"))?;
    let deployment_roots_file = config_dir.join("deployment-roots.json");
    std::fs::write(
        &deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming",
            "label":"M2 benchmark fixture",
            "container_path":root,
            "access":"read-only"
        }]}))
        .map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("cannot write deployment roots: {error}"))?;
    let config = AppConfig {
        mode: RunMode::Development,
        listen: "127.0.0.1:0"
            .parse()
            .map_err(|error| format!("invalid fixture listen address: {error}"))?,
        config_dir: config_dir.to_owned(),
        public_origin: Url::parse("http://127.0.0.1:3000").map_err(|error| error.to_string())?,
        trusted_proxy_cidrs: Vec::new(),
        deployment_roots_file: deployment_roots_file.clone(),
        web_dist: config_dir.join("web-dist"),
    };
    let db = migrate_with_backup(&config)
        .await
        .map_err(|error| error.safe_message().to_owned())?;
    let account_id = Uuid::from_u128(1);
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'m2-fixture','M2 Fixture','benchmark-only',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .map_err(|error| error.to_string())?;
    let roots = DeploymentRootSet::load(&deployment_roots_file, RunMode::Development)
        .map_err(|error| error.safe_message().to_owned())?;
    let fs: std::sync::Arc<dyn CapabilityFs> = std::sync::Arc::new(
        OsCapabilityFs::open(roots.declarations(), RunMode::Development)
            .map_err(|error| error.safe_message().to_owned())?,
    );
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .map_err(|error| error.safe_message().to_owned())?;
    let mut transaction = db.pool().begin().await.map_err(|error| error.to_string())?;
    for index in 0..TASK_COUNT {
        let sequence = u128::try_from(index).map_err(|error| error.to_string())? + 10;
        let task_id = Uuid::from_u128(0x1000_0000_0000_0000_0000_0000_0000_0000 + sequence);
        let batch_id = Uuid::from_u128(0x2000_0000_0000_0000_0000_0000_0000_0000 + sequence);
        let attempt_id = Uuid::from_u128(0x3000_0000_0000_0000_0000_0000_0000_0000 + sequence);
        sqlx::query(
            "INSERT INTO discovery_scan_batches
             (id,inbox_directory_id,inbox_version,started_at_us,created_at_us,updated_at_us)
             VALUES (?,?,1,1,1,1)",
        )
        .bind(batch_id.as_bytes().as_slice())
        .bind(inbox.id.as_bytes().as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO tasks_scan_tasks
             (id,account_id,scan_batch_id,inbox_directory_id,status,stage,recovering,
              current_attempt_id,lease_owner,lease_expires_at_us,created_at_us,updated_at_us)
             VALUES (?,?,?,?,'running','enumerating',0,?,'expired-fixture-worker',0,1,1)",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(batch_id.as_bytes().as_slice())
        .bind(inbox.id.as_bytes().as_slice())
        .bind(attempt_id.as_bytes().as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO tasks_scan_attempts
             (id,task_id,reason,ordinal,status,lease_owner,started_at_us,created_at_us)
             VALUES (?,?,'initial',1,'running','expired-fixture-worker',1,1)",
        )
        .bind(attempt_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
        sqlx::query(
            "INSERT INTO platform_outbox_events
             (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
             VALUES ('task.state-changed','1',?,? ,1)",
        )
        .bind(task_id.as_bytes().as_slice())
        .bind(format!(
            "{{\"status\":\"running\",\"recovering\":false,\"task_id\":\"{task_id}\"}}"
        ))
        .execute(&mut *transaction)
        .await
        .map_err(|error| error.to_string())?;
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    db.pool().close().await;
    Ok(())
}

#[derive(Debug)]
struct TargetPairIdentity {
    root: PathBuf,
    config_dir: PathBuf,
}

fn validate_target_spelling(root: &Path, config_dir: &Path) -> Result<(), String> {
    validate_target(root, "root")?;
    validate_target(config_dir, "config directory")?;
    if root.starts_with(config_dir) || config_dir.starts_with(root) {
        return Err("fixture root and config directory must not overlap".to_owned());
    }
    Ok(())
}

fn validate_target_pair(root: &Path, config_dir: &Path) -> Result<TargetPairIdentity, String> {
    validate_target_spelling(root, config_dir)?;
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| format!("cannot resolve fixture root: {error}"))?;
    let canonical_config = std::fs::canonicalize(config_dir)
        .map_err(|error| format!("cannot resolve fixture config directory: {error}"))?;
    if canonical_root.starts_with(&canonical_config)
        || canonical_config.starts_with(&canonical_root)
        || directory_is_ancestor_by_identity(&canonical_root, &canonical_config)?
        || directory_is_ancestor_by_identity(&canonical_config, &canonical_root)?
    {
        return Err("fixture root and config directory must not overlap".to_owned());
    }
    Ok(TargetPairIdentity {
        root: canonical_root,
        config_dir: canonical_config,
    })
}

#[cfg(unix)]
fn directory_is_ancestor_by_identity(
    possible_ancestor: &Path,
    target: &Path,
) -> Result<bool, String> {
    use std::os::unix::fs::MetadataExt as _;

    let ancestor_metadata = std::fs::metadata(possible_ancestor)
        .map_err(|error| format!("cannot inspect fixture target identity: {error}"))?;
    let mut candidate = Some(target);
    while let Some(path) = candidate {
        let metadata = std::fs::metadata(path)
            .map_err(|error| format!("cannot inspect fixture target identity: {error}"))?;
        if metadata.dev() == ancestor_metadata.dev() && metadata.ino() == ancestor_metadata.ino() {
            return Ok(true);
        }
        candidate = path.parent();
    }
    Ok(false)
}

#[cfg(not(unix))]
fn directory_is_ancestor_by_identity(
    possible_ancestor: &Path,
    target: &Path,
) -> Result<bool, String> {
    Ok(std::fs::canonicalize(target)
        .map_err(|error| format!("cannot inspect fixture target identity: {error}"))?
        .starts_with(
            std::fs::canonicalize(possible_ancestor)
                .map_err(|error| format!("cannot inspect fixture target identity: {error}"))?,
        ))
}

#[derive(Default)]
struct CreatedDirectories {
    paths: Vec<PathBuf>,
    keep: bool,
}

impl CreatedDirectories {
    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for CreatedDirectories {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        for path in self.paths.iter().rev() {
            let _ = std::fs::remove_dir(path);
        }
    }
}

fn materialize_target(
    path: &Path,
    label: &str,
    created: &mut CreatedDirectories,
) -> Result<(), String> {
    let mut missing = Vec::new();
    let mut candidate = Some(path);
    while let Some(current) = candidate {
        match std::fs::symlink_metadata(current) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(format!(
                        "{label} must be a directory without symbolic links"
                    ));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(current.to_owned());
                candidate = current.parent();
            }
            Err(error) => return Err(format!("cannot inspect {label}: {error}")),
        }
    }
    for directory in missing.into_iter().rev() {
        match std::fs::create_dir(&directory) {
            Ok(()) => created.paths.push(directory),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let metadata = std::fs::symlink_metadata(&directory)
                    .map_err(|inspect| format!("cannot inspect {label}: {inspect}"))?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(format!(
                        "{label} must be a directory without symbolic links"
                    ));
                }
            }
            Err(error) => return Err(format!("cannot create {label}: {error}")),
        }
    }
    Ok(())
}

fn validate_target(path: &Path, label: &str) -> Result<(), String> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(format!("{label} must be a non-root absolute path"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(format!("{label} must not contain dot path components"));
    }
    let mut ancestor = Some(path);
    while let Some(candidate) = ancestor {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("{label} must not traverse a symbolic link"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect {label}: {error}")),
        }
        ancestor = candidate.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_target_pair;

    #[test]
    fn case_insensitive_aliases_are_rejected_by_filesystem_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let temporary_root = std::fs::canonicalize(temporary.path()).unwrap();
        let canonical = temporary_root.join("FixtureCaseAlias");
        std::fs::create_dir(&canonical).unwrap();
        let alias = temporary_root.join("fixturecasealias");
        if !alias.is_dir() {
            eprintln!("skipped: test filesystem is case-sensitive");
            return;
        }

        let error = validate_target_pair(&canonical, &alias).unwrap_err();
        assert!(error.contains("must not overlap"), "{error}");
    }
}
