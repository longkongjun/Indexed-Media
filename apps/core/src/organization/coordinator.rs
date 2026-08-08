use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use crate::catalog::CatalogLocalResultPort;
use crate::catalog::model::{
    LocalStatus, MediaItemKind, MediaNodeKind, MetadataFieldState, MetadataSource,
    MetadataSourceType, NfoStatus, VerifiedFileAsset, VerifiedLocalResult, VerifiedMediaNode,
    VerifiedMediaTree, VerifiedMediaVersion, VerifiedMetadataValue,
};
use crate::discovery::model::{RootAccess, RootId};
use crate::discovery::organization_projection::OrganizationInboxProjection;
use crate::identification::organization_port::{
    ConfirmedOrganizationIdentity, OrganizationIdentityPort,
};
use crate::platform::task_runtime::TaskClock;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::processing::model::{ProcessingLease, ProcessingStage};
use crate::tasks::processing::store::ProcessingStore;
use crate::tasks::processing::worker::{
    ProcessingHandlerOutcome, ProcessingStageHandler, ProcessingStopToken,
};

use super::executor::{ExecutionOutcome, OrganizationExecutor};
use super::fs::{FileLocator, OrganizationFs, ProcessingStopToken as FsStopToken};
use super::journal_store::{JournalKind, JournalStatus, JournalStore, LocalNfoStatus};
use super::model::{ConfirmedNfoMetadata, OrganizationTarget, OrganizationTargetKind};
use super::plan_service::{OrganizationPlanService, OrganizationPlanningPort};
use super::plan_store::OrganizationPlanStore;
use super::planner::{
    OrganizationLocation, OrganizationPlanner, PlanAuthorization, PlanningIdentity, PlanningInput,
};
use super::target_store::OrganizationTargetStore;

const ORGANIZATION_RETRY_DELAY_US: i64 = 30_000_000;

#[derive(Clone)]
/// 从窄领域端口聚合 planner 当前事实的生产适配器。
pub struct SqliteOrganizationPlanningPort {
    sources: OrganizationInboxProjection,
    identities: Arc<dyn OrganizationIdentityPort>,
    targets: OrganizationTargetStore,
    fs: Arc<dyn OrganizationFs>,
    root_access: BTreeMap<RootId, RootAccess>,
}

impl SqliteOrganizationPlanningPort {
    /// 以数据库只读端口、能力文件系统和无宿主路径根策略创建适配器。
    #[must_use]
    pub fn new(
        pool: sqlx::SqlitePool,
        identities: Arc<dyn OrganizationIdentityPort>,
        fs: Arc<dyn OrganizationFs>,
        root_access: BTreeMap<RootId, RootAccess>,
    ) -> Self {
        Self {
            sources: OrganizationInboxProjection::new(pool.clone()),
            identities,
            targets: OrganizationTargetStore::new(pool),
            fs,
            root_access,
        }
    }
}

#[async_trait]
impl OrganizationPlanningPort for SqliteOrganizationPlanningPort {
    async fn load(&self, account_id: Uuid, task_id: Uuid) -> Result<PlanningInput, AppError> {
        let source = self.sources.source_for_task(account_id, task_id).await?;
        let confirmed = self
            .identities
            .confirmed_identity(account_id, task_id)
            .await?;
        let (kind, identity, selected_identity_id, nfo_metadata) = planning_identity(confirmed);
        let targets = self.targets.list(account_id).await?;
        let target = select_target(&targets, kind, source.inbox_directory_id)?;
        let source_locator = FileLocator::new(source.root_id.clone(), source.relative_path.clone());
        let observed = self
            .fs
            .observe(&source_locator)
            .map_err(filesystem)?
            .ok_or_else(|| {
                AppError::new(ErrorCode::ResourceConflict, "organization source changed")
            })?;
        let target_snapshot = self
            .fs
            .preflight_target(&target.root_id, &target.relative_path)
            .map_err(filesystem)?;
        let source_unchanged = observed
            .identity()
            .matches_durable_snapshot(&source.identity_snapshot)
            && observed.size_bytes() == source.size_bytes
            && observed.modified_at_ns() == i128::from(source.modified_at_ns);
        let source_writable = self.root_access.get(&source.root_id) == Some(&RootAccess::ReadWrite);
        let same_filesystem =
            observed.identity().device == target_snapshot.directory_identity.device;
        let base = PlanningInput {
            task_id,
            file_revision_id: source.file_revision_id,
            selected_identity_id,
            source: OrganizationLocation {
                root_id: source.root_id,
                relative_path: source.relative_path,
            },
            source_inbox_id: source.inbox_directory_id,
            source_writable,
            source_unchanged,
            destination_exists: false,
            same_filesystem,
            explicit_tags: BTreeSet::new(),
            one_time_authorized: false,
            identity,
            nfo_metadata,
            target,
        };
        let preview = OrganizationPlanner::plan(&base).map_err(planning)?;
        let destination = FileLocator::new(
            preview.destination.root_id,
            preview.destination.relative_path,
        );
        let mut current = base;
        current.destination_exists = self.fs.inspect(&destination).map_err(filesystem)?.is_some();
        Ok(current)
    }
}

