use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::discovery::capability::{CapabilityFs, DeploymentRootsFingerprint};
use crate::discovery::model::{
    CreateInboxCommand, DeploymentRootView, DirectoryIdentity, FsBoundaryError, InboxDirectoryPage,
    InboxDirectoryView, InboxHealth, InboxPreflightView, PreflightInboxCommand, RelativePath,
    RootId,
};
use crate::discovery::policy::{
    DiscoveryPolicyStore, DiscoveryPolicyView, PutDiscoveryPolicyCommand,
};
use crate::discovery::scanner::ScanSource;
use crate::discovery::store::{DiscoveryStore, NewInbox, StoredInbox};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;
use crate::shared::page::PageRequest;

#[async_trait]
/// 部署根目录检查与持久化收件箱目录操作。
pub trait DiscoveryUseCases: Send + Sync {
    /// 按稳定根目录 ID 顺序返回已配置根目录，不暴露主机路径。
    ///
    /// # Errors
    ///
    /// 配置源不可用时，实现可能返回 [`AppError`]。
    async fn list_roots(&self) -> Result<Vec<DeploymentRootView>, AppError>;
    /// 校验并打开候选收件箱，但不持久化它。
    ///
    /// 结果会报告可读性以及与现有收件箱的重叠情况。
    ///
    /// # Errors
    ///
    /// 当根目录/路径语法无效、根目录未知或不可用、遍历被禁止，或检查重叠时发生数据库失败，
    /// 返回 [`AppError`]。
    async fn preflight(
        &self,
        command: PreflightInboxCommand,
    ) -> Result<InboxPreflightView, AppError>;
    /// 重新验证候选目录，并原子持久化其身份快照。
    ///
    /// # Errors
    ///
    /// 当路径无效/不安全、根目录未知或已改变、与现有收件箱重叠，或提交新收件箱失败时，
    /// 返回 [`AppError`]。
    async fn create_inbox(
        &self,
        command: CreateInboxCommand,
    ) -> Result<InboxDirectoryView, AppError>;
    /// 加载收件箱，重新验证其根目录/目录身份，并持久化观测到的健康状态。
    ///
    /// # Errors
    ///
    /// 当收件箱不存在或无法读写其记录/健康更新时返回 [`AppError`]。能力重新验证失败会表示为
    /// `Unavailable`，而不是错误。
    async fn get_inbox(&self, id: Uuid) -> Result<InboxDirectoryView, AppError>;
}

#[derive(Clone)]
/// 协调不可变根目录视图、能力安全的文件系统访问与收件箱持久化。
pub struct DiscoveryService {
    roots: Arc<BTreeMap<RootId, DeploymentRootView>>,
    fs: Arc<dyn CapabilityFs>,
    store: DiscoveryStore,
    policy_store: DiscoveryPolicyStore,
    config_guard: Option<Arc<DeploymentRootsGuard>>,
}

struct DeploymentRootsGuard {
    path: PathBuf,
    fingerprint: DeploymentRootsFingerprint,
}

impl DiscoveryService {
    #[must_use]
    /// 根据已校验根目录、其文件系统适配器和 `SQLite` 连接池创建服务。
    ///
    /// 应用 [`Self::with_config_guard`] 前不会启用配置变更保护。
    pub fn new(
        roots: BTreeMap<RootId, DeploymentRootView>,
        fs: Arc<dyn CapabilityFs>,
        pool: sqlx::SqlitePool,
    ) -> Self {
        Self {
            roots: Arc::new(roots),
            fs,
            store: DiscoveryStore::new(pool.clone()),
            policy_store: DiscoveryPolicyStore::new(pool),
            config_guard: None,
        }
    }

    #[must_use]
    /// 创建仅用于策略存储测试/维护的服务；文件系统用例会失败关闭。
    pub fn policy_only(pool: sqlx::SqlitePool) -> Self {
        Self::new(BTreeMap::new(), Arc::new(PolicyOnlyFs), pool)
    }

    /// 返回一个收件箱的版本化发现策略。
    ///
    /// # Errors
    ///
    /// 收件箱不存在或数据库查询失败时返回 [`AppError`]。
    pub async fn get_policy(&self, inbox_id: Uuid) -> Result<DiscoveryPolicyView, AppError> {
        self.policy_store.get(inbox_id).await
    }

    /// 在 `expected_version` 匹配时完整替换一个收件箱的发现策略。
    ///
    /// # Errors
    ///
    /// 值越界、版本冲突、收件箱不存在或数据库失败时返回 [`AppError`]。
    pub async fn put_policy(
        &self,
        inbox_id: Uuid,
        command: PutDiscoveryPolicyCommand,
        expected_version: i64,
    ) -> Result<DiscoveryPolicyView, AppError> {
        self.policy_store
            .put(inbox_id, command, expected_version)
            .await
    }

