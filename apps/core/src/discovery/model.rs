use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::fd::OwnedFd;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::shared::page::CursorPage;

const MAX_ROOT_ID_BYTES: usize = 64;

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
/// 用于选择已配置部署根目录的已验证标识符。
///
/// 值由 1–64 个 ASCII 字节组成，以 `a`–`z` 开头，其余位置只能包含小写字母、数字、`-` 或 `_`。
/// 构造仅允许通过 [`Self::parse`]，因此序列化和显示的值始终满足该语法，可安全用作逻辑 ID（而非文件系统路径）。
pub struct RootId(String);

impl RootId {
    /// 解析稳定的部署根目录标识符。
    ///
    /// # Errors
    ///
    /// 标识符不符合安全语法时返回 [`FsBoundaryError::PathInvalid`]。
    pub fn parse(value: &str) -> Result<Self, FsBoundaryError> {
        let bytes = value.as_bytes();
        if bytes.is_empty()
            || bytes.len() > MAX_ROOT_ID_BYTES
            || !bytes[0].is_ascii_lowercase()
            || !bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-' || *byte == b'_'
            })
        {
            return Err(FsBoundaryError::PathInvalid);
        }
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    /// 借用传入 [`Self::parse`] 后得到的原始已验证标识符。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RootId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("RootId").field(&self.0).finish()
    }
}

impl fmt::Display for RootId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
/// 相对于部署根目录能力的规范化 UTF-8 路径。
///
/// 该值绝不为绝对路径，且不包含父级遍历、NUL、反斜杠、驱动器前缀或 UNC 前缀。空组件和 `.` 会被移除；
/// 逻辑根目录表示为 `.`。此验证仅为词法验证，文件系统访问仍须使用 [`DirectoryCapability`]。
pub struct RelativePath(String);

impl RelativePath {
    /// 解析并规范化不可信的 UTF-8 请求路径。
    ///
    /// # Errors
    ///
    /// 输入为空、绝对路径、父级路径、含 NUL、驱动器或 UNC 前缀、或含反斜杠时返回
    /// [`FsBoundaryError::PathInvalid`]。
    pub fn parse(value: &str) -> Result<Self, FsBoundaryError> {
        if value.is_empty()
            || value.contains('\0')
            || value.contains('\\')
            || value.starts_with('/')
            || value.starts_with("//")
            || has_windows_drive_prefix(value)
        {
            return Err(FsBoundaryError::PathInvalid);
        }
        let mut components = Vec::new();
        for component in value.split('/') {
            match component {
                "" | "." => {}
                ".." => return Err(FsBoundaryError::PathInvalid),
                value => components.push(value),
            }
        }
        let normalized = if components.is_empty() {
            ".".to_owned()
        } else {
            components.join("/")
        };
        Ok(Self(normalized))
    }

    #[must_use]
    /// 借用以斜杠分隔的规范化表示。
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|component| *component != ".")
    }

    #[must_use]
    /// 返回两个路径是否相等，或其中一个是否按完整组件计为另一个的祖先。
    ///
    /// 例如，`movies` 与 `movies/2026` 重叠，而 `movie` 与 `movies` 不重叠。
    pub fn overlaps(&self, other: &Self) -> bool {
        let left = self.components().collect::<Vec<_>>();
        let right = other.components().collect::<Vec<_>>();
        component_prefix(&left, &right) || component_prefix(&right, &left)
    }

    #[must_use]
    /// 借用规范化表示的 UTF-8 字节。
    pub fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for RelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("RelativePath")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn component_prefix(left: &[&str], right: &[&str]) -> bool {
    left.len() <= right.len() && left.iter().zip(right).all(|(left, right)| left == right)
}

fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 部署根目录声明的访问策略。
pub enum RootAccess {
    /// Core 可以发现内容，但不得修改它。
    ReadOnly,
    /// 此部署允许未来进行具备写入能力的操作，以及内容发现。
    ReadWrite,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 不含主机文件系统路径、对客户端可见的部署根目录声明。
pub struct DeploymentRootView {
    /// 收件箱命令使用的稳定逻辑标识符。
    pub id: RootId,
    /// 人类可读的部署标签。
    pub label: String,
    /// 声明的读写策略。
    pub access: RootAccess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 打开部署根目录能力时捕获的内核身份。
///
/// 设备号和 inode 始终存在。主机支持时，`mount_id` 可区分复用相同设备号/inode 对的替换挂载。
pub struct RootIdentity {
    /// 来自目录元数据的主机设备号。
    pub device: u64,
    /// 来自目录元数据的主机 inode 号。
    pub inode: u64,
    /// 可用时的 Linux 挂载标识符。
    pub mount_id: Option<u64>,
}

impl RootIdentity {
    #[must_use]
    /// 为持久化的重新验证快照确定性地编码身份。
    ///
    /// 编码依次为大端设备号和 inode、挂载 ID 是否存在的字节，随后是大端挂载 ID（缺失时为零）。
    pub fn snapshot_bytes(self) -> Vec<u8> {
        identity_bytes(self.device, self.inode, self.mount_id)
    }

    /// 返回可跨进程和容器命名空间持久化的设备号/inode 身份。
    pub(crate) fn durable_snapshot_bytes(self) -> [u8; 16] {
        durable_identity_bytes(self.device, self.inode)
    }

    /// 比较当前根目录与新版 16 字节或旧版 25 字节持久身份，忽略命名空间本地的挂载编号。
    pub(crate) fn matches_durable_snapshot(self, stored: &[u8]) -> bool {
        matches_durable_identity(self.device, self.inode, stored)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 通过能力观察到的文件或目录内核身份。
pub struct FileIdentity {
    /// 来自条目元数据的主机设备号。
    pub device: u64,
    /// 来自条目元数据的主机 inode 号。
    pub inode: u64,
    /// 可用时的 Linux 挂载标识符。
    pub mount_id: Option<u64>,
}

impl FileIdentity {
    #[must_use]
    /// 使用与 [`RootIdentity::snapshot_bytes`] 相同的确定性格式编码身份。
    pub fn snapshot_bytes(self) -> Vec<u8> {
        identity_bytes(self.device, self.inode, self.mount_id)
    }

    /// 返回可跨进程和容器命名空间持久化的设备号/inode 身份。
    pub(crate) fn durable_snapshot_bytes(self) -> [u8; 16] {
        durable_identity_bytes(self.device, self.inode)
    }

    /// 比较当前目录与新版 16 字节或旧版 25 字节持久身份，忽略命名空间本地的挂载编号。
    pub(crate) fn matches_durable_snapshot(self, stored: &[u8]) -> bool {
        matches_durable_identity(self.device, self.inode, stored)
    }
}

fn durable_identity_bytes(device: u64, inode: u64) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&device.to_be_bytes());
    bytes[8..].copy_from_slice(&inode.to_be_bytes());
    bytes
}

fn matches_durable_identity(device: u64, inode: u64, stored: &[u8]) -> bool {
    matches!(stored.len(), 16 | 25) && stored[..16] == durable_identity_bytes(device, inode)
}

fn identity_bytes(device: u64, inode: u64, mount_id: Option<u64>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(25);
    bytes.extend_from_slice(&device.to_be_bytes());
    bytes.extend_from_slice(&inode.to_be_bytes());
    bytes.push(u8::from(mount_id.is_some()));
    bytes.extend_from_slice(&mount_id.unwrap_or_default().to_be_bytes());
    bytes
}

/// 一个部署根目录下经验证目录的不可伪造句柄。
///
/// 此句柄会保留已打开的目录描述符、根目录和目录身份快照、逻辑/显示路径、原始 OS 路径字节及签发来源。
/// 文件系统适配器会拒绝并非由自身签发的能力；克隆操作共享同一个描述符。
pub struct DirectoryCapability {
    root_id: RootId,
    relative_path: RelativePath,
    raw_relative_path: Arc<[u8]>,
    root_identity: RootIdentity,
    directory_identity: FileIdentity,
    fd: Arc<OwnedFd>,
    provenance: [u8; 16],
}

pub(crate) struct VerifiedDirectoryCapability<'a> {
    /// 签发此能力的适配器对应的逻辑根目录。
    pub root_id: &'a RootId,
    /// 遍历前必须仍然匹配的根目录身份。
    pub root_identity: RootIdentity,
    /// 已打开目录描述符的身份。
    pub directory_identity: FileIdentity,
    /// 用于相对路径且不跟随链接操作的开放描述符。
    pub fd: &'a OwnedFd,
}

impl DirectoryCapability {
    pub(crate) fn issue(
        root_id: RootId,
        relative_path: RelativePath,
        root_identity: RootIdentity,
        directory_identity: FileIdentity,
        fd: OwnedFd,
        provenance: [u8; 16],
    ) -> Self {
        Self {
            root_id,
            raw_relative_path: Arc::from(relative_path.bytes()),
            relative_path,
            root_identity,
            directory_identity,
            fd: Arc::new(fd),
            provenance,
        }
    }

