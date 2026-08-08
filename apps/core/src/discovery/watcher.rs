use std::collections::BTreeMap;
use std::path::PathBuf;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::shared::error::{AppError, ErrorCode};

/// 相同收件箱/原始路径提示的合并窗口（2 秒）。
pub const COALESCE_WINDOW_US: i64 = 2_000_000;
/// 生产 watcher callback 与异步协调器之间的固定缓冲容量。
pub const WATCH_CHANNEL_CAPACITY: usize = 1_024;
/// 2 秒窗口内最多保留的唯一 `(inbox, raw path)` 数量。
pub const WATCH_COALESCER_CAPACITY: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
/// watcher callback 产生的无元数据路径提示。
///
/// 原始路径是唯一键；实际文件元数据只能在异步协调器后续通过能力文件系统重新读取。
pub struct WatchHint {
    /// 提示所属收件箱。
    pub inbox_directory_id: Uuid,
    /// 收件箱内保真的原始相对路径字节。
    pub relative_path_bytes: Vec<u8>,
    /// callback 观察到提示的 UTC 微秒。
    pub observed_at_us: i64,
}

impl WatchHint {
    #[must_use]
    /// 创建一个不包含文件元数据的 watcher 提示。
    pub const fn new(
        inbox_directory_id: Uuid,
        relative_path_bytes: Vec<u8>,
        observed_at_us: i64,
    ) -> Self {
        Self {
            inbox_directory_id,
            relative_path_bytes,
            observed_at_us,
        }
    }
}

#[derive(Clone)]
/// 供同步 watcher callback 使用的非阻塞、有界入口。
pub struct WatchIngress {
    sender: mpsc::Sender<WatchHint>,
    overflow: Arc<AtomicBool>,
}

impl WatchIngress {
    /// 创建给定容量的入口与唯一接收端。
    ///
    /// # Panics
    ///
    /// `capacity` 为零时 panic。
    #[must_use]
    pub fn bounded(capacity: usize) -> (Self, WatchReceiver) {
        assert!(capacity > 0, "watch channel capacity must be positive");
        let (sender, receiver) = mpsc::channel(capacity);
        let overflow = Arc::new(AtomicBool::new(false));
        (
            Self {
                sender,
                overflow: Arc::clone(&overflow),
            },
            WatchReceiver {
                receiver,
                overflow: Arc::clone(&overflow),
            },
        )
    }

    /// 非阻塞写入提示；缓冲已满/关闭时设置粘性 overflow 位并返回 `false`。
    #[must_use]
    pub fn try_send(&self, hint: WatchHint) -> bool {
        if self.sender.try_send(hint).is_ok() {
            true
        } else {
            self.mark_overflow();
            false
        }
    }

    /// 标记 callback 事件已不再可靠，要求完整对账。
    pub fn mark_overflow(&self) {
        self.overflow.store(true, Ordering::Release);
    }

    /// 原子取得并清除 overflow 信号。
    #[must_use]
    pub fn take_overflow(&self) -> bool {
        self.overflow.swap(false, Ordering::AcqRel)
    }
}

/// watcher 提示的唯一异步接收端，同时持有共享 overflow 信号。
pub struct WatchReceiver {
    receiver: mpsc::Receiver<WatchHint>,
    overflow: Arc<AtomicBool>,
}

impl WatchReceiver {
    /// 异步接收下一条提示；所有发送端关闭后返回 `None`。
    pub async fn recv(&mut self) -> Option<WatchHint> {
        self.receiver.recv().await
    }

    pub(crate) fn try_recv(&mut self) -> Result<WatchHint, mpsc::error::TryRecvError> {
        self.receiver.try_recv()
    }

    pub(crate) fn take_overflow(&self) -> bool {
        self.overflow.swap(false, Ordering::AcqRel)
    }
}

struct PendingHint {
    hint: WatchHint,
    first_received_at_us: i64,
}

/// 按 `(inbox, raw path)` 在固定 2 秒窗口内合并重复/乱序提示。
pub struct WatchCoalescer {
    pending: BTreeMap<(Uuid, Vec<u8>), PendingHint>,
    capacity: usize,
}

