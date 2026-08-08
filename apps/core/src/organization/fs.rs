use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use uuid::Uuid;

use crate::discovery::model::{FileIdentity, FsBoundaryError, RelativePath, RootId, RootIdentity};

/// 部署根能力内的一个文件位置；不包含宿主绝对路径。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileLocator {
    root_id: RootId,
    relative_path: RelativePath,
}

impl FileLocator {
    #[must_use]
    /// 创建一个已经过词法校验的逻辑文件位置。
    pub const fn new(root_id: RootId, relative_path: RelativePath) -> Self {
        Self {
            root_id,
            relative_path,
        }
    }

    #[must_use]
    /// 返回逻辑部署根 ID。
    pub const fn root_id(&self) -> &RootId {
        &self.root_id
    }

    #[must_use]
    /// 返回根能力内的规范化相对路径。
    pub const fn relative_path(&self) -> &RelativePath {
        &self.relative_path
    }
}

/// 目标目录预检时捕获的根与目录身份。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetCapabilitySnapshot {
    /// 根目录身份。
    pub root_identity: RootIdentity,
    /// 目标目录身份。
    pub directory_identity: FileIdentity,
}

/// 执行前观察到的来源文件事实。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedFile {
    locator: FileLocator,
    identity: FileIdentity,
    size_bytes: u64,
    modified_at_ns: i128,
}

impl ObservedFile {
    #[must_use]
    /// 返回被观察文件的逻辑位置。
    pub const fn locator(&self) -> &FileLocator {
        &self.locator
    }

    #[must_use]
    /// 返回文件系统身份。
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    #[must_use]
    /// 返回观察到的文件长度。
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    /// 返回观察到的纳秒修改时间。
    pub const fn modified_at_ns(&self) -> i128 {
        self.modified_at_ns
    }

    /// 从可信适配器捕获的文件事实创建观察结果。
    #[must_use]
    pub const fn from_parts(
        locator: FileLocator,
        identity: FileIdentity,
        size_bytes: u64,
        modified_at_ns: i128,
    ) -> Self {
        Self {
            locator,
            identity,
            size_bytes,
            modified_at_ns,
        }
    }
}

/// 一次 copy、move 或 hardlink 的不可变执行输入。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOperationSpec {
    id: Uuid,
    source: ObservedFile,
    destination: FileLocator,
}

impl FileOperationSpec {
    #[must_use]
    /// 使用执行前来源快照创建操作。
    pub const fn new(id: Uuid, source: ObservedFile, destination: FileLocator) -> Self {
        Self {
            id,
            source,
            destination,
        }
    }

    #[must_use]
    /// 返回稳定 operation ID。
    pub const fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    /// 返回预期来源事实。
    pub const fn source(&self) -> &ObservedFile {
        &self.source
    }

    #[must_use]
    /// 返回目标逻辑位置。
    pub const fn destination(&self) -> &FileLocator {
        &self.destination
    }
}

/// 一次只创建缺失 NFO 的不可变执行输入。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NfoOperationSpec {
    id: Uuid,
    destination: FileLocator,
}

impl NfoOperationSpec {
    #[must_use]
    /// 创建一个 NFO 发布操作。
    pub const fn new(id: Uuid, destination: FileLocator) -> Self {
        Self { id, destination }
    }

    #[must_use]
    /// 返回稳定 operation ID。
    pub const fn id(&self) -> Uuid {
        self.id
    }

    #[must_use]
    /// 返回 NFO 目标逻辑位置。
    pub const fn destination(&self) -> &FileLocator {
        &self.destination
    }
}

/// 一个已发布文件的核对事实。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedFile {
    locator: FileLocator,
    identity: FileIdentity,
    size_bytes: u64,
    sha256: [u8; 32],
}

impl AppliedFile {
    #[must_use]
    /// 返回已发布文件位置。
    pub const fn locator(&self) -> &FileLocator {
        &self.locator
    }

