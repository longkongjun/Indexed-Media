use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{Access, AtFlags, FileType, Mode, OFlags};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::bootstrap::config::RunMode;
use crate::discovery::capability::{
    CapabilityFileError, CapabilityFs, DirectoryEntryStream, RootDeclaration,
};
use crate::discovery::model::{
    DirectoryCapability, DirectoryEntry, DirectoryIdentity, EntryKind, EntryMetadata, FileIdentity,
    FsBoundaryError, RelativePath, RootAccess, RootId, RootIdentity,
};
use crate::organization::fs::{
    AppliedFile, CompensationOutcome, FileLocator, FileOperationError, FileOperationSpec,
    NfoOperationSpec, ObservedFile, OrganizationFs, ProcessingStopToken, TargetCapabilitySnapshot,
    VerifiedOperation, VerifiedOperationKind,
};
use crate::shared::error::{AppError, ErrorCode};

struct RootHandle {
    id: RootId,
    configured_path: PathBuf,
    access: RootAccess,
    identity: RootIdentity,
    fd: Arc<OwnedFd>,
}

impl std::fmt::Debug for RootHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RootHandle")
            .field("id", &self.id)
            .field("configured_path", &"[REDACTED]")
            .field("access", &self.access)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

/// 将全部发现操作锚定到预打开根目录描述符的主机文件系统适配器。
///
/// 每个实例都有唯一的来源；由其他实例签发的能力会被拒绝。
pub struct OsCapabilityFs {
    roots: BTreeMap<RootId, RootHandle>,
    provenance: [u8; 16],
}

impl OsCapabilityFs {
    /// 将每个已配置根目录作为目录能力打开，并记录其身份快照。
    ///
    /// # Errors
    ///
    /// 若任一根目录无法在不跟随其最终符号链接的情况下打开，则返回 [`AppError`]。
    pub fn open(declarations: &[RootDeclaration], mode: RunMode) -> Result<Self, AppError> {
        let mut roots = BTreeMap::new();
        for declaration in declarations {
            let fd = open_root(&declaration.container_path).map_err(|_| {
                AppError::new(ErrorCode::ConfigInvalid, "deployment root unavailable")
            })?;
            let identity = root_identity(&fd).map_err(|_| {
                AppError::new(
                    ErrorCode::ConfigInvalid,
                    "deployment root identity unavailable",
                )
            })?;
            validate_root_mode(&fd, mode).map_err(|_| {
                AppError::new(ErrorCode::ConfigInvalid, "deployment root is not permitted")
            })?;
            require_effective_read_search(&fd).map_err(|_| {
                AppError::new(
                    ErrorCode::ConfigInvalid,
                    "deployment root is not readable and searchable",
                )
            })?;
            bounded_readability_probe(&fd).map_err(|_| {
                AppError::new(
                    ErrorCode::ConfigInvalid,
                    "deployment root cannot be enumerated",
                )
            })?;
            roots.insert(
                declaration.id.clone(),
                RootHandle {
                    id: declaration.id.clone(),
                    configured_path: declaration.container_path.clone(),
                    access: declaration.access,
                    identity,
                    fd: Arc::new(fd),
                },
            );
        }
        Ok(Self {
            roots,
            provenance: *uuid::Uuid::now_v7().as_bytes(),
        })
    }

    fn root(&self, root_id: &RootId) -> Result<&RootHandle, FsBoundaryError> {
        self.roots.get(root_id).ok_or(FsBoundaryError::RootNotFound)
    }

    fn revalidate_root(root: &RootHandle) -> Result<(), FsBoundaryError> {
        let current = open_root(&root.configured_path).map_err(|_| FsBoundaryError::RootChanged)?;
        require_effective_read_search(&current).map_err(|_| FsBoundaryError::RootChanged)?;
        let current_identity = root_identity(&current).map_err(|_| FsBoundaryError::RootChanged)?;
        if current_identity == root.identity {
            Ok(())
        } else {
            Err(FsBoundaryError::RootChanged)
        }
    }
}

