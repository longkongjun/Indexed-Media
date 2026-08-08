#![cfg(unix)]

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::capability::DeploymentRootSet;
use mediaflow_core::discovery::model::{RelativePath, RootId};
use mediaflow_core::organization::fs::{
    CompensationOutcome, FileLocator, FileOperationError, FileOperationSpec, NfoOperationSpec,
    OrganizationFs, ProcessingStopToken, VerifiedOperation, VerifiedOperationKind,
};
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

#[test]
fn copy_streams_and_publishes_without_clobbering() {
    let fixture = Fixture::new("read-only");
    let bytes = vec![0x5a; 256 * 1024 + 17];
    fixture.write_source("ready/source.mkv", &bytes);
    fixture.create_target_directory("Movies/Arrival (2016)");
    let source = fixture.source("ready/source.mkv");
    let destination = fixture.target("Movies/Arrival (2016)/Arrival (2016).mkv");
    let observed = fixture.fs.observe(&source).unwrap().unwrap();
    let operation = FileOperationSpec::new(Uuid::now_v7(), observed, destination.clone());

    let applied = fixture
        .fs
        .copy_no_clobber(&operation, &ProcessingStopToken::default())
        .unwrap();

    assert_eq!(
        std::fs::read(fixture.target_path(&destination)).unwrap(),
        bytes
    );
    assert_eq!(applied.size_bytes(), bytes.len() as u64);
    assert_eq!(applied.sha256(), Sha256::digest(&bytes).as_slice());
    assert!(fixture.source_path(&source).exists());
    assert_eq!(fixture.operation_temps(operation.id()).count(), 0);

    let stopped_destination = fixture.target("Movies/stopped.mkv");
    let stopped = ProcessingStopToken::default();
    stopped.stop();
    let stopped_operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        stopped_destination.clone(),
    );
    assert_eq!(
        fixture
            .fs
            .copy_no_clobber(&stopped_operation, &stopped)
            .unwrap_err(),
        FileOperationError::IoTemporary
    );
    assert!(!fixture.target_path(&stopped_destination).exists());

    let original = b"do-not-overwrite";
    std::fs::write(fixture.target_path(&destination), original).unwrap();
    let retry = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        destination.clone(),
    );
    assert_eq!(
        fixture
            .fs
            .copy_no_clobber(&retry, &ProcessingStopToken::default())
            .unwrap_err(),
        FileOperationError::TargetExists
    );
    assert_eq!(
        std::fs::read(fixture.target_path(&destination)).unwrap(),
        original
    );

    fixture.write_source("ready/changed.mkv", b"before");
    let changed_source = fixture.source("ready/changed.mkv");
    let stale = fixture.fs.observe(&changed_source).unwrap().unwrap();
    fixture.write_source("ready/changed.mkv", b"after-and-longer");
    let changed_destination = fixture.target("Movies/changed-source.mkv");
    let changed = FileOperationSpec::new(Uuid::now_v7(), stale, changed_destination.clone());
    assert_eq!(
        fixture
            .fs
            .copy_no_clobber(&changed, &ProcessingStopToken::default())
            .unwrap_err(),
        FileOperationError::SourceChanged
    );
    assert!(!fixture.target_path(&changed_destination).exists());
}

#[test]
fn same_device_move_is_no_clobber_and_preserves_identity() {
    let fixture = Fixture::new("read-write");
    fixture.write_source("ready/source.mkv", b"move-me");
    fixture.create_target_directory("Movies");
    let source = fixture.source("ready/source.mkv");
    let destination = fixture.target("Movies/moved.mkv");
    let source_inode = std::fs::metadata(fixture.source_path(&source))
        .unwrap()
        .ino();
    let operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        destination.clone(),
    );

    let applied = fixture.fs.move_no_clobber(&operation).unwrap();

    assert!(!fixture.source_path(&source).exists());
    assert_eq!(
        std::fs::read(fixture.target_path(&destination)).unwrap(),
        b"move-me"
    );

    let read_only = Fixture::new("read-only");
    read_only.write_source("source.mkv", b"must-remain");
    let read_only_source = read_only.source("source.mkv");
    let read_only_destination = read_only.target("moved.mkv");
    let forbidden = FileOperationSpec::new(
        Uuid::now_v7(),
        read_only.fs.observe(&read_only_source).unwrap().unwrap(),
        read_only_destination.clone(),
    );
    assert_eq!(
        read_only.fs.move_no_clobber(&forbidden).unwrap_err(),
        FileOperationError::OperationUnsupported
    );
    assert!(read_only.source_path(&read_only_source).exists());
    assert!(!read_only.target_path(&read_only_destination).exists());
    assert_eq!(applied.identity().inode, source_inode);

    fixture.write_source("ready/second.mkv", b"keep-source");
    let second = fixture.source("ready/second.mkv");
    let collision = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&second).unwrap().unwrap(),
        destination.clone(),
    );
    assert_eq!(
        fixture.fs.move_no_clobber(&collision).unwrap_err(),
        FileOperationError::TargetExists
    );
    assert_eq!(
        std::fs::read(fixture.source_path(&second)).unwrap(),
        b"keep-source"
    );
    assert_eq!(
        std::fs::read(fixture.target_path(&destination)).unwrap(),
        b"move-me"
    );
}