    #[must_use]
    /// 返回发布后的文件系统身份。
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    #[must_use]
    /// 返回发布后的长度。
    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    #[must_use]
    /// 返回流式核对得到的 SHA-256。
    pub const fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    /// 从已发布并核对的文件事实创建结果。
    #[must_use]
    pub const fn from_parts(
        locator: FileLocator,
        identity: FileIdentity,
        size_bytes: u64,
        sha256: [u8; 32],
    ) -> Self {
        Self {
            locator,
            identity,
            size_bytes,
            sha256,
        }
    }
}

/// 已验证 operation 的文件语义，用于受控补偿。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerifiedOperationKind {
    /// 来源保持不变的复制。
    Copy,
    /// 来源被原子重命名到目标。
    Move,
    /// 来源保持不变的硬链接。
    Hardlink,
    /// 只创建缺失 NFO。
    Nfo,
}

/// 已完成核对、可供补偿重新观察的 operation。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedOperation {
    id: Uuid,
    kind: VerifiedOperationKind,
    source: Option<FileLocator>,
    applied: AppliedFile,
}

impl VerifiedOperation {
    #[must_use]
    /// 创建已核对 operation 事实。
    pub const fn new(
        id: Uuid,
        kind: VerifiedOperationKind,
        source: Option<FileLocator>,
        applied: AppliedFile,
    ) -> Self {
        Self {
            id,
            kind,
            source,
            applied,
        }
    }

    pub(crate) const fn id(&self) -> Uuid {
        self.id
    }

    pub(crate) const fn kind(&self) -> VerifiedOperationKind {
        self.kind
    }

    pub(crate) const fn source(&self) -> Option<&FileLocator> {
        self.source.as_ref()
    }

    pub(crate) const fn applied(&self) -> &AppliedFile {
        &self.applied
    }
}

/// 一次补偿尝试的安全结果。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompensationOutcome {
    /// 本 operation 创建且未变化的目标已移除。
    Removed,
    /// move 已安全恢复到原位置。
    Restored,
    /// 目标已经不存在，无需重复删除。
    AlreadyAbsent,
    /// 外部变化或平台条件使自动补偿不再可证明安全。
    ManualReview,
}

/// 文件 operation 的稳定失败分类；不携带宿主路径或底层错误正文。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FileOperationError {
    /// 路径无效、逃逸或遇到符号链接。
    #[error("path is outside the deployment root")]
    PathOutsideRoot,
    /// 已配置根身份发生变化。
    #[error("deployment root identity changed")]
    RootChanged,
    /// 来源与计划快照不再一致。
    #[error("source file changed")]
    SourceChanged,
    /// 最终目标已经存在。
    #[error("target already exists")]
    TargetExists,
    /// hardlink 的来源与目标不在同一设备。
    #[error("hardlink crosses a filesystem boundary")]
    HardlinkCrossDevice,
    /// move 需要由上层 journal 编排 copy/verify/remove。
    #[error("move requires composite cross-device execution")]
    CompositeMoveRequired,
    /// 当前平台或输入不支持该原语。
    #[error("filesystem operation is unsupported")]
    OperationUnsupported,
    /// 发现同 operation 的临时/隔离产物，不能证明是否可重放。
    #[error("filesystem operation state is ambiguous")]
    AmbiguousState,
    /// 可重试的、不暴露宿主细节的 I/O 失败或协作停止。
    #[error("temporary filesystem operation failure")]
    IoTemporary,
}

impl FileOperationError {
    #[must_use]
    /// 返回 journal 与公开 reason 使用的稳定字符串。
    pub const fn stable_reason(self) -> &'static str {
        match self {
            Self::PathOutsideRoot => "path-outside-root",
            Self::RootChanged => "root-changed",
            Self::SourceChanged => "source-changed",
            Self::TargetExists => "target-exists",
            Self::HardlinkCrossDevice => "hardlink-cross-device",
            Self::CompositeMoveRequired | Self::OperationUnsupported => "operation-unsupported",
            Self::AmbiguousState | Self::IoTemporary => "io-temporary",
        }
    }
}