impl CapabilityFs for OsCapabilityFs {
    fn preflight_directory(
        &self,
        root_id: &RootId,
        relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        let root = self.root(root_id)?;
        Self::revalidate_root(root)?;
        let mut current =
            rustix::io::dup(root.fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?;
        for component in relative.components() {
            current = open_directory_at(&current, OsStr::new(component))?;
        }
        require_effective_read_search(&current)?;
        bounded_readability_probe(&current)?;
        let directory_identity =
            file_identity(&current).map_err(|_| FsBoundaryError::Unavailable)?;
        Ok(DirectoryIdentity::new(DirectoryCapability::issue(
            root_id.clone(),
            relative.clone(),
            root.identity,
            directory_identity,
            current,
            self.provenance,
        )))
    }

    fn read_directory(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        let capability = capability.verify(&self.provenance)?;
        let root = self.root(capability.root_id)?;
        Self::revalidate_root(root)?;
        if root.identity != capability.root_identity
            || file_identity(capability.fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?
                != capability.directory_identity
        {
            return Err(FsBoundaryError::RootChanged);
        }
        require_effective_read_search(capability.fd.as_fd())?;
        let mut directory = rustix::fs::Dir::read_from(capability.fd.as_fd())
            .map_err(|_| FsBoundaryError::Unavailable)?;
        let mut entries = Vec::new();
        while let Some(entry) = directory.read() {
            let entry = entry.map_err(|_| FsBoundaryError::Unavailable)?;
            let bytes = entry.file_name().to_bytes();
            if matches!(bytes, b"." | b"..") {
                continue;
            }
            entries.push(DirectoryEntry::new(os_string_from_bytes(bytes)));
        }
        Ok(entries)
    }

    fn open_directory_stream(
        &self,
        capability: &DirectoryCapability,
    ) -> Result<Box<dyn DirectoryEntryStream>, FsBoundaryError> {
        let verified = capability.verify(&self.provenance)?;
        let root = self.root(verified.root_id)?;
        Self::revalidate_root(root)?;
        if root.identity != verified.root_identity
            || file_identity(verified.fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?
                != verified.directory_identity
        {
            return Err(FsBoundaryError::RootChanged);
        }
        require_effective_read_search(verified.fd.as_fd())?;
        let directory = rustix::fs::Dir::read_from(verified.fd.as_fd())
            .map_err(|_| FsBoundaryError::Unavailable)?;
        Ok(Box::new(OsDirectoryEntryStream { directory }))
    }

    fn open_child_directory(
        &self,
        capability: &DirectoryCapability,
        name: &OsStr,
    ) -> Result<DirectoryCapability, FsBoundaryError> {
        let verified = capability.verify(&self.provenance)?;
        let root = self.root(verified.root_id)?;
        Self::revalidate_root(root)?;
        if root.identity != verified.root_identity
            || file_identity(verified.fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?
                != verified.directory_identity
        {
            return Err(FsBoundaryError::RootChanged);
        }
        let fd = open_directory_at(verified.fd, name)?;
        require_effective_read_search(&fd)?;
        let identity = file_identity(&fd).map_err(|_| FsBoundaryError::Unavailable)?;
        let mut raw = if capability.raw_relative_path() == b"." {
            Vec::new()
        } else {
            capability.raw_relative_path().to_vec()
        };
        if !raw.is_empty() {
            raw.push(b'/');
        }
        raw.extend_from_slice(os_str_bytes(name));
        let mut display = if capability.relative_path_display() == "." {
            String::new()
        } else {
            capability.relative_path_display().to_owned()
        };
        if !display.is_empty() {
            display.push('/');
        }
        display.push_str(&safe_name_display(name));
        let relative = RelativePath::parse(&display).map_err(|_| FsBoundaryError::PathInvalid)?;
        Ok(DirectoryCapability::issue_child(
            capability, relative, raw, identity, fd,
        ))
    }

    fn metadata_no_follow(
        &self,
        capability: &DirectoryCapability,
        name: &OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        let capability = capability.verify(&self.provenance)?;
        let root = self.root(capability.root_id)?;
        Self::revalidate_root(root)?;
        if root.identity != capability.root_identity
            || file_identity(capability.fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?
                != capability.directory_identity
        {
            return Err(FsBoundaryError::RootChanged);
        }
        require_effective_read_search(capability.fd.as_fd())?;
        validate_single_name(name)?;
        let stat = rustix::fs::statat(capability.fd.as_fd(), name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| FsBoundaryError::Unavailable)?;
        let kind = mode_kind(stat.st_mode);
        Ok(EntryMetadata::new(
            kind,
            FileIdentity {
                device: u64::try_from(stat.st_dev).map_err(|_| FsBoundaryError::Internal)?,
                inode: stat.st_ino as u64,
                mount_id: capability.directory_identity.mount_id,
            },
            u64::try_from(stat.st_size).unwrap_or_default(),
            i128::from(stat.st_mtime) * 1_000_000_000_i128 + i128::from(stat.st_mtime_nsec),
        ))
    }

    fn read_bounded_relative_file(
        &self,
        capability: &DirectoryCapability,
        name: &OsStr,
        max_bytes: usize,
    ) -> Result<Vec<u8>, CapabilityFileError> {
        if max_bytes == 0 || max_bytes > 16 * 1024 * 1024 {
            return Err(FsBoundaryError::PathInvalid.into());
        }
        let verified = capability.verify(&self.provenance)?;
        let root = self.root(verified.root_id)?;
        Self::revalidate_root(root)?;
        if root.identity != verified.root_identity
            || file_identity(verified.fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?
                != verified.directory_identity
        {
            return Err(FsBoundaryError::RootChanged.into());
        }
        require_effective_read_search(verified.fd.as_fd())?;
        validate_single_name(name)?;
        let fd = match rustix::fs::openat(
            verified.fd,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(error) => {
                return Err(if is_symlink_at(verified.fd, name) {
                    FsBoundaryError::SymlinkForbidden.into()
                } else {
                    map_open_directory_error(error).into()
                });
            }
        };
        let stat = rustix::fs::fstat(fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?;
        if !FileType::from_raw_mode(stat.st_mode).is_file() {
            return Err(FsBoundaryError::Unavailable.into());
        }
        if u64::try_from(stat.st_size).map_or(true, |size| size > max_bytes as u64) {
            return Err(CapabilityFileError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(usize::try_from(stat.st_size).unwrap_or_default());
        std::fs::File::from(fd)
            .take(
                u64::try_from(max_bytes)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_to_end(&mut bytes)
            .map_err(|_| FsBoundaryError::Unavailable)?;
        if bytes.len() > max_bytes {
            Err(CapabilityFileError::TooLarge)
        } else {
            Ok(bytes)
        }
    }
}

const COPY_BUFFER_BYTES: usize = 256 * 1024;
const MAX_NFO_BYTES: usize = 1024 * 1024;

struct OpenedParent {
    fd: OwnedFd,
    name: OsString,
}

struct OpenedObservedFile {
    parent: OpenedParent,
    fd: OwnedFd,
    observed: ObservedFile,
}

impl OsCapabilityFs {
    fn open_file_parent(
        &self,
        locator: &FileLocator,
        writable: bool,
        create_missing: bool,
    ) -> Result<Option<OpenedParent>, FsBoundaryError> {
        let root = self.root(locator.root_id())?;
        Self::revalidate_root(root)?;
        if writable && root.access != RootAccess::ReadWrite {
            return Err(FsBoundaryError::Unavailable);
        }
        let mut components = locator.relative_path().components().collect::<Vec<_>>();
        let Some(name) = components.pop() else {
            return Err(FsBoundaryError::PathInvalid);
        };
        let mut current =
            rustix::io::dup(root.fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?;
        for component in components {
            let component = OsStr::new(component);
            match open_directory_at_optional(&current, component)? {
                Some(next) => current = next,
                None if create_missing => {
                    match rustix::fs::mkdirat(
                        current.as_fd(),
                        component,
                        Mode::from_raw_mode(0o755),
                    ) {
                        Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                        Err(_) => return Err(FsBoundaryError::Unavailable),
                    }
                    current = open_directory_at(&current, component)?;
                }
                None => return Ok(None),
            }
        }
        Ok(Some(OpenedParent {
            fd: current,
            name: OsString::from(name),
        }))
    }

    fn open_observed_file(
        &self,
        locator: &FileLocator,
    ) -> Result<Option<OpenedObservedFile>, FsBoundaryError> {
        let Some(parent) = self.open_file_parent(locator, false, false)? else {
            return Ok(None);
        };
        let fd = match rustix::fs::openat(
            parent.fd.as_fd(),
            &parent.name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(rustix::io::Errno::NOENT) => return Ok(None),
            Err(error) => {
                return Err(if is_symlink_at(&parent.fd, &parent.name) {
                    FsBoundaryError::SymlinkForbidden
                } else {
                    map_open_file_error(error)
                });
            }
        };
        let observed = observed_from_fd(locator.clone(), fd.as_fd())?;
        Ok(Some(OpenedObservedFile {
            parent,
            fd,
            observed,
        }))
    }

    fn source_for_operation(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<OpenedObservedFile, FileOperationError> {
        let Some(opened) = self.open_observed_file(operation.source().locator())? else {
            return Err(FileOperationError::SourceChanged);
        };
        if opened.observed != *operation.source() {
            return Err(FileOperationError::SourceChanged);
        }
        Ok(opened)
    }

    fn require_writable_root(&self, root_id: &RootId) -> Result<(), FileOperationError> {
        let root = self.root(root_id)?;
        Self::revalidate_root(root)?;
        if root.access == RootAccess::ReadWrite {
            Ok(())
        } else {
            Err(FileOperationError::OperationUnsupported)
        }
    }

    fn writable_destination_parent(
        &self,
        locator: &FileLocator,
    ) -> Result<OpenedParent, FileOperationError> {
        self.open_file_parent(locator, true, true)?
            .ok_or(FileOperationError::IoTemporary)
    }

    fn publish_bytes(
        &self,
        operation_id: Uuid,
        destination: &FileLocator,
        bytes: &[u8],
    ) -> Result<AppliedFile, FileOperationError> {
        let parent = self.writable_destination_parent(destination)?;
        let temporary_name = temporary_name(operation_id);
        let fd = create_temporary(&parent, &temporary_name)?;
        let mut file = std::fs::File::from(fd);
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            remove_temporary(&parent, &temporary_name);
            return Err(map_std_io(error));
        }
        if let Err(error) = publish_no_clobber(&parent, &temporary_name) {
            remove_temporary(&parent, &temporary_name);
            return Err(error);
        }
        rustix::fs::fsync(parent.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
        let identity = file_identity(file.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
        Ok(AppliedFile::from_parts(
            destination.clone(),
            identity,
            bytes.len() as u64,
            Sha256::digest(bytes).into(),
        ))
    }

    fn compensate_created(
        &self,
        operation: &VerifiedOperation,
    ) -> Result<CompensationOutcome, FileOperationError> {
        let Some(current) = self.open_observed_file(operation.applied().locator())? else {
            return Ok(CompensationOutcome::AlreadyAbsent);
        };
        let (sha256, size) = hash_fd(current.fd.as_fd())?;
        if !matches_applied(&current.observed, &sha256, size, operation.applied()) {
            return Ok(CompensationOutcome::ManualReview);
        }
        let quarantine = rollback_name(operation.id());
        if rename_no_replace(
            current.parent.fd.as_fd(),
            &current.parent.name,
            current.parent.fd.as_fd(),
            &quarantine,
        )
        .is_err()
        {
            return Ok(CompensationOutcome::ManualReview);
        }
        let quarantined = match open_named_observed(
            &current.parent,
            &quarantine,
            operation.applied().locator().clone(),
        ) {
            Ok(quarantined) => quarantined,
            Err(error) => {
                restore_quarantine(&current.parent, &quarantine);
                return Err(error);
            }
        };
        let Some(quarantined) = quarantined else {
            restore_quarantine(&current.parent, &quarantine);
            return Err(FileOperationError::IoTemporary);
        };
        let (quarantine_hash, quarantine_size) = match hash_fd(quarantined.fd.as_fd()) {
            Ok(facts) => facts,
            Err(error) => {
                restore_quarantine(&current.parent, &quarantine);
                return Err(error);
            }
        };
        if !matches_applied(
            &quarantined.observed,
            &quarantine_hash,
            quarantine_size,
            operation.applied(),
        ) {
            restore_quarantine(&current.parent, &quarantine);
            return Ok(CompensationOutcome::ManualReview);
        }

        if operation.kind() == VerifiedOperationKind::Move {
            return self.restore_move(operation, &current.parent, &quarantine);
        }
        rustix::fs::unlinkat(current.parent.fd.as_fd(), &quarantine, AtFlags::empty())
            .map_err(|_| FileOperationError::IoTemporary)?;
        rustix::fs::fsync(current.parent.fd.as_fd())
            .map_err(|_| FileOperationError::IoTemporary)?;
        Ok(CompensationOutcome::Removed)
    }

    fn restore_move(
        &self,
        operation: &VerifiedOperation,
        target_parent: &OpenedParent,
        quarantine: &OsStr,
    ) -> Result<CompensationOutcome, FileOperationError> {
        let Some(source) = operation.source() else {
            restore_quarantine(target_parent, quarantine);
            return Ok(CompensationOutcome::ManualReview);
        };
        if self.require_writable_root(source.root_id()).is_err() {
            restore_quarantine(target_parent, quarantine);
            return Ok(CompensationOutcome::ManualReview);
        }
        let Ok(Some(source_parent)) = self.open_file_parent(source, true, false) else {
            restore_quarantine(target_parent, quarantine);
            return Ok(CompensationOutcome::ManualReview);
        };
        if rename_no_replace(
            target_parent.fd.as_fd(),
            quarantine,
            source_parent.fd.as_fd(),
            &source_parent.name,
        )
        .is_err()
        {
            restore_quarantine(target_parent, quarantine);
            return Ok(CompensationOutcome::ManualReview);
        }
        rustix::fs::fsync(target_parent.fd.as_fd())
            .and_then(|()| rustix::fs::fsync(source_parent.fd.as_fd()))
            .map_err(|_| FileOperationError::IoTemporary)?;
        Ok(CompensationOutcome::Restored)
    }
}

impl OrganizationFs for OsCapabilityFs {
    fn preflight_target(
        &self,
        root: &RootId,
        path: &RelativePath,
    ) -> Result<TargetCapabilitySnapshot, FsBoundaryError> {
        let handle = self.root(root)?;
        if handle.access != RootAccess::ReadWrite {
            return Err(FsBoundaryError::Unavailable);
        }
        let identity = <Self as CapabilityFs>::preflight_directory(self, root, path)?;
        Ok(TargetCapabilitySnapshot {
            root_identity: identity.root_identity(),
            directory_identity: identity.directory_identity(),
        })
    }

    fn observe(&self, locator: &FileLocator) -> Result<Option<ObservedFile>, FsBoundaryError> {
        Ok(self
            .open_observed_file(locator)?
            .map(|opened| opened.observed))
    }

    fn inspect(&self, locator: &FileLocator) -> Result<Option<AppliedFile>, FsBoundaryError> {
        let Some(opened) = self.open_observed_file(locator)? else {
            return Ok(None);
        };
        let (sha256, size_bytes) = hash_fd(opened.fd.as_fd()).map_err(file_operation_boundary)?;
        if size_bytes != opened.observed.size_bytes() {
            return Err(FsBoundaryError::Unavailable);
        }
        Ok(Some(AppliedFile::from_parts(
            locator.clone(),
            opened.observed.identity(),
            size_bytes,
            sha256,
        )))
    }

    fn copy_no_clobber(
        &self,
        operation: &FileOperationSpec,
        stop: &ProcessingStopToken,
    ) -> Result<AppliedFile, FileOperationError> {
        if stop.is_stopped() {
            return Err(FileOperationError::IoTemporary);
        }
        let source = self.source_for_operation(operation)?;
        let parent = self.writable_destination_parent(operation.destination())?;
        let temporary_name = temporary_name(operation.id());
        let temporary_fd = create_temporary(&parent, &temporary_name)?;
        let mut target = std::fs::File::from(temporary_fd);
        let mut source_file = std::fs::File::from(source.fd);
        let mut digest = Sha256::new();
        let mut size_bytes = 0_u64;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
        loop {
            if stop.is_stopped() {
                remove_temporary(&parent, &temporary_name);
                return Err(FileOperationError::IoTemporary);
            }
            let read = match source_file.read(&mut buffer) {
                Ok(read) => read,
                Err(error) => {
                    remove_temporary(&parent, &temporary_name);
                    return Err(map_std_io(error));
                }
            };
            if read == 0 {
                break;
            }
            if stop.is_stopped() {
                remove_temporary(&parent, &temporary_name);
                return Err(FileOperationError::IoTemporary);
            }
            if let Err(error) = target.write_all(&buffer[..read]) {
                remove_temporary(&parent, &temporary_name);
                return Err(map_std_io(error));
            }
            digest.update(&buffer[..read]);
            size_bytes = size_bytes
                .checked_add(read as u64)
                .ok_or(FileOperationError::IoTemporary)?;
        }
        let final_source =
            observed_from_fd(operation.source().locator().clone(), source_file.as_fd())
                .map_err(FileOperationError::from)?;
        if final_source != *operation.source() || size_bytes != operation.source().size_bytes() {
            remove_temporary(&parent, &temporary_name);
            return Err(FileOperationError::SourceChanged);
        }
        if let Err(error) = target.sync_all() {
            remove_temporary(&parent, &temporary_name);
            return Err(map_std_io(error));
        }
        if stop.is_stopped() {
            remove_temporary(&parent, &temporary_name);
            return Err(FileOperationError::IoTemporary);
        }
        if let Err(error) = publish_no_clobber(&parent, &temporary_name) {
            remove_temporary(&parent, &temporary_name);
            return Err(error);
        }
        rustix::fs::fsync(parent.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
        let identity =
            file_identity(target.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
        Ok(AppliedFile::from_parts(
            operation.destination().clone(),
            identity,
            size_bytes,
            digest.finalize().into(),
        ))
    }

    fn move_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        self.require_writable_root(operation.source().locator().root_id())?;
        let source = self.source_for_operation(operation)?;
        let destination = self.writable_destination_parent(operation.destination())?;
        let destination_identity =
            file_identity(destination.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
        if !same_filesystem(operation.source().identity(), destination_identity) {
            return Err(FileOperationError::CompositeMoveRequired);
        }
        rename_no_replace(
            source.parent.fd.as_fd(),
            &source.parent.name,
            destination.fd.as_fd(),
            &destination.name,
        )
        .map_err(map_rename_move_error)?;
        rustix::fs::fsync(source.parent.fd.as_fd())
            .and_then(|()| rustix::fs::fsync(destination.fd.as_fd()))
            .map_err(|_| FileOperationError::IoTemporary)?;
        let source_after =
            observed_from_fd(operation.source().locator().clone(), source.fd.as_fd())
                .map_err(FileOperationError::from)?;
        let applied = opened_applied(self, operation.destination())?;
        if source_after != *operation.source()
            || applied.identity() != operation.source().identity()
        {
            let rollback = VerifiedOperation::new(
                operation.id(),
                VerifiedOperationKind::Move,
                Some(operation.source().locator().clone()),
                applied,
            );
            return match self.compensate_created(&rollback)? {
                CompensationOutcome::Restored => Err(FileOperationError::SourceChanged),
                CompensationOutcome::Removed
                | CompensationOutcome::AlreadyAbsent
                | CompensationOutcome::ManualReview => Err(FileOperationError::IoTemporary),
            };
        }
        Ok(applied)
    }

    fn hardlink_no_clobber(
        &self,
        operation: &FileOperationSpec,
    ) -> Result<AppliedFile, FileOperationError> {
        let source = self.source_for_operation(operation)?;
        let destination = self.writable_destination_parent(operation.destination())?;
        let destination_identity =
            file_identity(destination.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
        if !same_filesystem(operation.source().identity(), destination_identity) {
            return Err(FileOperationError::HardlinkCrossDevice);
        }
        rustix::fs::linkat(
            source.parent.fd.as_fd(),
            &source.parent.name,
            destination.fd.as_fd(),
            &destination.name,
            AtFlags::empty(),
        )
        .map_err(map_link_error)?;
        rustix::fs::fsync(destination.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
        let source_after =
            observed_from_fd(operation.source().locator().clone(), source.fd.as_fd())
                .map_err(FileOperationError::from)?;
        let applied = opened_applied(self, operation.destination())?;
        if source_after != *operation.source()
            || applied.identity() != operation.source().identity()
        {
            let rollback = VerifiedOperation::new(
                operation.id(),
                VerifiedOperationKind::Hardlink,
                None,
                applied,
            );
            return match self.compensate_created(&rollback)? {
                CompensationOutcome::Removed | CompensationOutcome::AlreadyAbsent => {
                    Err(FileOperationError::SourceChanged)
                }
                CompensationOutcome::Restored | CompensationOutcome::ManualReview => {
                    Err(FileOperationError::IoTemporary)
                }
            };
        }
        Ok(applied)
    }

    fn write_new_nfo(
        &self,
        operation: &NfoOperationSpec,
        bytes: &[u8],
    ) -> Result<AppliedFile, FileOperationError> {
        if bytes.len() > MAX_NFO_BYTES {
            return Err(FileOperationError::OperationUnsupported);
        }
        self.publish_bytes(operation.id(), operation.destination(), bytes)
    }

    fn remove_verified_source(
        &self,
        operation_id: Uuid,
        source: &ObservedFile,
        expected_sha256: &[u8; 32],
    ) -> Result<(), FileOperationError> {
        self.require_writable_root(source.locator().root_id())?;
        let quarantine = source_removal_name(operation_id);
        let parent = self
            .open_file_parent(source.locator(), true, false)?
            .ok_or(FileOperationError::SourceChanged)?;
        if let Some(existing_quarantine) =
            open_named_observed(&parent, &quarantine, source.locator().clone())?
        {
            if self.open_observed_file(source.locator())?.is_some() {
                return Err(FileOperationError::SourceChanged);
            }
            let (sha256, size) = hash_fd(existing_quarantine.fd.as_fd())?;
            if existing_quarantine.observed != *source
                || size != source.size_bytes()
                || &sha256 != expected_sha256
            {
                return Err(FileOperationError::SourceChanged);
            }
            rustix::fs::unlinkat(parent.fd.as_fd(), &quarantine, AtFlags::empty())
                .map_err(|_| FileOperationError::IoTemporary)?;
            rustix::fs::fsync(parent.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
            return Ok(());
        }
        let Some(current) = self.open_observed_file(source.locator())? else {
            return Ok(());
        };
        let (sha256, size) = hash_fd(current.fd.as_fd())?;
        if current.observed != *source || size != source.size_bytes() || &sha256 != expected_sha256
        {
            return Err(FileOperationError::SourceChanged);
        }
        rename_no_replace(
            current.parent.fd.as_fd(),
            &current.parent.name,
            current.parent.fd.as_fd(),
            &quarantine,
        )
        .map_err(map_publish_error)?;
        let quarantined =
            open_named_observed(&current.parent, &quarantine, source.locator().clone())?
                .ok_or(FileOperationError::IoTemporary)?;
        let (quarantine_hash, quarantine_size) = hash_fd(quarantined.fd.as_fd())?;
        if quarantined.observed != *source
            || quarantine_size != source.size_bytes()
            || &quarantine_hash != expected_sha256
        {
            restore_quarantine(&current.parent, &quarantine);
            return Err(FileOperationError::SourceChanged);
        }
        rustix::fs::unlinkat(current.parent.fd.as_fd(), &quarantine, AtFlags::empty())
            .map_err(|_| FileOperationError::IoTemporary)?;
        rustix::fs::fsync(current.parent.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)
    }

    fn compensate(
        &self,
        operation: &VerifiedOperation,
    ) -> Result<CompensationOutcome, FileOperationError> {
        self.compensate_created(operation)
    }
}

fn open_directory_at_optional(
    parent: &OwnedFd,
    name: &OsStr,
) -> Result<Option<OwnedFd>, FsBoundaryError> {
    validate_single_name(name)?;
    match rustix::fs::openat(
        parent.as_fd(),
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(Some(fd)),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(if is_symlink_at(parent, name) {
            FsBoundaryError::SymlinkForbidden
        } else if error == rustix::io::Errno::NOTDIR {
            FsBoundaryError::PathInvalid
        } else {
            map_open_directory_error(error)
        }),
    }
}

fn observed_from_fd(locator: FileLocator, fd: impl AsFd) -> Result<ObservedFile, FsBoundaryError> {
    let stat = rustix::fs::fstat(fd.as_fd()).map_err(|_| FsBoundaryError::Unavailable)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(FsBoundaryError::Unavailable);
    }
    let identity = file_identity(fd).map_err(|_| FsBoundaryError::Unavailable)?;
    let size_bytes = u64::try_from(stat.st_size).map_err(|_| FsBoundaryError::Internal)?;
    let modified_at_ns =
        i128::from(stat.st_mtime) * 1_000_000_000_i128 + i128::from(stat.st_mtime_nsec);
    Ok(ObservedFile::from_parts(
        locator,
        identity,
        size_bytes,
        modified_at_ns,
    ))
}

fn open_named_observed(
    parent: &OpenedParent,
    name: &OsStr,
    locator: FileLocator,
) -> Result<Option<OpenedObservedFile>, FileOperationError> {
    let fd = match rustix::fs::openat(
        parent.fd.as_fd(),
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT) => return Ok(None),
        Err(_) => return Err(FileOperationError::IoTemporary),
    };
    let observed = observed_from_fd(locator, fd.as_fd()).map_err(FileOperationError::from)?;
    let parent_fd =
        rustix::io::dup(parent.fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
    Ok(Some(OpenedObservedFile {
        parent: OpenedParent {
            fd: parent_fd,
            name: name.to_os_string(),
        },
        fd,
        observed,
    }))
}

fn opened_applied(
    fs: &OsCapabilityFs,
    locator: &FileLocator,
) -> Result<AppliedFile, FileOperationError> {
    let Some(opened) = fs.open_observed_file(locator)? else {
        return Err(FileOperationError::IoTemporary);
    };
    let (sha256, size_bytes) = hash_fd(opened.fd.as_fd())?;
    if size_bytes != opened.observed.size_bytes() {
        return Err(FileOperationError::SourceChanged);
    }
    Ok(AppliedFile::from_parts(
        locator.clone(),
        opened.observed.identity(),
        size_bytes,
        sha256,
    ))
}

fn hash_fd(fd: impl AsFd) -> Result<([u8; 32], u64), FileOperationError> {
    let before = rustix::fs::fstat(fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
    let duplicate = rustix::io::dup(fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
    let mut file = std::fs::File::from(duplicate);
    let mut digest = Sha256::new();
    let mut size_bytes = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
    loop {
        let read = file.read(&mut buffer).map_err(map_std_io)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size_bytes = size_bytes
            .checked_add(read as u64)
            .ok_or(FileOperationError::IoTemporary)?;
    }
    let after = rustix::fs::fstat(fd.as_fd()).map_err(|_| FileOperationError::IoTemporary)?;
    if before.st_dev != after.st_dev
        || before.st_ino != after.st_ino
        || before.st_size != after.st_size
        || before.st_mtime != after.st_mtime
        || before.st_mtime_nsec != after.st_mtime_nsec
    {
        return Err(FileOperationError::SourceChanged);
    }
    Ok((digest.finalize().into(), size_bytes))
}

fn matches_applied(
    observed: &ObservedFile,
    sha256: &[u8; 32],
    size_bytes: u64,
    applied: &AppliedFile,
) -> bool {
    observed.identity() == applied.identity()
        && observed.size_bytes() == applied.size_bytes()
        && size_bytes == applied.size_bytes()
        && sha256 == applied.sha256()
}

fn same_filesystem(source: FileIdentity, destination: FileIdentity) -> bool {
    source.device == destination.device
        && match (source.mount_id, destination.mount_id) {
            (Some(source), Some(destination)) => source == destination,
            _ => true,
        }
}

fn create_temporary(parent: &OpenedParent, name: &OsStr) -> Result<OwnedFd, FileOperationError> {
    rustix::fs::openat(
        parent.fd.as_fd(),
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::EXIST {
            FileOperationError::AmbiguousState
        } else {
            FileOperationError::IoTemporary
        }
    })
}

fn temporary_name(operation_id: Uuid) -> OsString {
    OsString::from(format!(".mediaflow-{operation_id}.tmp"))
}

fn rollback_name(operation_id: Uuid) -> OsString {
    OsString::from(format!(".mediaflow-{operation_id}.rollback"))
}

fn source_removal_name(operation_id: Uuid) -> OsString {
    OsString::from(format!(".mediaflow-{operation_id}.source-removal"))
}

fn publish_no_clobber(
    parent: &OpenedParent,
    temporary_name: &OsStr,
) -> Result<(), FileOperationError> {
    rename_no_replace(
        parent.fd.as_fd(),
        temporary_name,
        parent.fd.as_fd(),
        &parent.name,
    )
    .map_err(map_publish_error)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn rename_no_replace(
    source_parent: impl AsFd,
    source_name: &OsStr,
    target_parent: impl AsFd,
    target_name: &OsStr,
) -> rustix::io::Result<()> {
    rustix::fs::renameat_with(
        source_parent,
        source_name,
        target_parent,
        target_name,
        rustix::fs::RenameFlags::NOREPLACE,
    )
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn rename_no_replace(
    _source_parent: impl AsFd,
    _source_name: &OsStr,
    _target_parent: impl AsFd,
    _target_name: &OsStr,
) -> rustix::io::Result<()> {
    Err(rustix::io::Errno::NOTSUP)
}

fn restore_quarantine(parent: &OpenedParent, quarantine: &OsStr) {
    let _ = rename_no_replace(
        parent.fd.as_fd(),
        quarantine,
        parent.fd.as_fd(),
        &parent.name,
    );
}

fn remove_temporary(parent: &OpenedParent, name: &OsStr) {
    let _ = rustix::fs::unlinkat(parent.fd.as_fd(), name, AtFlags::empty());
}

fn map_open_file_error(error: rustix::io::Errno) -> FsBoundaryError {
    if error == rustix::io::Errno::LOOP {
        FsBoundaryError::SymlinkForbidden
    } else {
        FsBoundaryError::Unavailable
    }
}

fn map_publish_error(error: rustix::io::Errno) -> FileOperationError {
    if error == rustix::io::Errno::EXIST {
        FileOperationError::TargetExists
    } else if matches!(error, rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP) {
        FileOperationError::OperationUnsupported
    } else {
        FileOperationError::IoTemporary
    }
}

fn map_rename_move_error(error: rustix::io::Errno) -> FileOperationError {
    if error == rustix::io::Errno::XDEV {
        FileOperationError::CompositeMoveRequired
    } else {
        map_publish_error(error)
    }
}

fn map_link_error(error: rustix::io::Errno) -> FileOperationError {
    if error == rustix::io::Errno::XDEV {
        FileOperationError::HardlinkCrossDevice
    } else {
        map_publish_error(error)
    }
}

fn map_std_io(_error: std::io::Error) -> FileOperationError {
    FileOperationError::IoTemporary
}

fn file_operation_boundary(error: FileOperationError) -> FsBoundaryError {
    match error {
        FileOperationError::PathOutsideRoot => FsBoundaryError::PathInvalid,
        FileOperationError::RootChanged => FsBoundaryError::RootChanged,
        FileOperationError::SourceChanged
        | FileOperationError::TargetExists
        | FileOperationError::HardlinkCrossDevice
        | FileOperationError::CompositeMoveRequired
        | FileOperationError::OperationUnsupported
        | FileOperationError::AmbiguousState
        | FileOperationError::IoTemporary => FsBoundaryError::Unavailable,
    }
}

struct OsDirectoryEntryStream {
    directory: rustix::fs::Dir,
}

impl DirectoryEntryStream for OsDirectoryEntryStream {
    fn next_entry(&mut self) -> Result<Option<DirectoryEntry>, FsBoundaryError> {
        loop {
            let Some(entry) = self.directory.read() else {
                return Ok(None);
            };
            let entry = entry.map_err(|_| FsBoundaryError::Unavailable)?;
            let bytes = entry.file_name().to_bytes();
            if matches!(bytes, b"." | b"..") {
                continue;
            }
            return Ok(Some(DirectoryEntry::new(os_string_from_bytes(bytes))));
        }
    }
}

fn safe_name_display(name: &OsStr) -> String {
    String::from_utf8_lossy(os_str_bytes(name))
        .chars()
        .map(|character| {
            if character.is_control() || matches!(character, '/' | '\\') {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

fn open_root(path: &Path) -> Result<OwnedFd, FsBoundaryError> {
    let mut current = rustix::fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| FsBoundaryError::Unavailable)?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => current = open_directory_at(&current, name)?,
            _ => return Err(FsBoundaryError::PathInvalid),
        }
    }
    Ok(current)
}

#[cfg(target_os = "linux")]
fn validate_root_mode(fd: impl AsFd, mode: RunMode) -> Result<(), FsBoundaryError> {
    if !matches!(mode, RunMode::Production) {
        return Ok(());
    }
    use rustix::fs::{StatxAttributes, StatxFlags};
    let stat = rustix::fs::statx(
        fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        StatxFlags::MNT_ID | StatxFlags::BASIC_STATS,
    )
    .map_err(|_| FsBoundaryError::Unavailable)?;
    if stat.stx_mask & StatxFlags::MNT_ID.bits() == 0
        || !stat
            .stx_attributes_mask
            .contains(StatxAttributes::MOUNT_ROOT)
        || !stat.stx_attributes.contains(StatxAttributes::MOUNT_ROOT)
    {
        return Err(FsBoundaryError::Unavailable);
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
fn validate_root_mode(_fd: impl AsFd, _mode: RunMode) -> Result<(), FsBoundaryError> {
    Ok(())
}

fn open_directory_at(parent: &OwnedFd, name: &OsStr) -> Result<OwnedFd, FsBoundaryError> {
    validate_single_name(name)?;
    match rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(fd),
        Err(error) => {
            if is_symlink_at(parent, name) {
                Err(FsBoundaryError::SymlinkForbidden)
            } else {
                Err(map_open_directory_error(error))
            }
        }
    }
}

fn is_symlink_at(parent: &OwnedFd, name: &OsStr) -> bool {
    rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode).is_symlink())
}

fn require_effective_read_search(fd: impl AsFd) -> Result<(), FsBoundaryError> {
    rustix::fs::accessat(fd, ".", Access::READ_OK | Access::EXEC_OK, AtFlags::EACCESS)
        .map_err(|_| FsBoundaryError::Unavailable)
}

fn bounded_readability_probe(fd: impl AsFd) -> Result<(), FsBoundaryError> {
    let mut directory = rustix::fs::Dir::read_from(fd).map_err(|_| FsBoundaryError::Unavailable)?;
    if let Some(entry) = directory.read() {
        entry.map_err(|_| FsBoundaryError::Unavailable)?;
        #[cfg(test)]
        PROBE_ENTRY_VISITS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
static PROBE_ENTRY_VISITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn root_identity(fd: impl AsFd) -> rustix::io::Result<RootIdentity> {
    let identity = file_identity(fd)?;
    Ok(RootIdentity {
        device: identity.device,
        inode: identity.inode,
        mount_id: identity.mount_id,
    })
}

fn file_identity(fd: impl AsFd) -> rustix::io::Result<FileIdentity> {
    let fd = fd.as_fd();
    let stat = rustix::fs::fstat(fd)?;
    Ok(FileIdentity {
        device: u64::try_from(stat.st_dev).map_err(|_| rustix::io::Errno::OVERFLOW)?,
        inode: stat.st_ino as u64,
        mount_id: mount_id(fd)?,
    })
}

#[cfg(target_os = "linux")]
fn mount_id(fd: std::os::fd::BorrowedFd<'_>) -> rustix::io::Result<Option<u64>> {
    use rustix::fs::StatxFlags;
    match rustix::fs::statx(
        fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        StatxFlags::MNT_ID | StatxFlags::BASIC_STATS,
    ) {
        Ok(stat) => Ok(Some(stat.stx_mnt_id)),
        Err(rustix::io::Errno::NOSYS) => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(not(target_os = "linux"))]
#[allow(clippy::unnecessary_wraps)]
fn mount_id(_fd: std::os::fd::BorrowedFd<'_>) -> rustix::io::Result<Option<u64>> {
    Ok(None)
}

fn mode_kind(mode: rustix::fs::RawMode) -> EntryKind {
    let kind = FileType::from_raw_mode(mode);
    if kind.is_file() {
        EntryKind::File
    } else if kind.is_dir() {
        EntryKind::Directory
    } else if kind.is_symlink() {
        EntryKind::Symlink
    } else {
        EntryKind::Other
    }
}

fn map_open_directory_error(error: rustix::io::Errno) -> FsBoundaryError {
    if matches!(error, rustix::io::Errno::LOOP) {
        FsBoundaryError::SymlinkForbidden
    } else {
        FsBoundaryError::Unavailable
    }
}

fn validate_single_name(name: &OsStr) -> Result<(), FsBoundaryError> {
    let bytes = os_str_bytes(name);
    if bytes.is_empty()
        || matches!(bytes, b"." | b"..")
        || bytes.contains(&b'/')
        || bytes.contains(&0)
    {
        Err(FsBoundaryError::PathInvalid)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn os_str_bytes(value: &OsStr) -> &[u8] {
    use std::os::unix::ffi::OsStrExt;
    value.as_bytes()
}

#[cfg(unix)]
fn os_string_from_bytes(value: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(value.to_vec())
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::atomic::Ordering;

    use serde_json::json;

    use super::{OsCapabilityFs, PROBE_ENTRY_VISITS, os_str_bytes, os_string_from_bytes};
    use crate::bootstrap::config::RunMode;
    use crate::discovery::capability::{CapabilityFs, DeploymentRootSet};
    use crate::discovery::model::{RelativePath, RootId};

    #[test]
    fn unix_os_string_adapter_preserves_non_utf8_bytes_without_lossy_text() {
        let raw = [b'n', b'o', b'n', 0xff, b'u', b'8'];
        let name = os_string_from_bytes(&raw);

        assert_eq!(os_str_bytes(&name), raw);
        assert_ne!(name.to_string_lossy().as_bytes(), raw);
    }

    #[test]
    fn directory_operations_bound_readability_probes_without_repeated_enumeration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let names = (0..40)
            .map(|index| format!("entry-{index:02}"))
            .collect::<Vec<_>>();
        for name in &names {
            std::fs::write(root.join(name), b"data").unwrap();
        }
        let config = temp.path().join("deployment-roots.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({"roots":[{
                "id":"incoming",
                "label":"Incoming",
                "container_path":root,
                "access":"read-only"
            }]}))
            .unwrap(),
        )
        .unwrap();
        let declarations = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
        let fs = OsCapabilityFs::open(declarations.declarations(), RunMode::Development).unwrap();
        let root_id = RootId::parse("incoming").unwrap();

        PROBE_ENTRY_VISITS.store(0, Ordering::SeqCst);
        let identity = fs
            .preflight_directory(&root_id, &RelativePath::parse(".").unwrap())
            .unwrap();
        let preflight_visits = PROBE_ENTRY_VISITS.load(Ordering::SeqCst);

        PROBE_ENTRY_VISITS.store(0, Ordering::SeqCst);
        for name in &names {
            fs.metadata_no_follow(identity.capability(), std::ffi::OsStr::new(name))
                .unwrap();
        }
        let metadata_visits = PROBE_ENTRY_VISITS.load(Ordering::SeqCst);

        PROBE_ENTRY_VISITS.store(0, Ordering::SeqCst);
        let first = fs.read_directory(identity.capability()).unwrap();
        let read_visits = PROBE_ENTRY_VISITS.load(Ordering::SeqCst);
        let second = fs.read_directory(identity.capability()).unwrap();

        assert!(
            preflight_visits <= 1,
            "preflight visited {preflight_visits}"
        );
        assert_eq!(metadata_visits, 0);
        assert_eq!(read_visits, 0);
        assert_eq!(first.len(), names.len());
        assert_eq!(second.len(), names.len());
    }
}