#[test]
fn cross_device_move_is_reported_for_composite_execution() {
    let Some(other_root) = std::env::var_os("MEDIAFLOW_TEST_CROSS_DEVICE_ROOT") else {
        eprintln!("SKIPPED: MEDIAFLOW_TEST_CROSS_DEVICE_ROOT is not configured");
        return;
    };
    let source_temp = tempfile::tempdir().unwrap();
    let target_temp = tempfile::tempdir_in(other_root).unwrap();
    assert_ne!(
        std::fs::metadata(source_temp.path()).unwrap().dev(),
        std::fs::metadata(target_temp.path()).unwrap().dev(),
        "the configured root must be on another device"
    );
    let fixture = Fixture::with_roots(source_temp.path(), target_temp.path(), "read-write");
    fixture.write_source("source.mkv", b"cross-device");
    let source = fixture.source("source.mkv");
    let destination = fixture.target("destination.mkv");
    let operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        destination.clone(),
    );

    assert_eq!(
        fixture.fs.move_no_clobber(&operation).unwrap_err(),
        FileOperationError::CompositeMoveRequired
    );
    assert!(fixture.source_path(&source).exists());
    assert!(!fixture.target_path(&destination).exists());
}

#[test]
fn hardlink_requires_the_same_device_and_verifies_inode() {
    let fixture = Fixture::new("read-only");
    fixture.write_source("ready/source.mkv", b"linked");
    fixture.create_target_directory("Series");
    let source = fixture.source("ready/source.mkv");
    let destination = fixture.target("Series/linked.mkv");
    let operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        destination.clone(),
    );

    let applied = fixture.fs.hardlink_no_clobber(&operation).unwrap();

    let source_metadata = std::fs::metadata(fixture.source_path(&source)).unwrap();
    let target_metadata = std::fs::metadata(fixture.target_path(&destination)).unwrap();
    assert_eq!(source_metadata.ino(), target_metadata.ino());
    assert_eq!(applied.identity().inode, source_metadata.ino());
    assert_eq!(
        std::fs::read(fixture.target_path(&destination)).unwrap(),
        b"linked"
    );
}

#[test]
fn symlink_and_replaced_root_fail_before_side_effects() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("read-only");
    fixture.write_source("ready/source.mkv", b"inside");
    let outside = fixture.temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"outside").unwrap();
    symlink(&outside, fixture.target_root.join("escape")).unwrap();
    let source = fixture.source("ready/source.mkv");
    let escaped = fixture.target("escape/created.mkv");
    let operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        escaped,
    );

    assert_eq!(
        fixture
            .fs
            .copy_no_clobber(&operation, &ProcessingStopToken::default())
            .unwrap_err(),
        FileOperationError::PathOutsideRoot
    );
    assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"outside");
    assert!(!outside.join("created.mkv").exists());

    std::fs::write(fixture.target_root.join("not-a-directory"), b"file").unwrap();
    let invalid_parent = fixture.target("not-a-directory/created.mkv");
    let operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        invalid_parent.clone(),
    );
    assert_eq!(
        fixture
            .fs
            .copy_no_clobber(&operation, &ProcessingStopToken::default())
            .unwrap_err(),
        FileOperationError::PathOutsideRoot
    );
    assert!(!fixture.target_path(&invalid_parent).exists());

    let old_target = fixture.temp.path().join("old-target");
    std::fs::rename(&fixture.target_root, &old_target).unwrap();
    std::fs::create_dir(&fixture.target_root).unwrap();
    let replaced = fixture.target("replacement.mkv");
    let operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        replaced.clone(),
    );
    assert_eq!(
        fixture
            .fs
            .copy_no_clobber(&operation, &ProcessingStopToken::default())
            .unwrap_err(),
        FileOperationError::RootChanged
    );
    assert!(!fixture.target_path(&replaced).exists());
}

#[test]
fn compensation_removes_only_an_unchanged_created_target() {
    let fixture = Fixture::new("read-only");
    fixture.write_source("ready/source.mkv", b"copy-one");
    fixture.create_target_directory("Movies");
    let source = fixture.source("ready/source.mkv");
    let destination = fixture.target("Movies/copy.mkv");
    let operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture.fs.observe(&source).unwrap().unwrap(),
        destination.clone(),
    );
    let applied = fixture
        .fs
        .copy_no_clobber(&operation, &ProcessingStopToken::default())
        .unwrap();
    let verified = VerifiedOperation::new(
        operation.id(),
        VerifiedOperationKind::Copy,
        Some(source),
        applied,
    );

    assert_eq!(
        fixture.fs.compensate(&verified).unwrap(),
        CompensationOutcome::Removed
    );
    assert_eq!(
        fixture.fs.compensate(&verified).unwrap(),
        CompensationOutcome::AlreadyAbsent
    );

    let changed_destination = fixture.target("Movies/changed.mkv");
    let changed_operation = FileOperationSpec::new(
        Uuid::now_v7(),
        fixture
            .fs
            .observe(&fixture.source("ready/source.mkv"))
            .unwrap()
            .unwrap(),
        changed_destination.clone(),
    );
    let changed_applied = fixture
        .fs
        .copy_no_clobber(&changed_operation, &ProcessingStopToken::default())
        .unwrap();
    std::fs::write(
        fixture.target_path(&changed_destination),
        b"externally changed",
    )
    .unwrap();
    let changed = VerifiedOperation::new(
        changed_operation.id(),
        VerifiedOperationKind::Copy,
        None,
        changed_applied,
    );
    assert_eq!(
        fixture.fs.compensate(&changed).unwrap(),
        CompensationOutcome::ManualReview
    );
    assert_eq!(
        std::fs::read(fixture.target_path(&changed_destination)).unwrap(),
        b"externally changed"
    );
}

