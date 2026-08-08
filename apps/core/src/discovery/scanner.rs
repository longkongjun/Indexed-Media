#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use async_trait::async_trait;
use tokio::sync::{Notify, mpsc};
use uuid::Uuid;

use crate::discovery::capability::CapabilityFs;
use crate::discovery::model::{DirectoryCapability, FsBoundaryError};
use crate::discovery::observations::{FileObservation, ObservationSink, ScanEntryError};
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::model::ScanCounts;

/// 默认扫描器使用的阻塞目录工作器最大数量。
pub const DEFAULT_DIRECTORY_CONCURRENCY: usize = 4;
/// 生产者开始施加背压前保留的待处理目录最大数量。
pub const DIRECTORY_QUEUE_CAPACITY: usize = 64;
/// 阻塞工作器与异步接收循环之间缓冲的遍历消息最大数量。
pub const OBSERVATION_CHANNEL_CAPACITY: usize = 1_000;
/// 单次接收端写入中合计文件/错误观测结果的最大数量。
pub const DATABASE_BATCH_SIZE: usize = 500;
/// 即使没有观测结果，也会在检查点化计数前处理的遍历消息数量。
pub const TRAVERSAL_CHECKPOINT_INTERVAL: usize = 128;

/// 已重新验证的收件箱能力及其签发的文件系统适配器。
///
/// `capability` 必须由 `fs` 为 `inbox_directory_id` 签发；混用适配器会通过来源验证而失败关闭。
pub struct ScanSource {
    /// 接收文件观测结果的持久化收件箱。
    pub inbox_directory_id: Uuid,
    /// 用于遍历的开放起始目录。
    pub capability: DirectoryCapability,
    /// 签发且可重新验证该能力的适配器。
    pub fs: Arc<dyn CapabilityFs>,
}

#[async_trait]
/// 通过接收端检查点化观测结果的受能力约束递归扫描器。
pub trait Scanner: Send + Sync {
    /// 遍历 `source`，写入有界观测批次，并返回累计计数。
    ///
    /// 调用 [`ScanStopToken::stop`] 会请求协作式取消；已检查点化的批次仍保持已提交。
    /// 单项失败会增加计数并写入接收端；根能力失败会中止扫描。
    ///
    /// # Errors
    ///
    /// 当发生致命的文件系统边界失败、接收端/租约失败、阻塞工作器 panic，或在任何检查点
    /// 建立进度前取消时，返回 [`AppError`]。
    async fn scan(
        &self,
        source: ScanSource,
        sink: Arc<dyn ObservationSink>,
        stop: ScanStopToken,
    ) -> Result<ScanCounts, AppError>;
}

#[derive(Clone, Default)]
/// 扫描器生产者与消费者共享的、可克隆的单向协作式取消信号。
pub struct ScanStopToken {
    stopped: Arc<AtomicBool>,
    notified: Arc<Notify>,
}

impl ScanStopToken {
    /// 将令牌标记为已停止并唤醒当前等待者；重复调用没有效果。
    pub fn stop(&self) {
        if !self.stopped.swap(true, Ordering::AcqRel) {
            self.notified.notify_waiters();
        }
    }

    #[must_use]
    /// 返回是否已请求取消。
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    async fn stopped(&self) {
        if self.is_stopped() {
            return;
        }
        self.notified.notified().await;
    }
}

#[derive(Clone, Debug)]
/// 具有有界目录工作、观测缓冲与接收端批次的扫描器实现。
///
/// 默认使用 [`DEFAULT_DIRECTORY_CONCURRENCY`]；扫描开始时，内部覆盖值会被限制在
/// `1..=DEFAULT_DIRECTORY_CONCURRENCY` 范围内。
pub struct BoundedScanner {
    directory_concurrency: usize,
}

impl Default for BoundedScanner {
    fn default() -> Self {
        Self {
            directory_concurrency: DEFAULT_DIRECTORY_CONCURRENCY,
        }
    }
}

enum ScanMessage {
    VisitedDirectory,
    SkippedEntry,
    File(FileObservation),
    Error(ScanEntryError),
    Fatal(FsBoundaryError),
}

struct WorkItem {
    capability: DirectoryCapability,
    root: bool,
}

struct WorkState {
    queue: VecDeque<WorkItem>,
    outstanding: usize,
    stopped: bool,
}