/// 把已确认身份和持久计划连接到 `ProcessingTask` 的 planning 阶段。
pub struct OrganizationPlanningHandler {
    account_id: Option<Uuid>,
    service: OrganizationPlanService,
    tasks: ProcessingStore,
    clock: Arc<dyn TaskClock>,
}

impl OrganizationPlanningHandler {
    /// 创建 planning handler。
    #[must_use]
    pub fn new(
        account_id: Uuid,
        service: OrganizationPlanService,
        tasks: ProcessingStore,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            account_id: Some(account_id),
            service,
            tasks,
            clock,
        }
    }

    /// 创建在实际任务领取后解析账号的生产 handler，使首次启动无需预存管理员。
    #[must_use]
    pub fn new_dynamic(
        service: OrganizationPlanService,
        tasks: ProcessingStore,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            account_id: None,
            service,
            tasks,
            clock,
        }
    }
}

#[async_trait]
impl ProcessingStageHandler for OrganizationPlanningHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Planning
    }

    async fn run(
        &self,
        lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        let account_id = match self.account_id {
            Some(account_id) => account_id,
            None => self.tasks.account_id(lease.task.id).await?,
        };
        match self.service.ensure_current(account_id, lease.task.id).await {
            Ok(plan) => {
                self.tasks
                    .finish_organization_plan(
                        lease,
                        plan.id,
                        plan.authorization != PlanAuthorization::Paused,
                        self.clock.now_us(),
                    )
                    .await?;
            }
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::ValidationFailed
                        | ErrorCode::PathInvalid
                        | ErrorCode::ResourceConflict
                ) =>
            {
                self.tasks
                    .finish_organization_planning_failed(lease, self.clock.now_us())
                    .await?;
            }
            Err(error) => return Err(error),
        }
        Ok(ProcessingHandlerOutcome::Committed)
    }
}

/// 把 journal-first executor 连接到 `ProcessingTask` 的 file-operation 阶段。
pub struct OrganizationFileOperationHandler {
    executor: OrganizationExecutor,
    tasks: ProcessingStore,
    clock: Arc<dyn TaskClock>,
}

impl OrganizationFileOperationHandler {
    /// 创建 file-operation handler。
    #[must_use]
    pub fn new(
        executor: OrganizationExecutor,
        tasks: ProcessingStore,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            executor,
            tasks,
            clock,
        }
    }
}