    pub(crate) fn issue_child(
        parent: &Self,
        relative_path: RelativePath,
        raw_relative_path: Vec<u8>,
        directory_identity: FileIdentity,
        fd: OwnedFd,
    ) -> Self {
        Self {
            root_id: parent.root_id.clone(),
            relative_path,
            raw_relative_path: Arc::from(raw_relative_path),
            root_identity: parent.root_identity,
            directory_identity,
            fd: Arc::new(fd),
            provenance: parent.provenance,
        }
    }

    #[must_use]
    /// 借用能力遍历期间累积的精确相对 OS 路径字节。
    ///
    /// 子条目的这些字节可能不是 UTF-8，适合用作身份键而非 JSON。
    pub fn raw_relative_path(&self) -> &[u8] {
        &self.raw_relative_path
    }

    #[must_use]
    /// 返回用于 API 响应和诊断信息的规范化 UTF-8 显示路径。
    pub fn relative_path_display(&self) -> &str {
        self.relative_path.as_str()
    }

    pub(crate) fn verify(
        &self,
        provenance: &[u8; 16],
    ) -> Result<VerifiedDirectoryCapability<'_>, FsBoundaryError> {
        if self.provenance != *provenance {
            return Err(FsBoundaryError::PathInvalid);
        }
        Ok(VerifiedDirectoryCapability {
            root_id: &self.root_id,
            root_identity: self.root_identity,
            directory_identity: self.directory_identity,
            fd: &self.fd,
        })
    }
}

impl Clone for DirectoryCapability {
    fn clone(&self) -> Self {
        Self {
            root_id: self.root_id.clone(),
            relative_path: self.relative_path.clone(),
            raw_relative_path: Arc::clone(&self.raw_relative_path),
            root_identity: self.root_identity,
            directory_identity: self.directory_identity,
            fd: Arc::clone(&self.fd),
            provenance: self.provenance,
        }
    }
}

impl fmt::Debug for DirectoryCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DirectoryCapability")
            .field("root_id", &self.root_id)
            .field("relative_path", &self.relative_path)
            .finish_non_exhaustive()
    }
}

/// 由开放能力支撑的成功目录预检结果。
///
/// 它暴露用于持久化的不可变身份快照，并可被借用或消耗以继续受能力约束的遍历。
pub struct DirectoryIdentity {
    capability: DirectoryCapability,
}

impl DirectoryIdentity {
    pub(crate) fn new(capability: DirectoryCapability) -> Self {
        Self { capability }
    }

    #[must_use]
    /// 返回签发能力时捕获的根目录身份。
    pub fn root_identity(&self) -> RootIdentity {
        self.capability.root_identity
    }