#[async_trait]
impl Scanner for BoundedScanner {
    async fn scan(
        &self,
        source: ScanSource,
        sink: Arc<dyn ObservationSink>,
        stop: ScanStopToken,
    ) -> Result<ScanCounts, AppError> {
        let (sender, mut receiver) = mpsc::channel(OBSERVATION_CHANNEL_CAPACITY);
        let concurrency = self
            .directory_concurrency
            .clamp(1, DEFAULT_DIRECTORY_CONCURRENCY);
        let producer_stop = stop.clone();
        let producer = tokio::task::spawn_blocking(move || {
            run_workers(source, sender, producer_stop, concurrency);
        });
        let mut files = Vec::with_capacity(DATABASE_BATCH_SIZE);
        let mut errors = Vec::new();
        let mut counts = ScanCounts::default();
        let mut events_since_checkpoint = 0_usize;
        let mut fatal = None;
        let mut sink_error = None;
        loop {
            let message = tokio::select! {
                biased;
                () = stop.stopped() => None,
                message = receiver.recv() => message,
            };
            let Some(message) = message else {
                break;
            };
            match message {
                ScanMessage::VisitedDirectory => counts.visited_directories += 1,
                ScanMessage::SkippedEntry => counts.skipped_entries += 1,
                ScanMessage::File(file) => {
                    counts.observed_files += 1;
                    files.push(file);
                }
                ScanMessage::Error(error) => {
                    counts.errors += 1;
                    errors.push(error);
                }
                ScanMessage::Fatal(error) => {
                    fatal = Some(error);
                    stop.stop();
                }
            }
            events_since_checkpoint += 1;
            if fatal.is_some()
                || files.len() + errors.len() >= DATABASE_BATCH_SIZE
                || events_since_checkpoint >= TRAVERSAL_CHECKPOINT_INTERVAL
            {
                if let Err(error) = sink
                    .write_batch(
                        std::mem::take(&mut files),
                        std::mem::take(&mut errors),
                        counts,
                    )
                    .await
                {
                    sink_error = Some(error);
                    stop.stop();
                    break;
                }
                events_since_checkpoint = 0;
            }
            if fatal.is_some() {
                break;
            }
        }
        stop.stop();
        drop(receiver);
        producer
            .await
            .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?;
        if let Some(error) = fatal
            && sink_error
                .as_ref()
                .is_some_and(|error| error.code() == ErrorCode::TaskInvalidState)
        {
            return Err(boundary_error(error));
        }
        if let Some(error) = sink_error {
            return Err(error);
        }
        if fatal.is_none()
            && (!files.is_empty() || !errors.is_empty() || events_since_checkpoint > 0)
        {
            sink.write_batch(files, errors, counts).await?;
        }
        if let Some(error) = fatal {
            return Err(boundary_error(error));
        }
        if stop.is_stopped() && events_since_checkpoint == 0 && counts == ScanCounts::default() {
            return Err(AppError::new(ErrorCode::TaskLeaseLost, "scan stopped"));
        }
        Ok(counts)
    }
}

fn run_workers(
    source: ScanSource,
    sender: mpsc::Sender<ScanMessage>,
    stop: ScanStopToken,
    concurrency: usize,
) {
    let inbox_directory_id = source.inbox_directory_id;
    let state = Arc::new((
        Mutex::new(WorkState {
            queue: VecDeque::from([WorkItem {
                capability: source.capability,
                root: true,
            }]),
            outstanding: 1,
            stopped: false,
        }),
        Condvar::new(),
    ));
    let fatal = Arc::new(AtomicBool::new(false));
    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            let state = Arc::clone(&state);
            let fs = Arc::clone(&source.fs);
            let sender = sender.clone();
            let stop = stop.clone();
            let fatal = Arc::clone(&fatal);
            scope.spawn(move || {
                worker_loop(&state, &*fs, &sender, &stop, &fatal, inbox_directory_id);
            });
        }
    });
}

fn worker_loop(
    shared: &(Mutex<WorkState>, Condvar),
    fs: &dyn CapabilityFs,
    sender: &mpsc::Sender<ScanMessage>,
    stop: &ScanStopToken,
    fatal: &AtomicBool,
    inbox_directory_id: Uuid,
) {
    loop {
        let item = {
            let (lock, ready) = shared;
            let mut state = lock.lock().expect("scan queue lock");
            loop {
                if stop.is_stopped() || state.stopped || state.outstanding == 0 {
                    state.stopped = true;
                    state.queue.clear();
                    ready.notify_all();
                    return;
                }
                if let Some(item) = state.queue.pop_front() {
                    break item;
                }
                state = ready.wait(state).expect("scan queue wait");
            }
        };
        process_directory(item, shared, fs, sender, stop, fatal, inbox_directory_id);
        let (lock, ready) = shared;
        let mut state = lock.lock().expect("scan queue lock");
        state.outstanding = state.outstanding.saturating_sub(1);
        ready.notify_all();
    }
}

