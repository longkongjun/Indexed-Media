use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use uuid::Uuid;

use crate::discovery::capability::{CapabilityFs, DeploymentRootsFingerprint};
use crate::discovery::model::{
    DeploymentRootView, FsBoundaryError, RelativePath, RootAccess, RootId,
};
use crate::discovery::organization_projection::OrganizationInboxProjection;
use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

use super::model::{
    OrganizationNamingPattern, OrganizationNfoPolicy, OrganizationOperation, OrganizationRuleInput,
    OrganizationTarget, OrganizationTargetCommand, OrganizationTargetKind,
    OrganizationTargetPreflightCommand,
};
use super::target_store::{OrganizationTargetBoundary, OrganizationTargetStore};

/// 目标预检无法通过时返回的公开稳定分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum OrganizationTargetPreflightFailure {
    /// 目标根只允许读取。
    #[serde(rename = "organization.root-read-only")]
    RootReadOnly,
    /// 候选目标与收件箱或其他目标重叠。
    #[serde(rename = "organization.target-overlap")]
    TargetOverlap,
    /// 候选目录当前无法通过能力重新验证。
    #[serde(rename = "organization.target-unavailable")]
    TargetUnavailable,
}

/// 无副作用目标能力与 overlap 预检投影。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrganizationTargetPreflight {
    /// 部署根逻辑 ID。
    pub root_id: RootId,
    /// 规范化根内相对路径。
    pub relative_path: RelativePath,
    /// 配置策略和当前能力检查是否允许写目标。
    pub writable: bool,
    /// 是否与收件箱或其他目标重叠。
    pub overlaps_existing: bool,
    /// 当前命令没有来源位置，因此不推测文件系统关系。
    pub same_filesystem_hint: Option<bool>,
    /// 不通过时的稳定公开分类。
    pub failure_code: Option<OrganizationTargetPreflightFailure>,
}

/// 与 `OpenAPI` 一致、将内部微秒时间戳转换为 UTC 字符串的安全目标投影。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrganizationTargetView {
    /// 目标稳定 ID。
    pub id: Uuid,
    /// 目标媒体类型。
    pub kind: OrganizationTargetKind,
    /// 管理员可读名称。
    pub display_name: String,
    /// 部署根逻辑 ID。
    pub root_id: RootId,
    /// 根能力内相对目录。
    pub relative_path: RelativePath,
    /// 固定文件操作。
    pub operation: OrganizationOperation,
    /// 固定命名模式。
    pub naming_pattern: OrganizationNamingPattern,
    /// 缺失 NFO 策略。
    pub nfo_policy: OrganizationNfoPolicy,
    /// 是否允许低风险自动执行。
    pub automatic: bool,
    /// 是否允许新计划使用目标。
    pub enabled: bool,
    /// 有界结构化规则。
    pub rules: Vec<OrganizationRuleInput>,
    /// 聚合配置版本。
    pub config_version: i64,
    /// 最近提交的 UTC 时间。
    pub updated_at: String,
}

#[derive(Clone)]
/// 协调部署根、能力预检、discovery overlap 与目标聚合 store。
pub struct OrganizationTargetService {
    roots: Arc<BTreeMap<RootId, DeploymentRootView>>,
    fs: Arc<dyn CapabilityFs>,
    store: OrganizationTargetStore,
    inboxes: OrganizationInboxProjection,
    config_guard: Option<Arc<DeploymentRootsGuard>>,
}

struct DeploymentRootsGuard {
    path: PathBuf,
    fingerprint: DeploymentRootsFingerprint,
}

impl OrganizationTargetService {
    /// 以同一份已验证部署根和能力适配器创建服务。
    #[must_use]
    pub fn new(
        roots: BTreeMap<RootId, DeploymentRootView>,
        fs: Arc<dyn CapabilityFs>,
        pool: sqlx::SqlitePool,
    ) -> Self {
        Self::new_with_notifier(roots, fs, pool, OutboxNotifier::new())
    }

    /// 使用共享 outbox 通知器创建目标服务。
    #[must_use]
    pub fn new_with_notifier(
        roots: BTreeMap<RootId, DeploymentRootView>,
        fs: Arc<dyn CapabilityFs>,
        pool: sqlx::SqlitePool,
        notifier: OutboxNotifier,
    ) -> Self {
        Self {
            roots: Arc::new(roots),
            fs,
            store: OrganizationTargetStore::new_with_notifier(pool.clone(), notifier),
            inboxes: OrganizationInboxProjection::new(pool),
            config_guard: None,
        }
    }

