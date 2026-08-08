#![allow(clippy::too_many_lines)]

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mediaflow_core::discovery::capability::{
    CapabilityFs, DeploymentRootSet, DirectoryEntryStream,
};
use mediaflow_core::discovery::model::{
    DeploymentRootView, DirectoryCapability, DirectoryEntry, DirectoryIdentity, EntryMetadata,
    FsBoundaryError, RelativePath, RootAccess, RootId,
};
use mediaflow_core::discovery::observations::{FileObservation, ObservationSink, ScanEntryError};
use mediaflow_core::discovery::scanner::ScanStopToken;
use mediaflow_core::discovery::scanner::{BoundedScanner, ScanSource, Scanner};
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::task_runtime::ManualTaskClock;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::model::{NewScanTask, ScanCounts, ScanStatus};
use mediaflow_core::tasks::store::TaskStore;
use mediaflow_core::tasks::worker::ScanWorker;
use uuid::Uuid;

#[derive(Clone, Default)]
struct RecordingSink {
    batches: Arc<Mutex<Vec<ObservationBatch>>>,
}

#[derive(Clone)]
struct FailingSink;

#[async_trait]
impl ObservationSink for FailingSink {
    async fn write_batch(
        &self,
        _files: Vec<FileObservation>,
        _errors: Vec<ScanEntryError>,
        _progress: ScanCounts,
    ) -> Result<(), mediaflow_core::shared::error::AppError> {
        Err(mediaflow_core::shared::error::AppError::new(
            ErrorCode::TaskLeaseLost,
            "injected lease loss",
        ))
    }
}

struct CountingCapabilityFs {
    inner: Arc<dyn CapabilityFs>,
    calls: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
}

impl CapabilityFs for CountingCapabilityFs {
    fn preflight_directory(
        &self,
        root: &RootId,
        relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        self.inner.preflight_directory(root, relative)
    }

    fn read_directory(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        self.inner.read_directory(capability)
    }

    fn open_directory_stream(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Box<dyn DirectoryEntryStream>, FsBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active_streams.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(CountingDirectoryStream {
            inner: self.inner.open_directory_stream(capability)?,
            calls: Arc::clone(&self.calls),
            active_streams: Arc::clone(&self.active_streams),
        }))
    }

    fn open_child_directory(
        &self,
        capability: &DirectoryCapability,
        name: &std::ffi::OsStr,
    ) -> Result<DirectoryCapability, FsBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.open_child_directory(capability, name)
    }

    fn metadata_no_follow(
        &self,
        capability: &DirectoryCapability,
        name: &std::ffi::OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_micros(100));
        self.inner.metadata_no_follow(capability, name)
    }
}

struct CountingDirectoryStream {
    inner: Box<dyn DirectoryEntryStream>,
    calls: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
}

impl DirectoryEntryStream for CountingDirectoryStream {
    fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, FsBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_micros(100));
        self.inner.next_entry()
    }
}

impl Drop for CountingDirectoryStream {
    fn drop(&mut self) {
        self.active_streams.fetch_sub(1, Ordering::SeqCst);
    }
}

struct RootChangedFs;

impl CapabilityFs for RootChangedFs {
    fn preflight_directory(
        &self,
        _root: &RootId,
        _relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        Err(FsBoundaryError::RootChanged)
    }

    fn read_directory(
        &self,
        _capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        unreachable!("pre-scan root failure must not enumerate")
    }

    fn metadata_no_follow(
        &self,
        _capability: &DirectoryCapability,
        _name: &std::ffi::OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        unreachable!("pre-scan root failure must not read metadata")
    }
}

#[derive(Clone, Copy)]
enum RootReplacementKind {
    Symlink,
    Missing,
}

struct RootReplacementFs {
    inner: Arc<dyn CapabilityFs>,
    root: std::path::PathBuf,
    previous_root: std::path::PathBuf,
    kind: RootReplacementKind,
    replaced: Arc<std::sync::atomic::AtomicBool>,
    calls_after_detection: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
    detected: Option<Arc<std::sync::Barrier>>,
    release: Option<Arc<std::sync::Barrier>>,
}