fn process_directory(
    item: WorkItem,
    shared: &(Mutex<WorkState>, Condvar),
    fs: &dyn CapabilityFs,
    sender: &mpsc::Sender<ScanMessage>,
    stop: &ScanStopToken,
    fatal: &AtomicBool,
    inbox_directory_id: Uuid,
) {
    if stop.is_stopped() || fatal.load(Ordering::Acquire) {
        return;
    }
    if !send_message(shared, sender, stop, ScanMessage::VisitedDirectory) {
        return;
    }
    let mut stream = match fs.open_directory_stream(&item.capability) {
        Ok(stream) => stream,
        Err(error) => {
            if item.root || is_fatal(error) {
                stop_with_fatal(shared, sender, stop, fatal, error);
            } else {
                send_entry_error(shared, sender, stop, &item.capability, error, "directory");
            }
            return;
        }
    };
    loop {
        if stop.is_stopped() || fatal.load(Ordering::Acquire) {
            return;
        }
        let entry = match stream.next_entry() {
            Ok(Some(entry)) => entry,
            Ok(None) => return,
            Err(error) => {
                if item.root || is_fatal(error) {
                    stop_with_fatal(shared, sender, stop, fatal, error);
                } else {
                    send_entry_error(shared, sender, stop, &item.capability, error, "directory");
                }
                return;
            }
        };
        let path = child_path(&item.capability, entry.name());
        let metadata = match fs.metadata_no_follow(&item.capability, entry.name()) {
            Ok(metadata) => metadata,
            Err(error) => {
                if is_fatal(error) {
                    stop_with_fatal(shared, sender, stop, fatal, error);
                    return;
                }
                if !send_message(
                    shared,
                    sender,
                    stop,
                    ScanMessage::Error(ScanEntryError {
                        code: error_code(error),
                        scope: "entry",
                        relative_path_bytes: path.0,
                        relative_path_display: path.1,
                    }),
                ) {
                    return;
                }
                continue;
            }
        };
        if metadata.is_symlink() || (!metadata.is_file() && !metadata.is_directory()) {
            if !send_message(shared, sender, stop, ScanMessage::SkippedEntry) {
                return;
            }
            continue;
        }
        if metadata.is_file() {
            let modified_at_ns = i64::try_from(metadata.modified_at_ns).unwrap_or_else(|_| {
                if metadata.modified_at_ns.is_negative() {
                    i64::MIN
                } else {
                    i64::MAX
                }
            });
            if !send_message(
                shared,
                sender,
                stop,
                ScanMessage::File(FileObservation {
                    inbox_directory_id,
                    relative_path_bytes: path.0,
                    relative_path_display: path.1,
                    identity_snapshot: metadata.identity.snapshot_bytes(),
                    size_bytes: metadata.size,
                    modified_at_ns,
                }),
            ) {
                return;
            }
            continue;
        }
        match fs.open_child_directory(&item.capability, entry.name()) {
            Ok(capability) => enqueue_or_process_inline(
                WorkItem {
                    capability,
                    root: false,
                },
                shared,
                fs,
                sender,
                stop,
                fatal,
                inbox_directory_id,
            ),
            Err(FsBoundaryError::SymlinkForbidden) => {
                if !send_message(shared, sender, stop, ScanMessage::SkippedEntry) {
                    return;
                }
            }
            Err(error) if is_fatal(error) => {
                stop_with_fatal(shared, sender, stop, fatal, error);
                return;
            }
            Err(error) => {
                if !send_message(
                    shared,
                    sender,
                    stop,
                    ScanMessage::Error(ScanEntryError {
                        code: error_code(error),
                        scope: "directory",
                        relative_path_bytes: path.0,
                        relative_path_display: path.1,
                    }),
                ) {
                    return;
                }
            }
        }
    }
}

fn enqueue_or_process_inline(
    item: WorkItem,
    shared: &(Mutex<WorkState>, Condvar),
    fs: &dyn CapabilityFs,
    sender: &mpsc::Sender<ScanMessage>,
    stop: &ScanStopToken,
    fatal: &AtomicBool,
    inbox_directory_id: Uuid,
) {
    let (lock, ready) = shared;
    let mut state = lock.lock().expect("scan queue lock");
    if state.queue.len() < DIRECTORY_QUEUE_CAPACITY {
        state.queue.push_back(item);
        state.outstanding += 1;
        ready.notify_one();
        return;
    }
    drop(state);
    process_directory(item, shared, fs, sender, stop, fatal, inbox_directory_id);
}