    #[must_use]
    /// 返回已验证目标目录的身份。
    pub fn directory_identity(&self) -> FileIdentity {
        self.capability.directory_identity
    }

    #[must_use]
    /// 借用开放目录能力以执行后续相对路径操作。
    pub fn capability(&self) -> &DirectoryCapability {
        &self.capability
    }

    #[must_use]
    /// 消耗预检结果并返回其开放目录能力。
    pub fn into_capability(self) -> DirectoryCapability {
        self.capability
    }
}

impl fmt::Debug for DirectoryIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DirectoryIdentity")
            .field("root_id", &self.capability.root_id)
            .field("relative_path", &self.capability.relative_path)
            .field("root_identity", &self.capability.root_identity)
            .field("directory_identity", &self.capability.directory_identity)
            .finish()
    }
}

/// 受能力约束的目录枚举返回的原始子名称。
///
/// 该名称是单个主机原生组件，可能不是有效 UTF-8；`Debug` 会有意隐藏其字节，避免泄露主机路径。
pub struct DirectoryEntry {
    name: OsString,
}

impl DirectoryEntry {
    pub(crate) fn new(name: OsString) -> Self {
        Self { name }
    }

    #[must_use]
    /// 借用主机原生子名称，用于不跟随链接的元数据/打开操作。
    pub fn name(&self) -> &OsStr {
        &self.name
    }
}