#[async_trait]
impl ProcessingStageHandler for OrganizationFileOperationHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::FileOperation
    }

    async fn run(
        &self,
        lease: &ProcessingLease,
        stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        if stop.is_stopped() {
            return Ok(ProcessingHandlerOutcome::Paused {
                reason: crate::tasks::processing::model::ProcessingReason::OrganizationIoTemporary,
                next_retry_at_us: self.clock.now_us() + ORGANIZATION_RETRY_DELAY_US,
            });
        }
        let account_id = self.tasks.account_id(lease.task.id).await?;
        let fs_stop = FsStopToken::default();
        let outcome = self
            .executor
            .for_account(account_id)
            .run_file_stage(lease, fs_stop)
            .await?;
        match outcome {
            ExecutionOutcome::Completed(result) => {
                let nfo_pending = result
                    .remaining_actions
                    .iter()
                    .any(|action| action == "nfo");
                self.tasks
                    .finish_organization_file(lease, result.id, nfo_pending, self.clock.now_us())
                    .await?;
            }
            ExecutionOutcome::RecoveryPending { .. } | ExecutionOutcome::Stopped => {
                let now = self.clock.now_us();
                self.tasks
                    .finish_organization_io_pending(lease, now + ORGANIZATION_RETRY_DELAY_US, now)
                    .await?;
            }
            ExecutionOutcome::ManualReview(result) => {
                self.tasks
                    .finish_organization_manual_review(
                        lease,
                        result.map(|value| value.id),
                        self.clock.now_us(),
                    )
                    .await?;
            }
            ExecutionOutcome::Paused { .. } | ExecutionOutcome::Compensated(_) => {
                self.tasks
                    .finish_organization_manual_review(lease, None, self.clock.now_us())
                    .await?;
            }
        }
        Ok(ProcessingHandlerOutcome::Committed)
    }
}

/// 只推进 NFO journal，确保重试不会重复媒体文件 operation。
pub struct OrganizationNfoHandler {
    executor: OrganizationExecutor,
    tasks: ProcessingStore,
    clock: Arc<dyn TaskClock>,
}

impl OrganizationNfoHandler {
    /// 创建 NFO handler。
    #[must_use]
    pub fn new(
        executor: OrganizationExecutor,
        tasks: ProcessingStore,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            executor,
            tasks,
            clock,
        }
    }
}

#[async_trait]
impl ProcessingStageHandler for OrganizationNfoHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Nfo
    }

    async fn run(
        &self,
        lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        let account_id = self.tasks.account_id(lease.task.id).await?;
        match self
            .executor
            .for_account(account_id)
            .run_nfo_stage(lease)
            .await?
        {
            ExecutionOutcome::Completed(result) => {
                self.tasks
                    .finish_organization_nfo(lease, result.id, true, self.clock.now_us())
                    .await?;
            }
            ExecutionOutcome::RecoveryPending { .. }
            | ExecutionOutcome::ManualReview(_)
            | ExecutionOutcome::Paused { .. }
            | ExecutionOutcome::Stopped => {
                let result = lease.task.organization_result_id.ok_or_else(|| {
                    AppError::new(ErrorCode::Internal, "organization result reference missing")
                })?;
                self.tasks
                    .finish_organization_nfo(lease, result, false, self.clock.now_us())
                    .await?;
            }
            ExecutionOutcome::Compensated(_) => {
                return Err(AppError::new(
                    ErrorCode::TaskInvalidState,
                    "compensated result cannot enter NFO stage",
                ));
            }
        }
        Ok(ProcessingHandlerOutcome::Committed)
    }
}

/// 将完整 `LocalResult` 幂等映射到 Catalog 并完成 `ProcessingTask`。
pub struct OrganizationCompletionHandler {
    account_id: Option<Uuid>,
    plans: OrganizationPlanStore,
    journals: JournalStore,
    identities: Arc<dyn OrganizationIdentityPort>,
    catalog: Arc<dyn CatalogLocalResultPort>,
    tasks: ProcessingStore,
    clock: Arc<dyn TaskClock>,
}

impl OrganizationCompletionHandler {
    /// 创建 completion handler。
    #[must_use]
    pub fn new(
        account_id: Uuid,
        pool: sqlx::SqlitePool,
        identities: Arc<dyn OrganizationIdentityPort>,
        catalog: Arc<dyn CatalogLocalResultPort>,
        tasks: ProcessingStore,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            account_id: Some(account_id),
            plans: OrganizationPlanStore::new(pool.clone()),
            journals: JournalStore::new(pool),
            identities,
            catalog,
            tasks,
            clock,
        }
    }

    /// 创建按任务动态解析账号并复用共享 outbox journal store 的生产 handler。
    #[must_use]
    pub fn new_dynamic(
        pool: sqlx::SqlitePool,
        journals: JournalStore,
        identities: Arc<dyn OrganizationIdentityPort>,
        catalog: Arc<dyn CatalogLocalResultPort>,
        tasks: ProcessingStore,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            account_id: None,
            plans: OrganizationPlanStore::new(pool),
            journals,
            identities,
            catalog,
            tasks,
            clock,
        }
    }
}