#[test]
fn nfo_publish_is_bounded_and_preserves_existing_bytes() {
    let fixture = Fixture::new("read-only");
    fixture.create_target_directory("Movies");
    let destination = fixture.target("Movies/movie.nfo");
    let operation = NfoOperationSpec::new(Uuid::now_v7(), destination.clone());
    let applied = fixture.fs.write_new_nfo(&operation, b"<movie/>").unwrap();
    assert_eq!(applied.size_bytes(), 8);
    assert_eq!(
        std::fs::read(fixture.target_path(&destination)).unwrap(),
        b"<movie/>"
    );

    let replacement = NfoOperationSpec::new(Uuid::now_v7(), destination.clone());
    assert_eq!(
        fixture
            .fs
            .write_new_nfo(&replacement, b"replacement")
            .unwrap_err(),
        FileOperationError::TargetExists
    );
    assert_eq!(
        std::fs::read(fixture.target_path(&destination)).unwrap(),
        b"<movie/>"
    );

    let oversized = NfoOperationSpec::new(Uuid::now_v7(), fixture.target("Movies/oversized.nfo"));
    assert_eq!(
        fixture
            .fs
            .write_new_nfo(&oversized, &vec![0; 1024 * 1024 + 1])
            .unwrap_err(),
        FileOperationError::OperationUnsupported
    );
}

struct Fixture {
    temp: tempfile::TempDir,
    source_root: PathBuf,
    target_root: PathBuf,
    source_id: RootId,
    target_id: RootId,
    fs: OsCapabilityFs,
}

impl Fixture {
    fn new(source_access: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let target_root = temp.path().join("target");
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::create_dir_all(&target_root).unwrap();
        let source_root = std::fs::canonicalize(source_root).unwrap();
        let target_root = std::fs::canonicalize(target_root).unwrap();
        Self::build(temp, source_root, target_root, source_access)
    }

    fn with_roots(source_root: &Path, target_root: &Path, source_access: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        Self::build(
            temp,
            std::fs::canonicalize(source_root).unwrap(),
            std::fs::canonicalize(target_root).unwrap(),
            source_access,
        )
    }

    fn build(
        temp: tempfile::TempDir,
        source_root: PathBuf,
        target_root: PathBuf,
        source_access: &str,
    ) -> Self {
        let config = temp.path().join("deployment-roots.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({"roots":[
                {"id":"incoming","label":"Incoming","container_path":source_root,"access":source_access},
                {"id":"library","label":"Library","container_path":target_root,"access":"read-write"}
            ]}))
            .unwrap(),
        )
        .unwrap();
        let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
        let fs = OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap();
        Self {
            temp,
            source_root,
            target_root,
            source_id: RootId::parse("incoming").unwrap(),
            target_id: RootId::parse("library").unwrap(),
            fs,
        }
    }

    fn source(&self, path: &str) -> FileLocator {
        FileLocator::new(self.source_id.clone(), RelativePath::parse(path).unwrap())
    }

    fn target(&self, path: &str) -> FileLocator {
        FileLocator::new(self.target_id.clone(), RelativePath::parse(path).unwrap())
    }

    fn source_path(&self, locator: &FileLocator) -> PathBuf {
        self.source_root.join(locator.relative_path().as_str())
    }

    fn target_path(&self, locator: &FileLocator) -> PathBuf {
        self.target_root.join(locator.relative_path().as_str())
    }

    fn write_source(&self, relative: &str, bytes: &[u8]) {
        let path = self.source_root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn create_target_directory(&self, relative: &str) {
        std::fs::create_dir_all(self.target_root.join(relative)).unwrap();
    }

    fn operation_temps(&self, operation_id: Uuid) -> impl Iterator<Item = PathBuf> + '_ {
        let needle = format!(".mediaflow-{operation_id}.tmp");
        walk(&self.target_root)
            .filter(move |path| path.file_name().is_some_and(|name| name == needle.as_str()))
    }
}

fn walk(root: &Path) -> Box<dyn Iterator<Item = PathBuf> + '_> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Box::new(std::iter::empty());
    };
    Box::new(entries.filter_map(Result::ok).flat_map(|entry| {
        let path = entry.path();
        if path.is_dir() {
            let mut paths = vec![path.clone()];
            paths.extend(walk(&path));
            paths.into_iter()
        } else {
            vec![path].into_iter()
        }
    }))
}