impl fmt::Debug for DirectoryEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DirectoryEntry")
            .field("name_bytes", &"[OS BYTES]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 为目录条目观察到的不跟随链接文件类型。
pub enum EntryKind {
    /// 常规文件。
    File,
    /// 可进行受能力约束下钻的目录。
    Directory,
    /// 符号链接；发现流程会将其视为禁止项，而不会跟随它。
    Symlink,
    /// 套接字、设备、FIFO 或其他不受支持的文件系统类型。
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 分类并持久化一个目录条目所需的不跟随链接元数据。
pub struct EntryMetadata {
    kind: EntryKind,
    /// 条目自身的设备/inode/挂载身份。
    pub identity: FileIdentity,
    /// 主机报告的字节长度。
    pub size: u64,
    /// 以 Unix 纪元起的纳秒表示的主机修改时间戳。
    pub modified_at_ns: i128,
}

impl EntryMetadata {
    pub(crate) const fn new(
        kind: EntryKind,
        identity: FileIdentity,
        size: u64,
        modified_at_ns: i128,
    ) -> Self {
        Self {
            kind,
            identity,
            size,
            modified_at_ns,
        }
    }

    #[must_use]
    /// 返回不跟随链接的类型是否为 [`EntryKind::File`]。
    pub const fn is_file(self) -> bool {
        matches!(self.kind, EntryKind::File)
    }

    #[must_use]
    /// 返回不跟随链接的类型是否为 [`EntryKind::Directory`]。
    pub const fn is_directory(self) -> bool {
        matches!(self.kind, EntryKind::Directory)
    }

    #[must_use]
    /// 返回不跟随链接的类型是否为 [`EntryKind::Symlink`]。
    pub const fn is_symlink(self) -> bool {
        matches!(self.kind, EntryKind::Symlink)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// 文件系统请求无法留在其能力边界内时采取失败关闭的原因。
pub enum FsBoundaryError {
    #[error("unknown deployment root")]
    /// 没有已配置根目录具有请求的 [`RootId`]。
    RootNotFound,
    #[error("deployment root or directory is unavailable")]
    /// 声明的根目录或目标目录当前无法打开或读取。
    Unavailable,
    #[error("deployment root identity changed")]
    /// 重新验证发现已打开根目录不再具有其声明的身份。
    RootChanged,
    #[error("relative path is invalid")]
    /// 标识符/路径未通过词法验证或能力来源验证。
    PathInvalid,
    #[error("relative path escaped the capability root")]
    /// 内核/路径解析表明遍历超出了根目录能力范围。
    PathEscape,
    #[error("symbolic links are forbidden")]
    /// 不跟随链接操作遇到了符号链接。
    SymlinkForbidden,
    #[error("filesystem operation failed")]
    /// 不得暴露主机细节的意外文件系统故障。
    Internal,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
/// 为收件箱预检或创建提交的不可信逻辑根目录和相对路径。
///
/// 两个字符串在文件系统访问前都会被验证为 [`RootId`] 和 [`RelativePath`]。
pub struct PreflightInboxCommand {
    /// 来自配置的部署根目录标识符。
    pub root_id: String,
    /// 相对于该根目录的 UTF-8 路径。
    pub relative_path: String,
}

/// 收件箱创建接受与预检相同的已验证输入。
pub type CreateInboxCommand = PreflightInboxCommand;

impl fmt::Debug for PreflightInboxCommand {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreflightInboxCommand")
            .field("root_id", &self.root_id)
            .field("relative_path", &self.relative_path)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 候选收件箱的能力和重叠检查的非变更结果。
pub struct InboxPreflightView {
    /// 已验证的部署根目录。
    pub root_id: RootId,
    /// 在根目录下检查的规范化路径。
    pub relative_path: RelativePath,
    /// 目录通过能力边界打开并验证后为 `true`。
    pub readable: bool,
    /// 同一根目录上的现有收件箱是否相等、为祖先或为后代。
    pub overlaps_existing: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 已持久化的收件箱及其最近一次身份重新验证时观察到的健康状态。
pub struct InboxDirectoryView {
    /// 稳定的收件箱 UUID。
    pub id: Uuid,
    /// 包含该收件箱的部署根目录。
    pub root_id: RootId,
    /// 根目录下的规范化目录路径。
    pub relative_path: RelativePath,
    /// 将当前根目录/目录身份与持久化快照比较的结果。
    pub health: InboxHealth,
    /// 该次重新验证的 RFC 3339 UTC 时间戳。
    pub last_checked_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 重新验证已持久化收件箱能力边界的当前结果。
pub enum InboxHealth {
    /// 根目录和目录仍可使用其持久化身份读取。
    Available,
    /// 访问失败，或任一持久化身份已变化。
    Unavailable,
}

/// 重新验证后的收件箱目录视图游标页。
pub type InboxDirectoryPage = CursorPage<InboxDirectoryView>;

#[cfg(test)]
mod tests {
    use super::{FileIdentity, RootIdentity};

    #[test]
    fn durable_root_identity_accepts_mount_id_renumbering_only() {
        let stored = RootIdentity {
            device: 75,
            inode: 256,
            mount_id: Some(1_303),
        };
        let recreated_container = RootIdentity {
            mount_id: Some(1_400),
            ..stored
        };
        let replaced_root = RootIdentity {
            inode: 257,
            ..recreated_container
        };

        assert_ne!(
            stored.snapshot_bytes(),
            recreated_container.snapshot_bytes()
        );
        assert_eq!(recreated_container.durable_snapshot_bytes().len(), 16);
        assert!(recreated_container.matches_durable_snapshot(&stored.snapshot_bytes()));
        assert!(!replaced_root.matches_durable_snapshot(&stored.snapshot_bytes()));
    }

    #[test]
    fn durable_directory_identity_accepts_mount_id_renumbering_only() {
        let stored = FileIdentity {
            device: 75,
            inode: 16_453,
            mount_id: Some(1_303),
        };
        let recreated_container = FileIdentity {
            mount_id: Some(1_400),
            ..stored
        };
        let replaced_directory = FileIdentity {
            device: 76,
            ..recreated_container
        };

        assert_eq!(recreated_container.durable_snapshot_bytes().len(), 16);
        assert!(recreated_container.matches_durable_snapshot(&stored.snapshot_bytes()));
        assert!(!replaced_directory.matches_durable_snapshot(&stored.snapshot_bytes()));
    }
}