#[async_trait]
impl ProcessingStageHandler for OrganizationCompletionHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Completion
    }

    async fn run(
        &self,
        lease: &ProcessingLease,
        _stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        let account_id = match self.account_id {
            Some(account_id) => account_id,
            None => self.tasks.account_id(lease.task.id).await?,
        };
        let plan = self
            .plans
            .current(account_id, lease.task.id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "organization plan not found"))?;
        let result = self
            .journals
            .result_for_task(account_id, lease.task.id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "organization result not found"))?;
        let identity = self
            .identities
            .confirmed_identity(account_id, lease.task.id)
            .await?;
        let journals = self.journals.for_task(account_id, lease.task.id).await?;
        let verified = verified_local_result(lease, &plan, &result, &journals, &identity)?;
        let now = self.clock.now_us();
        let Ok(media_item_id) = self
            .catalog
            .apply_local_result(account_id, &verified, now)
            .await
        else {
            self.tasks
                .finish_organization_catalog_pending(
                    lease,
                    result.id,
                    now + ORGANIZATION_RETRY_DELAY_US,
                    now,
                )
                .await?;
            return Ok(ProcessingHandlerOutcome::Committed);
        };
        self.journals
            .mark_catalog_committed(account_id, result.id, result.version, media_item_id, now)
            .await?;
        self.tasks
            .finish_organization_completed(lease, result.id, media_item_id, now)
            .await?;
        Ok(ProcessingHandlerOutcome::Committed)
    }
}

fn planning_identity(
    identity: ConfirmedOrganizationIdentity,
) -> (
    OrganizationTargetKind,
    PlanningIdentity,
    Option<Uuid>,
    ConfirmedNfoMetadata,
) {
    match identity {
        ConfirmedOrganizationIdentity::Movie {
            title,
            year,
            version_label,
            candidate_id,
            nfo_metadata,
        } => (
            OrganizationTargetKind::Movie,
            PlanningIdentity::Movie {
                title,
                year,
                version_label,
            },
            Some(candidate_id),
            nfo_metadata,
        ),
        ConfirmedOrganizationIdentity::SeriesEpisode {
            series_title,
            season,
            episodes,
            version_label,
            candidate_id,
            nfo_metadata,
        } => (
            OrganizationTargetKind::Series,
            PlanningIdentity::SeriesEpisode {
                series_title,
                season,
                episodes,
                version_label,
            },
            Some(candidate_id),
            nfo_metadata,
        ),
        ConfirmedOrganizationIdentity::GenericVideo {
            title,
            group_hint,
            decision_id,
        } => (
            OrganizationTargetKind::GenericVideo,
            PlanningIdentity::GenericVideo {
                title,
                group: group_hint,
                sequence: None,
            },
            Some(decision_id),
            ConfirmedNfoMetadata::default(),
        ),
    }
}

fn select_target(
    targets: &[OrganizationTarget],
    kind: OrganizationTargetKind,
    inbox_id: Uuid,
) -> Result<OrganizationTarget, AppError> {
    let compatible = targets
        .iter()
        .filter(|target| target.kind == kind)
        .collect::<Vec<_>>();
    if compatible.len() == 1 {
        return Ok(compatible[0].clone());
    }
    let matched = compatible
        .into_iter()
        .filter(|target| {
            target.enabled
                && target.rules.iter().any(|rule| {
                    rule.enabled
                        && rule.media_kind == kind
                        && rule.inbox_directory_id.is_none_or(|id| id == inbox_id)
                        && rule.explicit_tag.is_none()
                })
        })
        .collect::<Vec<_>>();
    if matched.len() == 1 {
        Ok(matched[0].clone())
    } else {
        Err(AppError::new(
            ErrorCode::ValidationFailed,
            "organization target selection is ambiguous",
        ))
    }
}