impl Default for WatchCoalescer {
    fn default() -> Self {
        Self::with_capacity(WATCH_COALESCER_CAPACITY)
    }
}

impl WatchCoalescer {
    /// 创建具有显式唯一路径硬上限的合并器。
    ///
    /// # Panics
    ///
    /// `capacity` 为零时 panic。
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "watch coalescer capacity must be positive");
        Self {
            pending: BTreeMap::new(),
            capacity,
        }
    }

    /// 合并一个提示；更旧事件不会让保留的观察时间倒退。
    ///
    /// 新唯一键在容量已满时返回 `false`；重复键仍可原地合并。
    #[must_use]
    pub fn push(&mut self, hint: WatchHint) -> bool {
        let key = (hint.inbox_directory_id, hint.relative_path_bytes.clone());
        if let Some(pending) = self.pending.get_mut(&key) {
            pending.hint.observed_at_us = pending.hint.observed_at_us.max(hint.observed_at_us);
            return true;
        }
        if self.pending.len() >= self.capacity {
            return false;
        }
        self.pending.insert(
            key,
            PendingHint {
                first_received_at_us: hint.observed_at_us,
                hint,
            },
        );
        true
    }

    /// 取出首次收到时间已达到 2 秒窗口的提示，保留尚未到期项。
    #[must_use]
    pub fn drain_ready(&mut self, now_us: i64) -> Vec<WatchHint> {
        let ready = self
            .pending
            .iter()
            .filter(|(_, pending)| {
                now_us
                    >= pending
                        .first_received_at_us
                        .saturating_add(COALESCE_WINDOW_US)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        ready
            .into_iter()
            .filter_map(|key| self.pending.remove(&key).map(|pending| pending.hint))
            .collect()
    }

    /// 返回当前唯一待处理路径数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// 返回当前是否没有待处理路径。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 丢弃所有不再可靠的逐路径提示；调用方必须安排完整对账。
    pub fn clear(&mut self) {
        self.pending.clear();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// watcher 绝对事件路径无法安全映射到收件箱的原因。
pub enum WatchMapError {
    /// 事件路径不在指定收件箱下，或含父级/绝对重解释组件。
    OutsideInbox,
    /// 事件指向收件箱根本身，应升级为完整对账而非单路径提示。
    InboxRoot,
}

/// 将绝对事件路径词法映射为保真的收件箱相对字节。
///
/// 此函数不读取元数据；映射成功也仅产生提示。后续扫描仍必须通过能力文件系统重新验证根与文件事实。
///
/// # Errors
///
/// 事件不在收件箱内、包含逃逸组件或指向收件箱根本身时返回 [`WatchMapError`]。
pub fn map_event_path(inbox_root: &Path, event_path: &Path) -> Result<Vec<u8>, WatchMapError> {
    let relative = event_path
        .strip_prefix(inbox_root)
        .map_err(|_| WatchMapError::OutsideInbox)?;
    if relative.as_os_str().is_empty() {
        return Err(WatchMapError::InboxRoot);
    }
    if relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(WatchMapError::OutsideInbox);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(relative.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        Ok(relative.to_string_lossy().as_bytes().to_vec())
    }
}

#[derive(Clone, Eq, PartialEq)]
/// 允许 watcher 观察的已验证收件箱宿主路径。
pub struct WatchedInbox {
    /// 收件箱稳定 ID。
    pub inbox_directory_id: Uuid,
    /// 仅传给本地 watcher、不进入日志/API 的宿主绝对路径。
    pub host_path: PathBuf,
}

impl std::fmt::Debug for WatchedInbox {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WatchedInbox")
            .field("inbox_directory_id", &self.inbox_directory_id)
            .field("host_path", &"[REDACTED]")
            .finish()
    }
}

/// 可替换收件箱集合的文件系统 watcher 端口。
pub trait FileWatch: Send {
    /// 使活动递归监听与 `targets` 完全一致。
    ///
    /// # Errors
    ///
    /// 目标不是稳定绝对目录，或底层 watcher 无法增删监听时返回 [`AppError`]。
    fn replace(&mut self, targets: Vec<WatchedInbox>) -> Result<(), AppError>;
}

/// 基于 `notify` 的生产 watcher；callback 只做路径映射和有界 `try_send`。
pub struct NotifyFileWatch {
    watcher: notify::RecommendedWatcher,
    mappings: Arc<RwLock<Vec<WatchedInbox>>>,
    watched: BTreeMap<Uuid, PathBuf>,
    ingress: WatchIngress,
}

impl NotifyFileWatch {
    /// 创建尚未监听任何路径的生产适配器。
    ///
    /// # Errors
    ///
    /// 操作系统 watcher 无法初始化时返回 [`AppError`]。
    pub fn new(ingress: WatchIngress) -> Result<Self, AppError> {
        use notify::Watcher as _;

        let mappings = Arc::new(RwLock::new(Vec::<WatchedInbox>::new()));
        let callback_mappings = Arc::clone(&mappings);
        let callback_ingress = ingress.clone();
        let watcher = notify::RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| match result {
                Ok(event) => {
                    let observed_at_us = chrono::Utc::now().timestamp_micros();
                    let Ok(mappings) = callback_mappings.read() else {
                        callback_ingress.mark_overflow();
                        return;
                    };
                    for path in event.paths {
                        let mapped = mappings.iter().find_map(|target| {
                            map_event_path(&target.host_path, &path).ok().map(
                                |relative_path_bytes| {
                                    WatchHint::new(
                                        target.inbox_directory_id,
                                        relative_path_bytes,
                                        observed_at_us,
                                    )
                                },
                            )
                        });
                        if let Some(hint) = mapped {
                            let _ = callback_ingress.try_send(hint);
                        } else {
                            callback_ingress.mark_overflow();
                        }
                    }
                }
                Err(_) => callback_ingress.mark_overflow(),
            },
            notify::Config::default(),
        )
        .map_err(watch_error)?;
        Ok(Self {
            watcher,
            mappings,
            watched: BTreeMap::new(),
            ingress,
        })
    }
}

impl FileWatch for NotifyFileWatch {
    fn replace(&mut self, mut targets: Vec<WatchedInbox>) -> Result<(), AppError> {
        use notify::Watcher as _;

        targets.sort_by_key(|target| target.inbox_directory_id);
        let desired = targets
            .iter()
            .map(|target| (target.inbox_directory_id, target.host_path.clone()))
            .collect::<BTreeMap<_, _>>();
        if desired == self.watched {
            return Ok(());
        }
        validate_targets(&targets)?;
        let removed = self
            .watched
            .iter()
            .filter(|(id, path)| desired.get(id).is_none_or(|desired| desired != *path))
            .map(|(id, path)| (*id, path.clone()))
            .collect::<Vec<_>>();
        for (id, path) in removed {
            self.watcher.unwatch(&path).map_err(|error| {
                self.ingress.mark_overflow();
                watch_error(error)
            })?;
            self.watched.remove(&id);
        }
        for target in &targets {
            if self.watched.get(&target.inbox_directory_id) == Some(&target.host_path) {
                continue;
            }
            self.watcher
                .watch(&target.host_path, notify::RecursiveMode::Recursive)
                .map_err(|error| {
                    self.ingress.mark_overflow();
                    watch_error(error)
                })?;
            self.watched
                .insert(target.inbox_directory_id, target.host_path.clone());
        }
        *self.mappings.write().map_err(|_| {
            AppError::new(ErrorCode::Internal, "watch mapping lock is unavailable")
        })? = targets;
        Ok(())
    }
}

fn validate_targets(targets: &[WatchedInbox]) -> Result<(), AppError> {
    let mut ids = BTreeMap::new();
    for target in targets {
        if !target.host_path.is_absolute()
            || ids
                .insert(target.inbox_directory_id, &target.host_path)
                .is_some()
        {
            return Err(AppError::new(
                ErrorCode::RootUnavailable,
                "watch target is invalid",
            ));
        }
        let metadata = std::fs::symlink_metadata(&target.host_path)
            .map_err(|error| AppError::with_source(ErrorCode::RootUnavailable, error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::new(
                ErrorCode::RootUnavailable,
                "watch target is not a stable directory",
            ));
        }
    }
    Ok(())
}

fn watch_error(error: notify::Error) -> AppError {
    AppError::with_source(ErrorCode::RootUnavailable, error)
}