impl From<FsBoundaryError> for FileOperationError {
    fn from(error: FsBoundaryError) -> Self {
        match error {
            FsBoundaryError::RootChanged => Self::RootChanged,
            FsBoundaryError::PathInvalid
            | FsBoundaryError::PathEscape
            | FsBoundaryError::SymlinkForbidden => Self::PathOutsideRoot,
            FsBoundaryError::RootNotFound
            | FsBoundaryError::Unavailable
            | FsBoundaryError::Internal => Self::IoTemporary,
        }
    }
}

/// 文件复制生产者与 worker 共享的协作停止信号。
#[derive(Clone, Default)]
pub struct ProcessingStopToken {
    stopped: Arc<AtomicBool>,
}

impl ProcessingStopToken {
    /// 请求在下一个安全块边界停止；重复调用无副作用。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    #[must_use]
    /// 返回是否已请求停止。
    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
}

/// 锚定到预打开 deployment roots 的整理文件系统能力。
pub trait OrganizationFs: Send + Sync {
    /// 预检一个可写目标目录。
    ///
    /// # Errors
    ///
    /// 目标根只读、身份变化、路径无效或目录不可用时失败关闭。
    fn preflight_target(
        &self,
        root: &RootId,
        path: &RelativePath,
    ) -> Result<TargetCapabilitySnapshot, FsBoundaryError>;

    /// 不跟随链接地观察普通文件；不存在返回 `None`。
    ///
    /// # Errors
    ///
    /// 根或路径边界无法重新证明时失败关闭。
    fn observe(&self, locator: &FileLocator) -> Result<Option<ObservedFile>, FsBoundaryError>;

    /// 读取普通文件的身份、长度和 SHA-256，用于 journal 恢复核对。
    ///
    /// # Errors
    ///
    /// 根或路径边界无法重新证明、文件在读取期间变化或 I/O 失败时失败关闭。
    fn inspect(&self, locator: &FileLocator) -> Result<Option<AppliedFile>, FsBoundaryError>;

    /// 以固定缓冲复制并 no-clobber 发布。
    ///
    /// # Errors
    ///
    /// 来源变化、目标存在、协作停止或能力边界失败时返回稳定错误。
    fn copy_no_clobber(
        &self,
        operation: &FileOperationSpec,
        stop: &ProcessingStopToken,
    ) -> Result<AppliedFile, FileOperationError>;

    /// 在同设备内以 no-clobber rename 移动来源。
    ///
    /// # Errors
    ///
    /// 跨设备返回 [`FileOperationError::CompositeMoveRequired`]；不执行降级复制。
    fn move_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError>;

    /// 在同设备内以 no-clobber 语义创建硬链接。
    ///
    /// # Errors
    ///
    /// 跨设备或身份无法核对时失败关闭；不降级为 copy/move。
    fn hardlink_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError>;

    /// 以 1 MiB 上限发布一个原先不存在的 NFO。
    ///
    /// # Errors
    ///
    /// 超限、目标存在或能力边界失败时返回稳定错误。
    fn write_new_nfo(
        &self,
        operation: &NfoOperationSpec,
        bytes: &[u8],
    ) -> Result<AppliedFile, FileOperationError>;

    /// 在跨设备 move 的目标已核对后，只删除仍匹配快照与摘要的来源。
    ///
    /// 来源已经不存在时按幂等成功处理；任何身份或摘要变化都失败关闭。
    ///
    /// # Errors
    ///
    /// 来源变化、来源根只读或能力边界/I/O 失败时返回稳定错误。
    fn remove_verified_source(
        &self,
        operation_id: Uuid,
        source: &ObservedFile,
        expected_sha256: &[u8; 32],
    ) -> Result<(), FileOperationError>;

    /// 只补偿仍与已核对事实一致的 operation 产物。
    ///
    /// # Errors
    ///
    /// I/O 或能力边界无法安全完成观察/操作时返回稳定错误。
    fn compensate(
        &self,
        operation: &VerifiedOperation,
    ) -> Result<CompensationOutcome, FileOperationError>;
}