fn verified_local_result(
    lease: &ProcessingLease,
    plan: &super::plan_store::OrganizationPlanRecord,
    result: &super::journal_store::LocalResultView,
    journals: &[super::journal_store::JournalRecord],
    identity: &ConfirmedOrganizationIdentity,
) -> Result<VerifiedLocalResult, AppError> {
    let file = journals
        .iter()
        .find(|journal| {
            journal.plan_id == plan.id
                && journal.status == JournalStatus::Verified
                && matches!(
                    journal.kind,
                    JournalKind::Copy | JournalKind::Move | JournalKind::Hardlink
                )
        })
        .ok_or_else(|| AppError::new(ErrorCode::ResourceConflict, "verified journal missing"))?;
    let applied = file
        .applied
        .as_ref()
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "verified file facts missing"))?;
    let file_id = stable_uuid(b"organization.catalog.file\0", &[result.id.as_bytes()]);
    let media_id = stable_uuid(
        b"organization.catalog.media\0",
        &[
            plan.draft.target.id.as_bytes(),
            identity_id(identity).as_bytes(),
        ],
    );
    let local_status = if result.nfo_status == LocalNfoStatus::Failed {
        LocalStatus::Partial
    } else {
        LocalStatus::Complete
    };
    let nfo_status = match result.nfo_status {
        LocalNfoStatus::NotRequested => NfoStatus::NotRequested,
        LocalNfoStatus::Preserved | LocalNfoStatus::Generated => NfoStatus::Complete,
        LocalNfoStatus::Failed => NfoStatus::Failed,
    };
    let (kind, title, year, nodes, versions) = catalog_tree(identity, media_id, file_id);
    Ok(VerifiedLocalResult {
        result_id: result.id,
        task_id: lease.task.id,
        library_id: plan.draft.target.id,
        media: VerifiedMediaTree {
            id: media_id,
            kind,
            title: title.clone(),
            year,
            local_status,
            metadata: vec![VerifiedMetadataValue {
                field: "title".to_owned(),
                value: Some(title),
                state: MetadataFieldState::Present,
                source: Some(MetadataSource {
                    kind: if kind == MediaItemKind::GenericVideo {
                        MetadataSourceType::Manual
                    } else {
                        MetadataSourceType::Tmdb
                    },
                    id: Some(identity_id(identity).to_string()),
                    version: Some("1".to_owned()),
                }),
            }],
            artwork_refs: vec![],
            nodes,
            versions,
        },
        file_assets: vec![VerifiedFileAsset {
            id: file_id,
            file_revision_id: lease.task.file_revision_id,
            source_relative_path: plan.draft.source.relative_path.as_str().to_owned(),
            current_relative_path: plan.draft.destination.relative_path.as_str().to_owned(),
            size_bytes: applied.size_bytes(),
        }],
        nfo_status,
    })
}