fn stop_with_fatal(
    shared: &(Mutex<WorkState>, Condvar),
    sender: &mpsc::Sender<ScanMessage>,
    stop: &ScanStopToken,
    fatal: &AtomicBool,
    error: FsBoundaryError,
) {
    if !fatal.swap(true, Ordering::AcqRel) {
        let _ = send_message(shared, sender, stop, ScanMessage::Fatal(error));
    }
    let (lock, ready) = shared;
    let mut state = lock.lock().expect("scan queue lock");
    state.stopped = true;
    state.queue.clear();
    ready.notify_all();
}

fn send_entry_error(
    shared: &(Mutex<WorkState>, Condvar),
    sender: &mpsc::Sender<ScanMessage>,
    stop: &ScanStopToken,
    capability: &DirectoryCapability,
    error: FsBoundaryError,
    scope: &'static str,
) {
    let _ = send_message(
        shared,
        sender,
        stop,
        ScanMessage::Error(ScanEntryError {
            code: error_code(error),
            scope,
            relative_path_bytes: capability.raw_relative_path().to_vec(),
            relative_path_display: capability.relative_path_display().to_owned(),
        }),
    );
}

fn send_message(
    shared: &(Mutex<WorkState>, Condvar),
    sender: &mpsc::Sender<ScanMessage>,
    stop: &ScanStopToken,
    message: ScanMessage,
) -> bool {
    if stop.is_stopped() {
        return false;
    }
    if sender.blocking_send(message).is_ok() {
        return true;
    }
    stop.stop();
    let (lock, ready) = shared;
    let mut state = lock.lock().expect("scan queue lock");
    state.stopped = true;
    state.queue.clear();
    ready.notify_all();
    false
}

fn child_path(capability: &DirectoryCapability, name: &OsStr) -> (Vec<u8>, String) {
    let name_bytes = os_bytes(name);
    let mut raw = if capability.raw_relative_path() == b"." {
        Vec::new()
    } else {
        capability.raw_relative_path().to_vec()
    };
    if !raw.is_empty() {
        raw.push(b'/');
    }
    raw.extend_from_slice(name_bytes);
    let mut display = if capability.relative_path_display() == "." {
        String::new()
    } else {
        capability.relative_path_display().to_owned()
    };
    if !display.is_empty() {
        display.push('/');
    }
    display.extend(
        String::from_utf8_lossy(name_bytes)
            .chars()
            .map(|character| {
                if character.is_control() || matches!(character, '/' | '\\') {
                    '\u{fffd}'
                } else {
                    character
                }
            }),
    );
    (raw, display)
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes()
}

#[cfg(not(unix))]
fn os_bytes(value: &OsStr) -> &[u8] {
    value.to_str().unwrap_or("[UNREPRESENTABLE]").as_bytes()
}

fn is_fatal(error: FsBoundaryError) -> bool {
    matches!(
        error,
        FsBoundaryError::RootChanged | FsBoundaryError::PathEscape | FsBoundaryError::PathInvalid
    )
}

fn error_code(error: FsBoundaryError) -> &'static str {
    match error {
        FsBoundaryError::RootNotFound => "root.not_found",
        FsBoundaryError::Unavailable => "entry.unavailable",
        FsBoundaryError::RootChanged => "root.unavailable",
        FsBoundaryError::PathInvalid => "path.invalid",
        FsBoundaryError::PathEscape => "path.escape",
        FsBoundaryError::SymlinkForbidden => "path.symlink_forbidden",
        FsBoundaryError::Internal => "internal.error",
    }
}

fn boundary_error(error: FsBoundaryError) -> AppError {
    let code = match error {
        FsBoundaryError::RootNotFound => ErrorCode::RootNotFound,
        FsBoundaryError::Unavailable | FsBoundaryError::RootChanged => ErrorCode::RootUnavailable,
        FsBoundaryError::PathInvalid => ErrorCode::PathInvalid,
        FsBoundaryError::PathEscape => ErrorCode::PathEscape,
        FsBoundaryError::SymlinkForbidden => ErrorCode::PathSymlinkForbidden,
        FsBoundaryError::Internal => ErrorCode::Internal,
    };
    AppError::new(code, "scan capability boundary failed")
}