    #[must_use]
    /// 当 `path` 不再匹配已加载的根目录指纹时，关闭扫描源创建。
    pub fn with_config_guard(
        mut self,
        path: PathBuf,
        fingerprint: DeploymentRootsFingerprint,
    ) -> Self {
        self.config_guard = Some(Arc::new(DeploymentRootsGuard { path, fingerprint }));
        self
    }

    #[must_use]
    /// 返回受保护的部署根目录文件是否仍具有已加载的身份和字节内容。
    ///
    /// 未受保护的服务返回 `true`；读取错误或任何快照变化返回 `false`。
    pub fn configuration_is_current(&self) -> bool {
        self.config_guard
            .as_ref()
            .is_none_or(|guard| guard.fingerprint.matches_path(&guard.path))
    }

    /// 仅在配置和持久化身份重新验证后打开扫描源。
    ///
    /// # Errors
    ///
    /// 当配置已改变、收件箱不存在、目录无法安全打开、根目录/目录身份不同于存储快照，
    /// 或 `SQLite` 无法加载收件箱时返回 [`AppError`]。
    pub async fn open_scan_source(&self, id: Uuid) -> Result<ScanSource, AppError> {
        if !self.configuration_is_current() {
            return Err(AppError::new(
                ErrorCode::RootUnavailable,
                "deployment root configuration changed",
            ));
        }
        let stored = self.store.get(id).await?;
        let identity = self
            .fs
            .preflight_directory(&stored.root_id, &stored.relative_path)
            .map_err(|error| {
                boundary_error(error, Some(&stored.root_id), Some(&stored.relative_path))
            })?;
        if !identity_matches_stored(&identity, &stored) {
            return Err(AppError::new(
                ErrorCode::RootUnavailable,
                "inbox capability identity changed",
            ));
        }
        Ok(ScanSource {
            inbox_directory_id: id,
            capability: identity.into_capability(),
            fs: Arc::clone(&self.fs),
        })
    }

    /// 加载一页收件箱，逐个重新验证，并持久化每个结果健康值。每个健康更新独立提交。
    /// 如果较后的更新失败，本方法返回错误而不返回页面；此前项目已提交的健康变化仍可见。
    ///
    /// # Errors
    ///
    /// 当游标无效、存储数据格式错误/损坏、查询失败或任何健康更新失败时返回 [`AppError`]。
    /// 文件系统重新验证失败会变为不可用项目。
    pub async fn list_inboxes(&self, page: PageRequest) -> Result<InboxDirectoryPage, AppError> {
        let stored = self.store.list(&page).await?;
        let mut items = Vec::with_capacity(stored.items.len());
        for inbox in stored.items {
            items.push(self.revalidate(inbox).await?);
        }
        Ok(InboxDirectoryPage {
            items,
            next_cursor: stored.next_cursor,
        })
    }

    fn parse_command(command: &PreflightInboxCommand) -> Result<(RootId, RelativePath), AppError> {
        let root_id =
            RootId::parse(&command.root_id).map_err(|error| boundary_error(error, None, None))?;
        let relative_path = RelativePath::parse(&command.relative_path)
            .map_err(|error| boundary_error(error, Some(&root_id), None))?;
        Ok((root_id, relative_path))
    }

    async fn revalidate(&self, stored: StoredInbox) -> Result<InboxDirectoryView, AppError> {
        let checked_at = chrono::Utc::now().timestamp_micros();
        let health = match self
            .fs
            .preflight_directory(&stored.root_id, &stored.relative_path)
        {
            Ok(identity) if identity_matches_stored(&identity, &stored) => InboxHealth::Available,
            Ok(_) | Err(_) => InboxHealth::Unavailable,
        };
        self.store
            .update_health(stored.id, health, checked_at)
            .await?;
        Ok(InboxDirectoryView {
            id: stored.id,
            root_id: stored.root_id,
            relative_path: stored.relative_path,
            health,
            last_checked_at: format_timestamp(checked_at),
        })
    }
}

struct PolicyOnlyFs;

impl CapabilityFs for PolicyOnlyFs {
    fn preflight_directory(
        &self,
        _root: &RootId,
        _relative: &RelativePath,
    ) -> Result<crate::discovery::model::DirectoryIdentity, FsBoundaryError> {
        Err(FsBoundaryError::Internal)
    }

    fn read_directory(
        &self,
        _capability: &crate::discovery::model::DirectoryCapability,
    ) -> Result<Vec<crate::discovery::model::DirectoryEntry>, FsBoundaryError> {
        Err(FsBoundaryError::Internal)
    }