    /// 绑定已加载 deployment-roots 文档的精确指纹。
    #[must_use]
    pub fn with_config_guard(
        mut self,
        path: PathBuf,
        fingerprint: DeploymentRootsFingerprint,
    ) -> Self {
        self.config_guard = Some(Arc::new(DeploymentRootsGuard { path, fingerprint }));
        self
    }

    /// 返回部署根配置是否仍与服务创建时的快照相同。
    #[must_use]
    pub fn configuration_is_current(&self) -> bool {
        self.config_guard
            .as_ref()
            .is_none_or(|guard| guard.fingerprint.matches_path(&guard.path))
    }

    /// 无持久化、无目录创建地检查目标策略、overlap 和当前目录能力。
    ///
    /// # Errors
    ///
    /// 根或路径语法无效、根不存在、安全路径检查失败或数据库不可用时返回稳定应用错误。
    pub async fn preflight(
        &self,
        account_id: Uuid,
        command: OrganizationTargetPreflightCommand,
    ) -> Result<OrganizationTargetPreflight, AppError> {
        let (root_id, relative_path) = parse_location(&command.root_id, &command.relative_path)?;
        let root = self.root(&root_id)?;
        if root.access != RootAccess::ReadWrite {
            return Ok(preflight_failure(
                root_id,
                relative_path,
                false,
                false,
                OrganizationTargetPreflightFailure::RootReadOnly,
            ));
        }
        let overlaps_existing = self.inboxes.has_overlap(&root_id, &relative_path).await?
            || self
                .store
                .has_overlap(account_id, &root_id, &relative_path, None)
                .await?;
        if overlaps_existing {
            return Ok(preflight_failure(
                root_id,
                relative_path,
                true,
                true,
                OrganizationTargetPreflightFailure::TargetOverlap,
            ));
        }
        match self.fs.preflight_directory(&root_id, &relative_path) {
            Ok(_) => Ok(OrganizationTargetPreflight {
                root_id,
                relative_path,
                writable: true,
                overlaps_existing: false,
                same_filesystem_hint: None,
                failure_code: None,
            }),
            Err(FsBoundaryError::Unavailable | FsBoundaryError::RootChanged) => {
                Ok(preflight_failure(
                    root_id,
                    relative_path,
                    false,
                    false,
                    OrganizationTargetPreflightFailure::TargetUnavailable,
                ))
            }
            Err(error) => Err(boundary_error(error)),
        }
    }

    /// 返回账户目标的稳定游标页。
    ///
    /// # Errors
    ///
    /// 游标、数据库或持久投影无效时返回稳定应用错误。
    pub async fn list(
        &self,
        account_id: Uuid,
        page: &PageRequest,
    ) -> Result<CursorPage<OrganizationTargetView>, AppError> {
        let page = self.store.list_page(account_id, page).await?;
        Ok(CursorPage {
            items: page
                .items
                .into_iter()
                .map(to_view)
                .collect::<Result<_, _>>()?,
            next_cursor: page.next_cursor,
        })
    }

