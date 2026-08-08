use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::Read;
use std::os::fd::AsFd;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::bootstrap::config::RunMode;
use crate::discovery::model::{
    DeploymentRootView, DirectoryCapability, DirectoryEntry, DirectoryIdentity, EntryMetadata,
    FsBoundaryError, RelativePath, RootAccess, RootId,
};
use crate::shared::error::{AppError, ErrorCode};

/// 保持已打开部署根目录能力边界的文件系统操作。
pub trait CapabilityFs: Send + Sync {
    /// 打开并校验已配置根目录下的相对目录。
    ///
    /// # Errors
    ///
    /// 根目录未知/不可用、路径不安全、遇到符号链接，或无法证明打开目录位于当前根目录下时，
    /// 返回 [`FsBoundaryError`]。
    fn preflight_directory(
        &self,
        root: &RootId,
        relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError>;
    /// 从现有目录能力中枚举原始子项名称。
    ///
    /// # Errors
    ///
    /// 能力来源/根目录身份重新验证失败，或无法在不离开边界的情况下枚举目录时，
    /// 返回 [`FsBoundaryError`]。
    fn read_directory(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError>;
    /// 为能力目录打开流式原始名称迭代器。
    ///
    /// # Errors
    ///
    /// 根目录/目录身份已改变或无法开始枚举时返回 [`FsBoundaryError`]。
    fn open_directory_stream(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Box<dyn DirectoryEntryStream>, FsBoundaryError> {
        Ok(Box::new(VecDirectoryEntryStream {
            entries: self.read_directory(capability)?.into(),
        }))
    }
    /// 在不跟随符号链接的前提下打开子目录。
    ///
    /// # Errors
    ///
    /// 名称不安全、解析为符号链接/非目录，或无法相对已验证父目录打开子项时返回
    /// [`FsBoundaryError`]。默认实现不受支持，返回 [`FsBoundaryError::Internal`]。
    fn open_child_directory(
        &self,
        _capability: &DirectoryCapability,
        _name: &std::ffi::OsStr,
    ) -> Result<DirectoryCapability, FsBoundaryError> {
        Err(FsBoundaryError::Internal)
    }
    /// 在不跟随符号链接的前提下读取子项元数据。
    ///
    /// # Errors
    ///
    /// 名称/能力无效、根目录已改变、无法在不跟随链接的前提下检查条目，或无法安全表示主机元数据时，
    /// 返回 [`FsBoundaryError`]。
    fn metadata_no_follow(
        &self,
        capability: &DirectoryCapability,
        name: &std::ffi::OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError>;
    /// 相对于已验证的目录能力读取一个受大小限制的普通文件。
    ///
    /// # Errors
    ///
    /// 当 capability/name 无效、根目录已改变，或条目不是普通的非符号链接文件时，返回
    /// [`CapabilityFileError::Boundary`]。超过字节限制时返回 [`CapabilityFileError::TooLarge`]，
    /// 且不会返回部分内容。默认实现不受支持。
    fn read_bounded_relative_file(
        &self,
        _capability: &DirectoryCapability,
        _name: &std::ffi::OsStr,
        _max_bytes: usize,
    ) -> Result<Vec<u8>, CapabilityFileError> {
        Err(CapabilityFileError::Boundary(FsBoundaryError::Internal))
    }
}

/// 受限能力相对文件读取的稳定失败分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CapabilityFileError {
    /// 文件系统能力边界拒绝该操作，或无法完成该操作。
    #[error(transparent)]
    Boundary(#[from] FsBoundaryError),
    /// 已打开的普通文件字节数超过调用方限制。
    #[error("file exceeds the read limit")]
    TooLarge,
}

/// 从一个已验证目录能力中拉取式枚举原始子项名称。
pub trait DirectoryEntryStream {
    /// 返回下一个原始目录条目；流结束时返回 `None`。
    ///
    /// # Errors
    ///
    /// 迭代期间底层目录读取失败时返回 [`FsBoundaryError`]。
    fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, FsBoundaryError>;
}

struct VecDirectoryEntryStream {
    entries: std::collections::VecDeque<DirectoryEntry>,
}

impl DirectoryEntryStream for VecDirectoryEntryStream {
    fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, FsBoundaryError> {
        Ok(self.entries.pop_front())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RootDocument {
    roots: Vec<RawRootDeclaration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRootDeclaration {
    id: String,
    label: String,
    container_path: PathBuf,
    access: RootAccess,
}

#[derive(Clone)]
/// 包含私有主机路径的已校验部署根目录声明。
///
/// `Debug` 会有意遮蔽容器路径；客户端响应使用 [`DeploymentRootView`]。
pub struct RootDeclaration {
    pub(crate) id: RootId,
    pub(crate) label: String,
    pub(crate) container_path: PathBuf,
    pub(crate) access: RootAccess,
}

impl fmt::Debug for RootDeclaration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RootDeclaration")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("access", &self.access)
            .field("container_path", &"[REDACTED]")
            .finish()
    }
}

/// 有序、已校验的部署根目录声明及其无路径公开视图。
///
/// 由 [`Self::load`] 加载的集合至多包含 100 个唯一 ID、互不重叠的规范化绝对主机路径、
/// 有边界的非空标签以及精确源文件的指纹。
pub struct DeploymentRootSet {
    declarations: Vec<RootDeclaration>,
    views: Vec<DeploymentRootView>,
    fingerprint: DeploymentRootsFingerprint,
}

#[derive(Clone, Eq, PartialEq)]
/// 一次不跟随链接的配置读取所捕获的文件身份、长度和 SHA-256 摘要。
pub struct DeploymentRootsFingerprint {
    device: u64,
    inode: u64,
    length: u64,
    sha256: [u8; 32],
}

impl DeploymentRootsFingerprint {
    #[must_use]
    /// 不跟随 `path` 的最终符号链接重新打开它，并比较完整快照。
    ///
    /// 文件无法读取，或身份、长度、内容已变化时返回 `false`。
    pub fn matches_path(&self, path: &Path) -> bool {
        read_config_snapshot(path).is_ok_and(|(_, current)| current == *self)
    }
}

impl DeploymentRootSet {
    /// 严格加载、校验并限制部署根目录声明。
    ///
    /// # Errors
    ///
    /// 文件不可用/是符号链接、JSON 无效、声明超过 100 个根目录、ID 无效/重复、标签为空或超过
    /// 100 个字符，或路径非绝对、为根目录、含父路径或重叠时，返回 [`AppError`]。
    pub fn load(path: &Path, _mode: RunMode) -> Result<Self, AppError> {
        let (bytes, fingerprint) = read_config_snapshot(path)?;
        let document: RootDocument = serde_json::from_slice(&bytes)
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        if document.roots.len() > 100 {
            return Err(config_error("too many deployment roots"));
        }
        let mut ids = BTreeSet::new();
        let mut paths = Vec::<PathBuf>::new();
        let mut declarations = Vec::with_capacity(document.roots.len());
        for raw in document.roots {
            let id = RootId::parse(&raw.id)
                .map_err(|_| config_error("deployment root ID is invalid"))?;
            if !ids.insert(id.clone()) {
                return Err(config_error("deployment root ID is duplicated"));
            }
            if raw.label.trim().is_empty() || raw.label.chars().count() > 100 {
                return Err(config_error("deployment root label is invalid"));
            }
            let normalized = normalize_absolute_path(&raw.container_path)?;
            if paths
                .iter()
                .any(|existing| path_overlaps(existing, &normalized))
            {
                return Err(config_error("deployment roots overlap"));
            }
            paths.push(normalized.clone());
            declarations.push(RootDeclaration {
                id,
                label: raw.label,
                container_path: normalized,
                access: raw.access,
            });
        }
        let views = declarations
            .iter()
            .map(|root| DeploymentRootView {
                id: root.id.clone(),
                label: root.label.clone(),
                access: root.access,
            })
            .collect();
        Ok(Self {
            declarations,
            views,
            fingerprint,
        })
    }

    #[must_use]
    /// 借用已校验声明，包括适配器使用的私有容器路径。
    pub fn declarations(&self) -> &[RootDeclaration] {
        &self.declarations
    }

    #[must_use]
    /// 借用按声明顺序排列、可安全用于 API 响应的无路径视图。
    pub fn views(&self) -> &[DeploymentRootView] {
        &self.views
    }

    #[must_use]
    /// 将无路径视图克隆为以根目录 ID 为键的确定性映射。
    pub fn view_map(&self) -> BTreeMap<RootId, DeploymentRootView> {
        self.views
            .iter()
            .cloned()
            .map(|view| (view.id.clone(), view))
            .collect()
    }

    #[must_use]
    /// 克隆配置变化时用于关闭路由的源文件指纹。
    pub fn fingerprint(&self) -> DeploymentRootsFingerprint {
        self.fingerprint.clone()
    }
}

impl fmt::Debug for DeploymentRootSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeploymentRootSet")
            .field("views", &self.views)
            .finish_non_exhaustive()
    }
}

fn read_config_snapshot(path: &Path) -> Result<(Vec<u8>, DeploymentRootsFingerprint), AppError> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    let stat = rustix::fs::fstat(fd.as_fd())
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    let mut bytes = Vec::new();
    std::fs::File::from(fd)
        .read_to_end(&mut bytes)
        .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
    let fingerprint = DeploymentRootsFingerprint {
        device: u64::try_from(stat.st_dev).map_err(|_| config_error("config identity invalid"))?,
        inode: stat.st_ino as u64,
        length: u64::try_from(stat.st_size).map_err(|_| config_error("config size invalid"))?,
        sha256: Sha256::digest(&bytes).into(),
    };
    Ok((bytes, fingerprint))
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, AppError> {
    if !path.is_absolute() || path == Path::new("/") {
        return Err(config_error("deployment root path is invalid"));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(config_error("deployment root path contains ..")),
        }
    }
    Ok(normalized)
}

fn path_overlaps(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn config_error(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ConfigInvalid, message)
}