fn catalog_tree(
    identity: &ConfirmedOrganizationIdentity,
    media_id: Uuid,
    file_id: Uuid,
) -> (
    MediaItemKind,
    String,
    Option<u16>,
    Vec<VerifiedMediaNode>,
    Vec<VerifiedMediaVersion>,
) {
    match identity {
        ConfirmedOrganizationIdentity::Movie {
            title,
            year,
            version_label,
            ..
        } => (
            MediaItemKind::Movie,
            title.clone(),
            *year,
            vec![],
            vec![VerifiedMediaVersion {
                id: stable_uuid(b"organization.catalog.version\0", &[media_id.as_bytes()]),
                owner_node_id: None,
                label: version_label.clone(),
                file_asset_ids: vec![file_id],
            }],
        ),
        ConfirmedOrganizationIdentity::SeriesEpisode {
            series_title,
            season,
            episodes,
            version_label,
            nfo_metadata,
            ..
        } => {
            let season_id = stable_uuid(
                b"organization.catalog.season\0",
                &[media_id.as_bytes(), &season.to_be_bytes()],
            );
            let mut nodes = vec![VerifiedMediaNode {
                id: season_id,
                parent_id: None,
                kind: MediaNodeKind::Season,
                title: format!("Season {season}"),
                ordinal: u32::from(*season),
            }];
            let mut versions = Vec::with_capacity(episodes.len());
            for episode in episodes {
                let episode_id = stable_uuid(
                    b"organization.catalog.episode\0",
                    &[season_id.as_bytes(), &episode.to_be_bytes()],
                );
                nodes.push(VerifiedMediaNode {
                    id: episode_id,
                    parent_id: Some(season_id),
                    kind: MediaNodeKind::Episode,
                    title: format!("Episode {episode}"),
                    ordinal: u32::from(*episode),
                });
                versions.push(VerifiedMediaVersion {
                    id: stable_uuid(
                        b"organization.catalog.version\0",
                        &[episode_id.as_bytes(), file_id.as_bytes()],
                    ),
                    owner_node_id: Some(episode_id),
                    label: version_label.clone(),
                    file_asset_ids: vec![file_id],
                });
            }
            (
                MediaItemKind::Series,
                series_title.clone(),
                nfo_metadata.year,
                nodes,
                versions,
            )
        }
        ConfirmedOrganizationIdentity::GenericVideo {
            title, group_hint, ..
        } => {
            let node_id = stable_uuid(
                b"organization.catalog.generic\0",
                &[media_id.as_bytes(), file_id.as_bytes()],
            );
            (
                MediaItemKind::GenericVideo,
                group_hint.clone().unwrap_or_else(|| title.clone()),
                None,
                vec![VerifiedMediaNode {
                    id: node_id,
                    parent_id: None,
                    kind: MediaNodeKind::GenericVideoItem,
                    title: title.clone(),
                    ordinal: 1,
                }],
                vec![VerifiedMediaVersion {
                    id: stable_uuid(
                        b"organization.catalog.version\0",
                        &[node_id.as_bytes(), file_id.as_bytes()],
                    ),
                    owner_node_id: Some(node_id),
                    label: None,
                    file_asset_ids: vec![file_id],
                }],
            )
        }
    }
}

fn identity_id(identity: &ConfirmedOrganizationIdentity) -> Uuid {
    match identity {
        ConfirmedOrganizationIdentity::Movie { candidate_id, .. }
        | ConfirmedOrganizationIdentity::SeriesEpisode { candidate_id, .. } => *candidate_id,
        ConfirmedOrganizationIdentity::GenericVideo { decision_id, .. } => *decision_id,
    }
}

fn stable_uuid(domain: &[u8], parts: &[&[u8]]) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn planning(error: super::planner::PlanningDecision) -> AppError {
    AppError::new(
        ErrorCode::ValidationFailed,
        format!("organization planning paused: {error:?}"),
    )
}

fn filesystem(error: crate::discovery::model::FsBoundaryError) -> AppError {
    AppError::with_source(ErrorCode::ResourceConflict, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_tree_maps_series_and_generic_confirmed_identities_deterministically() {
        let candidate_id = Uuid::from_u128(1);
        let media_id = Uuid::from_u128(2);
        let file_id = Uuid::from_u128(3);
        let series = ConfirmedOrganizationIdentity::SeriesEpisode {
            series_title: "Example".to_owned(),
            season: 1,
            episodes: vec![1, 2],
            version_label: Some("WEB-DL".to_owned()),
            candidate_id,
            nfo_metadata: ConfirmedNfoMetadata::default(),
        };
        let (kind, title, _, nodes, versions) = catalog_tree(&series, media_id, file_id);
        assert_eq!(kind, MediaItemKind::Series);
        assert_eq!(title, "Example");
        assert_eq!(nodes.len(), 3);
        assert_eq!(versions.len(), 2);
        assert!(
            versions
                .iter()
                .all(|version| version.file_asset_ids == [file_id])
        );

        let generic = ConfirmedOrganizationIdentity::GenericVideo {
            title: "Lesson 1".to_owned(),
            group_hint: Some("Course".to_owned()),
            decision_id: Uuid::from_u128(4),
        };
        let first = catalog_tree(&generic, media_id, file_id);
        let second = catalog_tree(&generic, media_id, file_id);
        assert_eq!(first, second);
        assert_eq!(first.0, MediaItemKind::GenericVideo);
        assert_eq!(first.1, "Course");
        assert_eq!(first.3[0].title, "Lesson 1");
        assert_eq!(first.4[0].file_asset_ids, [file_id]);
    }
}