    fn metadata_no_follow(
        &self,
        _capability: &crate::discovery::model::DirectoryCapability,
        _name: &std::ffi::OsStr,
    ) -> Result<crate::discovery::model::EntryMetadata, FsBoundaryError> {
        Err(FsBoundaryError::Internal)
    }
}

#[async_trait]
impl DiscoveryUseCases for DiscoveryService {
    async fn list_roots(&self) -> Result<Vec<DeploymentRootView>, AppError> {
        Ok(self.roots.values().cloned().collect())
    }

    async fn preflight(
        &self,
        command: PreflightInboxCommand,
    ) -> Result<InboxPreflightView, AppError> {
        let (root_id, relative_path) = Self::parse_command(&command)?;
        if !self.roots.contains_key(&root_id) {
            return Err(boundary_error(
                FsBoundaryError::RootNotFound,
                Some(&root_id),
                Some(&relative_path),
            ));
        }
        self.fs
            .preflight_directory(&root_id, &relative_path)
            .map_err(|error| boundary_error(error, Some(&root_id), Some(&relative_path)))?;
        let overlaps_existing = self.store.has_overlap(&root_id, &relative_path).await?;
        Ok(InboxPreflightView {
            root_id,
            relative_path,
            readable: true,
            overlaps_existing,
        })
    }

    async fn create_inbox(
        &self,
        command: CreateInboxCommand,
    ) -> Result<InboxDirectoryView, AppError> {
        let (root_id, relative_path) = Self::parse_command(&command)?;
        if !self.roots.contains_key(&root_id) {
            return Err(boundary_error(
                FsBoundaryError::RootNotFound,
                Some(&root_id),
                Some(&relative_path),
            ));
        }
        let identity = self
            .fs
            .preflight_directory(&root_id, &relative_path)
            .map_err(|error| boundary_error(error, Some(&root_id), Some(&relative_path)))?;
        let now = chrono::Utc::now().timestamp_micros();
        let root_snapshot = identity.root_identity().durable_snapshot_bytes();
        let directory_snapshot = identity.directory_identity().durable_snapshot_bytes();
        let stored = self
            .store
            .create_immediate(NewInbox {
                id: new_id(),
                root_id: &root_id,
                relative_path: &relative_path,
                root_identity: &root_snapshot,
                directory_identity: &directory_snapshot,
                now_us: now,
            })
            .await
            .map_err(|error| {
                if error.code() == ErrorCode::InboxOverlap {
                    error
                        .with_detail("root_id", root_id.as_str())
                        .with_detail("relative_path", relative_path.as_str())
                } else {
                    error
                }
            })?;
        Ok(InboxDirectoryView {
            id: stored.id,
            root_id: stored.root_id,
            relative_path: stored.relative_path,
            health: stored.health,
            last_checked_at: format_timestamp(stored.last_checked_at_us),
        })
    }

    async fn get_inbox(&self, id: Uuid) -> Result<InboxDirectoryView, AppError> {
        self.revalidate(self.store.get(id).await?).await
    }
}

fn identity_matches_stored(identity: &DirectoryIdentity, stored: &StoredInbox) -> bool {
    identity
        .root_identity()
        .matches_durable_snapshot(&stored.root_identity)
        && identity
            .directory_identity()
            .matches_durable_snapshot(&stored.directory_identity)
}

fn format_timestamp(timestamp_us: i64) -> String {
    chrono::DateTime::from_timestamp_micros(timestamp_us)
        .expect("database timestamps are valid UTC microseconds")
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn boundary_error(
    error: FsBoundaryError,
    root_id: Option<&RootId>,
    relative_path: Option<&RelativePath>,
) -> AppError {
    let code = match error {
        FsBoundaryError::RootNotFound => ErrorCode::RootNotFound,
        FsBoundaryError::Unavailable | FsBoundaryError::RootChanged => ErrorCode::RootUnavailable,
        FsBoundaryError::PathInvalid => ErrorCode::PathInvalid,
        FsBoundaryError::PathEscape => ErrorCode::PathEscape,
        FsBoundaryError::SymlinkForbidden => ErrorCode::PathSymlinkForbidden,
        FsBoundaryError::Internal => ErrorCode::Internal,
    };
    let mut app_error = AppError::new(code, error.to_string());
    if let Some(root_id) = root_id {
        app_error = app_error.with_detail("root_id", root_id.as_str());
    }
    if let Some(relative_path) = relative_path {
        app_error = app_error.with_detail("relative_path", relative_path.as_str());
    }
    app_error
}
