use super::*;
use crate::discovery::capability::DirectoryEntryStream;
use crate::discovery::model::{
    DirectoryEntry, DirectoryIdentity, EntryKind, EntryMetadata, FileIdentity, RelativePath,
    RootId, RootIdentity,
};
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct ChildGate {
    open: Mutex<bool>,
    ready: Condvar,
}

impl ChildGate {
    fn wait(&self) {
        let mut open = self.open.lock().unwrap();
        while !*open {
            open = self.ready.wait(open).unwrap();
        }
    }
    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.ready.notify_all();
    }
}

struct HighFanoutFs {
    root: DirectoryCapability,
    root_thread: Mutex<Option<std::thread::ThreadId>>,
    children: Arc<ChildGate>,
    inline_reached: Arc<Notify>,
    channel_full: Arc<Notify>,
    metadata_calls: AtomicUsize,
}

impl CapabilityFs for HighFanoutFs {
    fn preflight_directory(
        &self,
        _: &RootId,
        _: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        Ok(DirectoryIdentity::new(self.root.clone()))
    }
    fn read_directory(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        if capability.raw_relative_path() != b"." {
            return Ok(Vec::new());
        }
        let mut names: Vec<_> = (0..1201).map(|i| format!("file-{i}.mkv").into()).collect();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            names.push(std::ffi::OsString::from_vec(b"raw-\xff.mkv".to_vec()));
        }
        names.push("skip-link".into());
        names.extend((0..130).map(|i| format!("dir-{i}").into()));
        Ok(names.into_iter().map(DirectoryEntry::new).collect())
    }
    fn open_directory_stream(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Box<dyn DirectoryEntryStream>, FsBoundaryError> {
        if capability.raw_relative_path() == b"." {
            *self.root_thread.lock().unwrap() = Some(std::thread::current().id());
        } else {
            if *self.root_thread.lock().unwrap() == Some(std::thread::current().id()) {
                self.inline_reached.notify_one();
            }
            self.children.wait();
        }
        Ok(Box::new(Entries(
            self.read_directory(capability)?.into_iter(),
        )))
    }
    fn open_child_directory(
        &self,
        parent: &DirectoryCapability,
        name: &OsStr,
    ) -> Result<DirectoryCapability, FsBoundaryError> {
        let relative = RelativePath::parse(name.to_str().unwrap())
            .map_err(|_| FsBoundaryError::PathInvalid)?;
        let fd = rustix::io::dup(parent.verify(&[0; 16])?.fd).unwrap();
        Ok(DirectoryCapability::issue_child(
            parent,
            relative,
            os_bytes(name).to_vec(),
            identity(),
            fd,
        ))
    }
    fn metadata_no_follow(
        &self,
        _: &DirectoryCapability,
        name: &OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        let calls = self.metadata_calls.fetch_add(1, Ordering::SeqCst) + 1;
        // 首批128消息含根目录；缓冲1000消息后，下一文件的发送必须等待接收端。
        if calls == TRAVERSAL_CHECKPOINT_INTERVAL + OBSERVATION_CHANNEL_CAPACITY {
            self.channel_full.notify_one();
        }
        let kind = if name == "skip-link" {
            EntryKind::Symlink
        } else if name.to_string_lossy().starts_with("dir-") {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        Ok(EntryMetadata::new(kind, identity(), 1, 1))
    }
}

struct Entries(std::vec::IntoIter<DirectoryEntry>);
impl DirectoryEntryStream for Entries {
    fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, FsBoundaryError> {
        Ok(self.0.next())
    }
}
fn identity() -> FileIdentity {
    FileIdentity {
        device: 1,
        inode: 1,
        mount_id: None,
    }
}

#[derive(Default)]
struct HeldSink {
    batches: Mutex<Vec<Vec<FileObservation>>>,
    calls: AtomicUsize,
    started: Notify,
    release: Notify,
}
#[async_trait]
impl ObservationSink for HeldSink {
    async fn write_batch(
        &self,
        files: Vec<FileObservation>,
        errors: Vec<ScanEntryError>,
        _: ScanCounts,
    ) -> Result<(), AppError> {
        assert!(files.len() + errors.len() <= DATABASE_BATCH_SIZE);
        self.batches.lock().unwrap().push(files);
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.started.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

#[tokio::test]
async fn algorithm_high_fanout_finishes_after_channel_and_directory_queue_backpressure() {
    let temporary = tempfile::tempdir().unwrap();
    let capability = DirectoryCapability::issue(
        RootId::parse("test").unwrap(),
        RelativePath::parse(".").unwrap(),
        RootIdentity {
            device: 1,
            inode: 1,
            mount_id: None,
        },
        identity(),
        std::fs::File::open(temporary.path()).unwrap().into(),
        [0; 16],
    );
    let children = Arc::new(ChildGate::default());
    let inline_reached = Arc::new(Notify::new());
    let channel_full = Arc::new(Notify::new());
    let fs = Arc::new(HighFanoutFs {
        root: capability.clone(),
        root_thread: Mutex::new(None),
        children: children.clone(),
        inline_reached: inline_reached.clone(),
        channel_full: channel_full.clone(),
        metadata_calls: AtomicUsize::new(0),
    });
    let sink = Arc::new(HeldSink::default());
    let scan_sink = sink.clone();
    let scan_fs = fs.clone();
    let mut scan = tokio::spawn(async move {
        BoundedScanner::default()
            .scan(
                ScanSource {
                    inbox_directory_id: Uuid::now_v7(),
                    capability,
                    fs: scan_fs,
                },
                scan_sink,
                ScanStopToken::default(),
            )
            .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    let reached = tokio::time::timeout_at(deadline, async {
        sink.started.notified().await;
        channel_full.notified().await;
        let bounded = fs.metadata_calls.load(Ordering::SeqCst) < 1201;
        sink.release.notify_one();
        inline_reached.notified().await;
        bounded
    })
    .await;
    // 所有断言路径都先释放人为阻塞，避免失败后遗留阻塞工作器。
    sink.release.notify_one();
    children.release();
    let result = tokio::time::timeout_at(deadline, &mut scan).await;
    if result.is_err() {
        scan.abort();
    }
    assert_eq!(
        reached,
        Ok(true),
        "algorithm did not reach both bounded backpressure checkpoints"
    );
    let summary = result.unwrap().unwrap().unwrap();
    assert_eq!(summary.observed_files, 1201 + u64::from(cfg!(unix)));
    assert_eq!(summary.visited_directories, 131);
    assert_eq!(summary.skipped_entries, 1);
    assert_eq!(summary.errors, 0);
    #[cfg(unix)]
    assert!(
        sink.batches
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .any(|file| file.relative_path_bytes == b"raw-\xff.mkv"
                && file.relative_path_display.as_bytes() != file.relative_path_bytes)
    );
}