    /// 读取账户拥有的单个目标。
    ///
    /// # Errors
    ///
    /// 目标不存在、数据库或持久投影无效时返回稳定应用错误。
    pub async fn get(
        &self,
        account_id: Uuid,
        target_id: Uuid,
    ) -> Result<OrganizationTargetView, AppError> {
        self.store
            .get(account_id, target_id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "organization target not found"))
            .and_then(to_view)
    }

    /// 校验能力与 overlap 后创建目标聚合。
    ///
    /// # Errors
    ///
    /// 命令、能力、overlap、数据库或账户约束失败时返回稳定应用错误。
    pub async fn create(
        &self,
        account_id: Uuid,
        command: OrganizationTargetCommand,
    ) -> Result<OrganizationTargetView, AppError> {
        let input = command.into_input()?;
        let boundary = self.boundary(account_id, &input, None).await?;
        to_view(
            self.store
                .insert(account_id, Uuid::now_v7(), input, &boundary, now_us())
                .await?,
        )
    }

    /// 以当前版本校验能力和 overlap 后完整替换目标聚合。
    ///
    /// # Errors
    ///
    /// 命令、能力、overlap、数据库或乐观并发失败时返回稳定应用错误。
    pub async fn replace(
        &self,
        account_id: Uuid,
        target_id: Uuid,
        expected_version: i64,
        command: OrganizationTargetCommand,
    ) -> Result<OrganizationTargetView, AppError> {
        let input = command.into_input()?;
        let boundary = self.boundary(account_id, &input, Some(target_id)).await?;
        to_view(
            self.store
                .replace(
                    account_id,
                    target_id,
                    expected_version,
                    input,
                    &boundary,
                    now_us(),
                )
                .await?,
        )
    }

    /// 以当前版本删除未受后续计划引用的目标。
    ///
    /// # Errors
    ///
    /// 数据库、引用门禁或乐观并发失败时返回稳定应用错误。
    pub async fn delete(
        &self,
        account_id: Uuid,
        target_id: Uuid,
        expected_version: i64,
    ) -> Result<(), AppError> {
        self.store
            .delete(account_id, target_id, expected_version)
            .await
    }

    async fn boundary(
        &self,
        account_id: Uuid,
        input: &super::model::OrganizationTargetInput,
        excluded_target_id: Option<Uuid>,
    ) -> Result<OrganizationTargetBoundary, AppError> {
        let root = self.root(&input.root_id)?.clone();
        if root.access != RootAccess::ReadWrite {
            return Err(AppError::new(
                ErrorCode::OrganizationRootReadOnly,
                "organization target root is read-only",
            ));
        }
        let overlap = self
            .inboxes
            .has_overlap(&input.root_id, &input.relative_path)
            .await?
            || self
                .store
                .has_overlap(
                    account_id,
                    &input.root_id,
                    &input.relative_path,
                    excluded_target_id,
                )
                .await?;
        if overlap {
            return Err(AppError::new(
                ErrorCode::OrganizationTargetOverlap,
                "organization target overlaps existing configuration",
            ));
        }
        self.fs
            .preflight_directory(&input.root_id, &input.relative_path)
            .map_err(boundary_error)?;
        Ok(OrganizationTargetBoundary {
            root,
            overlaps_inbox: false,
        })
    }

    fn root(&self, root_id: &RootId) -> Result<&DeploymentRootView, AppError> {
        self.roots
            .get(root_id)
            .ok_or_else(|| AppError::new(ErrorCode::RootNotFound, "deployment root not found"))
    }
}

fn preflight_failure(
    root_id: RootId,
    relative_path: RelativePath,
    writable: bool,
    overlaps_existing: bool,
    failure_code: OrganizationTargetPreflightFailure,
) -> OrganizationTargetPreflight {
    OrganizationTargetPreflight {
        root_id,
        relative_path,
        writable,
        overlaps_existing,
        same_filesystem_hint: None,
        failure_code: Some(failure_code),
    }
}

fn parse_location(root_id: &str, relative_path: &str) -> Result<(RootId, RelativePath), AppError> {
    let root_id = RootId::parse(root_id)
        .map_err(|error| AppError::new(ErrorCode::PathInvalid, error.to_string()))?;
    let relative_path = RelativePath::parse(relative_path)
        .map_err(|error| AppError::new(ErrorCode::PathInvalid, error.to_string()))?;
    Ok((root_id, relative_path))
}

fn boundary_error(error: FsBoundaryError) -> AppError {
    let code = match error {
        FsBoundaryError::RootNotFound => ErrorCode::RootNotFound,
        FsBoundaryError::Unavailable | FsBoundaryError::RootChanged => {
            ErrorCode::OrganizationTargetUnavailable
        }
        FsBoundaryError::PathInvalid => ErrorCode::PathInvalid,
        FsBoundaryError::PathEscape => ErrorCode::PathEscape,
        FsBoundaryError::SymlinkForbidden => ErrorCode::PathSymlinkForbidden,
        FsBoundaryError::Internal => ErrorCode::Internal,
    };
    AppError::new(code, error.to_string())
}

fn to_view(target: OrganizationTarget) -> Result<OrganizationTargetView, AppError> {
    let updated_at = chrono::DateTime::from_timestamp_micros(target.updated_at_us)
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "invalid organization timestamp"))?
        .to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    Ok(OrganizationTargetView {
        id: target.id,
        kind: target.kind,
        display_name: target.display_name,
        root_id: target.root_id,
        relative_path: target.relative_path,
        operation: target.operation,
        naming_pattern: target.naming_pattern,
        nfo_policy: target.nfo_policy,
        automatic: target.automatic,
        enabled: target.enabled,
        rules: target.rules,
        config_version: target.config_version,
        updated_at,
    })
}

fn now_us() -> i64 {
    chrono::Utc::now().timestamp_micros()
}