impl RootReplacementFs {
    fn record_if_replaced(&self) {
        if self.replaced.load(Ordering::SeqCst) {
            self.calls_after_detection.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl CapabilityFs for RootReplacementFs {
    fn preflight_directory(
        &self,
        root: &RootId,
        relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        self.record_if_replaced();
        self.inner.preflight_directory(root, relative)
    }

    fn read_directory(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        self.record_if_replaced();
        self.inner.read_directory(capability)
    }

    fn open_directory_stream(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Box<dyn DirectoryEntryStream>, FsBoundaryError> {
        self.record_if_replaced();
        let inner = self.inner.open_directory_stream(capability)?;
        self.active_streams.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(RootReplacementStream {
            inner,
            replaced: Arc::clone(&self.replaced),
            calls_after_detection: Arc::clone(&self.calls_after_detection),
            active_streams: Arc::clone(&self.active_streams),
        }))
    }

    fn open_child_directory(
        &self,
        capability: &DirectoryCapability,
        name: &std::ffi::OsStr,
    ) -> Result<DirectoryCapability, FsBoundaryError> {
        self.record_if_replaced();
        self.inner.open_child_directory(capability, name)
    }

    fn metadata_no_follow(
        &self,
        capability: &DirectoryCapability,
        name: &std::ffi::OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        if self.replaced.swap(true, Ordering::SeqCst) {
            self.calls_after_detection.fetch_add(1, Ordering::SeqCst);
            return self.inner.metadata_no_follow(capability, name);
        }
        std::fs::rename(&self.root, &self.previous_root).unwrap();
        if matches!(self.kind, RootReplacementKind::Symlink) {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&self.previous_root, &self.root).unwrap();
        }
        let result = self.inner.metadata_no_follow(capability, name);
        if let Some(detected) = &self.detected {
            detected.wait();
        }
        if let Some(release) = &self.release {
            release.wait();
        }
        result
    }
}

struct RootReplacementStream {
    inner: Box<dyn DirectoryEntryStream>,
    replaced: Arc<std::sync::atomic::AtomicBool>,
    calls_after_detection: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
}

impl DirectoryEntryStream for RootReplacementStream {
    fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, FsBoundaryError> {
        if self.replaced.load(Ordering::SeqCst) {
            self.calls_after_detection.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.next_entry()
    }
}

impl Drop for RootReplacementStream {
    fn drop(&mut self) {
        self.active_streams.fetch_sub(1, Ordering::SeqCst);
    }
}

enum MetadataFault {
    EntryUnavailable(String),
    RootChangedAfter(usize),
    RootChangedAfterCancellation {
        threshold: usize,
        detected: Arc<std::sync::Barrier>,
        release: Arc<std::sync::Barrier>,
    },
}

struct MetadataFaultFs {
    inner: Arc<dyn CapabilityFs>,
    fault: MetadataFault,
    metadata_calls: AtomicUsize,
    calls_after_root_change: Arc<AtomicUsize>,
    root_changed: Arc<std::sync::atomic::AtomicBool>,
    active_streams: Arc<AtomicUsize>,
}

impl CapabilityFs for MetadataFaultFs {
    fn preflight_directory(
        &self,
        root: &RootId,
        relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        self.inner.preflight_directory(root, relative)
    }

    fn read_directory(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        if self.root_changed.load(Ordering::SeqCst) {
            self.calls_after_root_change.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.read_directory(capability)
    }

    fn open_directory_stream(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Box<dyn DirectoryEntryStream>, FsBoundaryError> {
        if self.root_changed.load(Ordering::SeqCst) {
            self.calls_after_root_change.fetch_add(1, Ordering::SeqCst);
        }
        let inner = self.inner.open_directory_stream(capability)?;
        self.active_streams.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FaultTrackingStream {
            inner,
            root_changed: Arc::clone(&self.root_changed),
            calls_after_root_change: Arc::clone(&self.calls_after_root_change),
            active_streams: Arc::clone(&self.active_streams),
        }))
    }

    fn open_child_directory(
        &self,
        capability: &DirectoryCapability,
        name: &std::ffi::OsStr,
    ) -> Result<DirectoryCapability, FsBoundaryError> {
        if self.root_changed.load(Ordering::SeqCst) {
            self.calls_after_root_change.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.open_child_directory(capability, name)
    }

    fn metadata_no_follow(
        &self,
        capability: &DirectoryCapability,
        name: &std::ffi::OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        if self.root_changed.load(Ordering::SeqCst) {
            self.calls_after_root_change.fetch_add(1, Ordering::SeqCst);
        }
        let call = self.metadata_calls.fetch_add(1, Ordering::SeqCst) + 1;
        match &self.fault {
            MetadataFault::EntryUnavailable(failing_name)
                if name.to_string_lossy() == failing_name.as_str() =>
            {
                Err(FsBoundaryError::Unavailable)
            }
            MetadataFault::RootChangedAfter(threshold) if call == *threshold => {
                self.root_changed.store(true, Ordering::SeqCst);
                Err(FsBoundaryError::RootChanged)
            }
            MetadataFault::RootChangedAfterCancellation {
                threshold,
                detected,
                release,
            } if call == *threshold => {
                self.root_changed.store(true, Ordering::SeqCst);
                detected.wait();
                release.wait();
                Err(FsBoundaryError::RootChanged)
            }
            _ => self.inner.metadata_no_follow(capability, name),
        }
    }
}

struct FaultTrackingStream {
    inner: Box<dyn DirectoryEntryStream>,
    root_changed: Arc<std::sync::atomic::AtomicBool>,
    calls_after_root_change: Arc<AtomicUsize>,
    active_streams: Arc<AtomicUsize>,
}

impl DirectoryEntryStream for FaultTrackingStream {
    fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, FsBoundaryError> {
        if self.root_changed.load(Ordering::SeqCst) {
            self.calls_after_root_change.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.next_entry()
    }
}

impl Drop for FaultTrackingStream {
    fn drop(&mut self) {
        self.active_streams.fetch_sub(1, Ordering::SeqCst);
    }
}

type ObservationBatch = (Vec<FileObservation>, Vec<ScanEntryError>);

#[async_trait]
impl ObservationSink for RecordingSink {
    async fn write_batch(
        &self,
        files: Vec<FileObservation>,
        errors: Vec<ScanEntryError>,
        _progress: ScanCounts,
    ) -> Result<(), mediaflow_core::shared::error::AppError> {
        self.batches.lock().unwrap().push((files, errors));
        Ok(())
    }
}

#[test]
fn scan_state_and_counts_start_from_a_durable_queued_baseline() {
    assert_eq!(ScanStatus::Queued.as_str(), "queued");
    assert_eq!(ScanCounts::default().observed_files, 0);
}

#[tokio::test]
async fn creation_is_atomic_idempotent_and_claiming_has_one_lease_winner() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    let inbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let request = NewScanTask {
        account_id,
        inbox_directory_id: inbox_id,
        idempotency_key: "create-1".to_owned(),
        now_us: 1_000_000,
    };

    let first = store.create(request.clone()).await.unwrap();
    let duplicate = store.create(request).await.unwrap();

    assert_eq!(first.id, duplicate.id);
    assert_eq!(first.status, ScanStatus::Queued);
    for table in [
        "tasks_scan_tasks",
        "discovery_scan_batches",
        "tasks_scan_attempts",
        "tasks_idempotency_keys",
        "platform_outbox_events",
    ] {
        let count = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 1, "{table}");
    }

    let one = store.claim_next("worker-a", 2_000_000).await.unwrap();
    let two = store.claim_next("worker-b", 2_000_000).await.unwrap();
    assert_eq!(one.as_ref().map(|lease| lease.task.id), Some(first.id));
    assert!(two.is_none());

    let lease = one.unwrap();
    let observation = FileObservation {
        inbox_directory_id: inbox_id,
        relative_path_bytes: b"movie.mkv".to_vec(),
        relative_path_display: "movie.mkv".to_owned(),
        identity_snapshot: vec![9; 25],
        size_bytes: 42,
        modified_at_ns: 123,
    };
    let lease = store
        .record_observations(
            &lease,
            std::slice::from_ref(&observation),
            &[],
            ScanCounts {
                observed_files: 1,
                ..ScanCounts::default()
            },
            2_100_000,
        )
        .await
        .unwrap();
    let lease = store
        .record_observations(
            &lease,
            &[observation],
            &[],
            ScanCounts {
                observed_files: 1,
                ..ScanCounts::default()
            },
            2_200_000,
        )
        .await
        .unwrap();
    assert_eq!(lease.task.counts.observed_files, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_files")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_scan_file_observations")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
    let stale = store
        .commit_batch(
            &lease,
            ScanCounts {
                observed_files: 1,
                ..ScanCounts::default()
            },
            40_000_001,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code(), ErrorCode::TaskLeaseLost);
}

#[tokio::test]
async fn running_progress_transaction_persists_visited_and_skipped_counter_snapshot() {
    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    let inbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: "progress-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let lease = store
        .claim_next("progress-worker", 2_000_000)
        .await
        .unwrap()
        .unwrap();
    let observation = FileObservation {
        inbox_directory_id: inbox_id,
        relative_path_bytes: b"movie.mkv".to_vec(),
        relative_path_display: "movie.mkv".to_owned(),
        identity_snapshot: vec![9; 25],
        size_bytes: 42,
        modified_at_ns: 123,
    };
    let progress = ScanCounts {
        visited_directories: 7,
        observed_files: 1,
        skipped_entries: 3,
        errors: 0,
    };

    let lease = store
        .record_observations(&lease, &[observation], &[], progress, 2_100_000)
        .await
        .unwrap();

    assert_eq!(lease.task.counts, progress);
    assert_eq!(
        store.get(account_id, task.id).await.unwrap().counts,
        progress
    );
    let payload = sqlx::query_scalar::<_, String>(
        "SELECT payload_json FROM platform_outbox_events
         WHERE aggregate_id=? AND event_type='task.progress' ORDER BY id DESC LIMIT 1",
    )
    .bind(task.id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["visited_directories"], 7);
    assert_eq!(payload["skipped_entries"], 3);
}

#[tokio::test]
async fn bounded_scanner_streams_batches_preserves_raw_names_and_skips_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    for index in 0..1_201 {
        std::fs::write(root.join(format!("file-{index:04}.mkv")), b"x").unwrap();
    }
    for index in 0..130 {
        std::fs::create_dir(root.join(format!("dir-{index:03}"))).unwrap();
    }
    #[cfg(unix)]
    let raw_created = {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;
        let raw_created = std::fs::write(
            root.join(std::ffi::OsString::from_vec(b"raw-\xff.mkv".to_vec())),
            b"x",
        )
        .is_ok();
        symlink(root.join("file-0000.mkv"), root.join("skip-link")).unwrap();
        raw_created
    };
    #[cfg(not(unix))]
    let raw_created = false;
    let root = std::fs::canonicalize(root).unwrap();
    let config = temp.path().join("roots.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let roots = DeploymentRootSet::load(
        &config,
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let root_id = mediaflow_core::discovery::model::RootId::parse("incoming").unwrap();
    let capability = fs
        .preflight_directory(
            &root_id,
            &mediaflow_core::discovery::model::RelativePath::parse(".").unwrap(),
        )
        .unwrap()
        .into_capability();
    let sink = RecordingSink::default();
    let summary = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        BoundedScanner::default().scan(
            ScanSource {
                inbox_directory_id: Uuid::now_v7(),
                capability,
                fs,
            },
            Arc::new(sink.clone()),
            ScanStopToken::default(),
        ),
    )
    .await
    .expect("bounded high-fanout traversal must not deadlock")
    .unwrap();
    let batches = sink.batches.lock().unwrap();
    assert!(
        batches
            .iter()
            .all(|(files, errors)| files.len() + errors.len() <= 500)
    );
    assert_eq!(summary.observed_files, 1_201 + u64::from(raw_created));
    assert_eq!(summary.visited_directories, 131);
    assert_eq!(summary.skipped_entries, 1);
    if raw_created {
        assert!(batches.iter().flat_map(|batch| &batch.0).any(|file| {
            file.relative_path_bytes == b"raw-\xff.mkv"
                && file.relative_path_display.as_bytes() != file.relative_path_bytes
        }));
    }
}

#[tokio::test]
async fn scanner_stops_and_joins_all_filesystem_producers_before_returning_sink_error() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    for index in 0..700 {
        std::fs::write(root.join(format!("file-{index:04}.mkv")), b"x").unwrap();
    }
    for index in 0..300 {
        std::fs::create_dir(root.join(format!("dir-{index:04}"))).unwrap();
    }
    let root = std::fs::canonicalize(root).unwrap();
    let config = temp.path().join("roots.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let roots = DeploymentRootSet::load(
        &config,
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let inner: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let root_id = RootId::parse("incoming").unwrap();
    let capability = inner
        .preflight_directory(&root_id, &RelativePath::parse(".").unwrap())
        .unwrap()
        .into_capability();
    let calls = Arc::new(AtomicUsize::new(0));
    let active_streams = Arc::new(AtomicUsize::new(0));
    let fs: Arc<dyn CapabilityFs> = Arc::new(CountingCapabilityFs {
        inner,
        calls: Arc::clone(&calls),
        active_streams: Arc::clone(&active_streams),
    });

    let error = BoundedScanner::default()
        .scan(
            ScanSource {
                inbox_directory_id: Uuid::now_v7(),
                capability,
                fs,
            },
            Arc::new(FailingSink),
            ScanStopToken::default(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::TaskLeaseLost);
    let calls_at_return = calls.load(Ordering::SeqCst);
    assert_eq!(active_streams.load(Ordering::SeqCst), 0);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(calls.load(Ordering::SeqCst), calls_at_return);
}

#[tokio::test]
async fn empty_directory_traversal_reaches_a_bounded_cancellation_checkpoint() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    std::fs::create_dir(&root).unwrap();
    for index in 0..500 {
        std::fs::create_dir(root.join(format!("empty-{index:04}"))).unwrap();
    }
    let root = std::fs::canonicalize(root).unwrap();
    let config = temp.path().join("roots.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let roots = DeploymentRootSet::load(
        &config,
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let inner: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let capability = inner
        .preflight_directory(
            &RootId::parse("incoming").unwrap(),
            &RelativePath::parse(".").unwrap(),
        )
        .unwrap()
        .into_capability();
    let calls = Arc::new(AtomicUsize::new(0));
    let active_streams = Arc::new(AtomicUsize::new(0));
    let fs: Arc<dyn CapabilityFs> = Arc::new(CountingCapabilityFs {
        inner,
        calls: Arc::clone(&calls),
        active_streams: Arc::clone(&active_streams),
    });

    let error = BoundedScanner::default()
        .scan(
            ScanSource {
                inbox_directory_id: Uuid::now_v7(),
                capability,
                fs,
            },
            Arc::new(FailingSink),
            ScanStopToken::default(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code(), ErrorCode::TaskLeaseLost);
    assert_eq!(active_streams.load(Ordering::SeqCst), 0);
    assert!(
        calls.load(Ordering::SeqCst) < 1_000,
        "cancellation checkpoint must stop before enumerating every empty directory"
    );
}

#[tokio::test]
async fn worker_claims_scans_persists_and_finishes_without_in_memory_queue_truth() {
    use mediaflow_core::discovery::DiscoveryUseCases;
    use mediaflow_core::discovery::model::CreateInboxCommand;
    use mediaflow_core::discovery::service::DiscoveryService;

    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("worker-root");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("a.mkv"), b"a").unwrap();
    std::fs::write(root.join("b.mkv"), b"bb").unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
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
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let fs: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox.id,
            idempotency_key: "worker-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let clock = Arc::new(ManualTaskClock::new(2_000_000));
    let worker = ScanWorker::new(
        "worker-e2e".to_owned(),
        store.clone(),
        discovery,
        Arc::new(BoundedScanner::default()),
        clock,
    );

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    let finished = store.get(account_id, task.id).await.unwrap();
    assert_eq!(finished.status, ScanStatus::Completed);
    assert_eq!(finished.counts.observed_files, 2);
    assert_eq!(finished.counts.visited_directories, 1);
    assert_eq!(worker.run_once().await.unwrap(), None);
}

#[tokio::test]
async fn pre_scan_root_failure_is_persisted_as_one_safe_root_scoped_error() {
    use mediaflow_core::discovery::service::DiscoveryService;

    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account_id = Uuid::now_v7();
    let inbox_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','test',1,1)",
    )
    .bind(account_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let root_id = RootId::parse("incoming").unwrap();
    let discovery = DiscoveryService::new(
        BTreeMap::from([(
            root_id.clone(),
            DeploymentRootView {
                id: root_id,
                label: "Incoming".to_owned(),
                access: RootAccess::ReadOnly,
            },
        )]),
        Arc::new(RootChangedFs),
        db.pool().clone(),
    );
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox_id,
            idempotency_key: "root-failure-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let worker = ScanWorker::new(
        "root-failure-worker".to_owned(),
        store.clone(),
        discovery,
        Arc::new(BoundedScanner::default()),
        Arc::new(ManualTaskClock::new(2_000_000)),
    );

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    let finished = store.get(account_id, task.id).await.unwrap();
    assert_eq!(finished.status, ScanStatus::Failed);
    assert_eq!(finished.counts.errors, 1);
    let errors = store
        .list_errors(
            account_id,
            task.id,
            &PageRequest::new(None, Some(10)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(errors.items.len(), 1);
    assert_eq!(errors.items[0].code, "root.unavailable");
    assert_eq!(errors.items[0].relative_path, ".");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT scope FROM tasks_scan_errors WHERE task_id=?",)
            .bind(task.id.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "root"
    );
}

async fn faulting_worker_fixture(
    files: &[&str],
    fault: MetadataFault,
) -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    Uuid,
    TaskStore,
    mediaflow_core::tasks::model::ScanTaskView,
    ScanWorker,
    Arc<MetadataFaultFs>,
) {
    use mediaflow_core::discovery::DiscoveryUseCases;
    use mediaflow_core::discovery::model::CreateInboxCommand;
    use mediaflow_core::discovery::service::DiscoveryService;

    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("fault-root");
    std::fs::create_dir(&root).unwrap();
    for file in files {
        std::fs::write(root.join(file), b"x").unwrap();
    }
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
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
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let inner: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let fault_fs = Arc::new(MetadataFaultFs {
        inner,
        fault,
        metadata_calls: AtomicUsize::new(0),
        calls_after_root_change: Arc::new(AtomicUsize::new(0)),
        root_changed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        active_streams: Arc::new(AtomicUsize::new(0)),
    });
    let fs: Arc<dyn CapabilityFs> = fault_fs.clone();
    let discovery = DiscoveryService::new(roots.view_map(), fs, db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox.id,
            idempotency_key: "fault-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let worker = ScanWorker::new(
        "fault-worker".to_owned(),
        store.clone(),
        discovery,
        Arc::new(BoundedScanner::default()),
        Arc::new(ManualTaskClock::new(2_000_000)),
    );
    (fixture, db, account_id, store, task, worker, fault_fs)
}

#[tokio::test]
async fn isolated_metadata_error_is_durable_and_other_files_finish_partial_success() {
    let (_fixture, _db, account_id, store, task, worker, _fs) = faulting_worker_fixture(
        &["good-a.mkv", "bad.mkv", "good-b.mkv"],
        MetadataFault::EntryUnavailable("bad.mkv".to_owned()),
    )
    .await;

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    let finished = store.get(account_id, task.id).await.unwrap();
    assert_eq!(finished.status, ScanStatus::PartialSuccess);
    assert_eq!(finished.counts.observed_files, 2);
    assert_eq!(finished.counts.errors, 1);
    let errors = store
        .list_errors(
            account_id,
            task.id,
            &PageRequest::new(None, Some(10)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(errors.items.len(), 1);
    assert_eq!(errors.items[0].code, "entry.unavailable");
    assert_eq!(errors.items[0].relative_path, "bad.mkv");
}

#[tokio::test]
async fn mid_scan_root_change_stops_access_and_persists_only_a_safe_root_error() {
    let (_fixture, _db, account_id, store, task, worker, fs) = faulting_worker_fixture(
        &["a.mkv", "b.mkv", "c.mkv", "d.mkv", "e.mkv", "f.mkv"],
        MetadataFault::RootChangedAfter(3),
    )
    .await;

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    let finished = store.get(account_id, task.id).await.unwrap();
    assert_eq!(finished.status, ScanStatus::Failed);
    assert_eq!(finished.counts.visited_directories, 1);
    assert_eq!(finished.counts.observed_files, 2);
    assert_eq!(finished.counts.errors, 1);
    assert_eq!(fs.calls_after_root_change.load(Ordering::SeqCst), 0);
    let errors = store
        .list_errors(
            account_id,
            task.id,
            &PageRequest::new(None, Some(10)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(errors.items.len(), 1);
    assert_eq!(errors.items[0].code, "root.unavailable");
    assert_eq!(errors.items[0].relative_path, ".");
}

#[tokio::test]
async fn detected_root_failure_wins_over_concurrent_cancel_and_joins_the_producer() {
    let detected = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let (_fixture, _db, account_id, store, task, worker, fs) = faulting_worker_fixture(
        &["a.mkv", "b.mkv", "c.mkv", "d.mkv"],
        MetadataFault::RootChangedAfterCancellation {
            threshold: 2,
            detected: Arc::clone(&detected),
            release: Arc::clone(&release),
        },
    )
    .await;
    let worker_run = tokio::spawn(async move { worker.run_once().await });
    tokio::task::spawn_blocking(move || detected.wait())
        .await
        .unwrap();
    store
        .request_cancel(account_id, task.id, "fatal-race-cancel", 2_100_000)
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || release.wait())
        .await
        .unwrap();

    assert_eq!(worker_run.await.unwrap().unwrap(), Some(task.id));
    let finished = store.get(account_id, task.id).await.unwrap();
    assert_eq!(
        finished.status,
        ScanStatus::Failed,
        "an already-detected capability failure must outrank cancellation"
    );
    assert_eq!(finished.counts.errors, 1);
    let errors = store
        .list_errors(
            account_id,
            task.id,
            &PageRequest::new(None, Some(10)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(errors.items.len(), 1);
    assert_eq!(errors.items[0].code, "root.unavailable");
    assert_eq!(errors.items[0].relative_path, ".");
    assert_eq!(fs.calls_after_root_change.load(Ordering::SeqCst), 0);
    assert_eq!(
        fs.active_streams.load(Ordering::SeqCst),
        0,
        "the scanner must join and drop its filesystem producer before returning"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn real_os_root_replaced_by_symlink_is_fatal_and_joins_the_producer() {
    let (_fixture, db, account_id, store, task, worker, fs) =
        root_replacement_worker_fixture(RootReplacementKind::Symlink, None, None).await;

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    assert_real_root_failure(account_id, &db, &store, task.id, &fs).await;
}

#[tokio::test]
async fn real_os_root_becoming_unavailable_is_fatal_and_joins_the_producer() {
    let (_fixture, db, account_id, store, task, worker, fs) =
        root_replacement_worker_fixture(RootReplacementKind::Missing, None, None).await;

    assert_eq!(worker.run_once().await.unwrap(), Some(task.id));
    assert_real_root_failure(account_id, &db, &store, task.id, &fs).await;
}

#[cfg(unix)]
#[tokio::test]
async fn real_os_root_symlink_failure_wins_over_concurrent_cancel() {
    let detected = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let (_fixture, db, account_id, store, task, worker, fs) = root_replacement_worker_fixture(
        RootReplacementKind::Symlink,
        Some(Arc::clone(&detected)),
        Some(Arc::clone(&release)),
    )
    .await;
    let worker_run = tokio::spawn(async move { worker.run_once().await });
    tokio::task::spawn_blocking(move || detected.wait())
        .await
        .unwrap();
    store
        .request_cancel(account_id, task.id, "real-root-race-cancel", 2_100_000)
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || release.wait())
        .await
        .unwrap();

    assert_eq!(worker_run.await.unwrap().unwrap(), Some(task.id));
    assert_real_root_failure(account_id, &db, &store, task.id, &fs).await;
}

async fn root_replacement_worker_fixture(
    kind: RootReplacementKind,
    detected: Option<Arc<std::sync::Barrier>>,
    release: Option<Arc<std::sync::Barrier>>,
) -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    Uuid,
    TaskStore,
    mediaflow_core::tasks::model::ScanTaskView,
    ScanWorker,
    Arc<RootReplacementFs>,
) {
    use mediaflow_core::discovery::DiscoveryUseCases;
    use mediaflow_core::discovery::model::CreateInboxCommand;
    use mediaflow_core::discovery::service::DiscoveryService;

    let fixture =
        common::TestConfigDir::new(mediaflow_core::bootstrap::config::RunMode::Development);
    let base = fixture.config().config_dir.parent().unwrap();
    let root = base.join("real-replacement-root");
    let previous_root = base.join("real-replacement-root-old");
    std::fs::create_dir(&root).unwrap();
    for file in ["a.mkv", "b.mkv", "c.mkv", "d.mkv"] {
        std::fs::write(root.join(file), b"x").unwrap();
    }
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming","label":"Incoming","container_path":root,"access":"read-only"
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
        mediaflow_core::bootstrap::config::RunMode::Development,
    )
    .unwrap();
    let inner: Arc<dyn CapabilityFs> = Arc::new(
        OsCapabilityFs::open(
            roots.declarations(),
            mediaflow_core::bootstrap::config::RunMode::Development,
        )
        .unwrap(),
    );
    let fs = Arc::new(RootReplacementFs {
        inner,
        root,
        previous_root,
        kind,
        replaced: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        calls_after_detection: Arc::new(AtomicUsize::new(0)),
        active_streams: Arc::new(AtomicUsize::new(0)),
        detected,
        release,
    });
    let discovery = DiscoveryService::new(roots.view_map(), fs.clone(), db.pool().clone());
    let inbox = discovery
        .create_inbox(CreateInboxCommand {
            root_id: "incoming".to_owned(),
            relative_path: ".".to_owned(),
        })
        .await
        .unwrap();
    let store = TaskStore::new(db.pool().clone());
    let task = store
        .create(NewScanTask {
            account_id,
            inbox_directory_id: inbox.id,
            idempotency_key: "real-root-replacement-create".to_owned(),
            now_us: 1_000_000,
        })
        .await
        .unwrap();
    let worker = ScanWorker::new(
        "real-root-replacement-worker".to_owned(),
        store.clone(),
        discovery,
        Arc::new(BoundedScanner::default()),
        Arc::new(ManualTaskClock::new(2_000_000)),
    );
    (fixture, db, account_id, store, task, worker, fs)
}

async fn assert_real_root_failure(
    account_id: Uuid,
    db: &mediaflow_core::platform::db::Db,
    store: &TaskStore,
    task_id: Uuid,
    fs: &RootReplacementFs,
) {
    let finished = store.get(account_id, task_id).await.unwrap();
    assert_eq!(finished.status, ScanStatus::Failed);
    assert_eq!(finished.counts.errors, 1);
    let errors = store
        .list_errors(
            account_id,
            task_id,
            &PageRequest::new(None, Some(10)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(errors.items.len(), 1);
    assert_eq!(errors.items[0].code, "root.unavailable");
    assert_eq!(errors.items[0].relative_path, ".");
    let root_error = sqlx::query_as::<_, (String, Vec<u8>, String)>(
        "SELECT scope,relative_path_bytes,relative_path_display
         FROM tasks_scan_errors WHERE task_id=?",
    )
    .bind(task_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        root_error,
        ("root".to_owned(), b".".to_vec(), ".".to_owned())
    );
    assert_eq!(
        fs.calls_after_detection.load(Ordering::SeqCst),
        0,
        "the first root revalidation failure must stop all later filesystem access"
    );
    assert_eq!(
        fs.active_streams.load(Ordering::SeqCst),
        0,
        "the real filesystem producer must be joined before worker return"
    );
}
