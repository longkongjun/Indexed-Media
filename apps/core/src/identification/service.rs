use std::collections::BTreeMap;
use std::ffi::OsString;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Datelike as _;
use sha2::{Digest as _, Sha256};
use sqlx::{Row as _, SqlitePool};

use crate::connectors::enhancer::model::{
    BaseIdentityHint, EnhancementHints, EnhancementInput, EnhancerMediaKind,
};
use crate::connectors::enhancer::port::EnhancerError;
use crate::connectors::enhancer::service::EnhancerService;
use crate::connectors::model::{
    CacheStatus, ExternalIdSource, MetadataCandidate, ProviderError, ProviderMediaKind,
    TmdbExternalIdRequest, TmdbSearchRequest,
};
use crate::connectors::service::{ConnectorService, MetadataProvider};
use crate::discovery::capability::CapabilityFs;
use crate::discovery::model::{RelativePath, RootId};
use crate::identification::decision::{
    DecisionCandidate, DecisionInput, DecisionRules, EnhancedField, EnhancementGuard, decide,
};
use crate::identification::evidence::{
    EvidenceDraft, EvidenceKind, EvidenceSource, EvidenceStrength,
};
use crate::identification::manual::model::{ManualIdentificationContext, ManualIdentityHint};
use crate::identification::manual::store::ManualDecisionStore;
use crate::identification::model::{
    CandidateIdentityGraph, ExternalIdHint, MediaKind, ParsedIdentityHint,
};
use crate::identification::nfo::{NfoKind, NfoLocator, NfoParser};
use crate::identification::parser::FilenameParser;
use crate::identification::store::{
    IdentificationCommit, IdentificationCommitOutcome, IdentificationStore,
};
use crate::platform::outbox::OutboxNotifier;
use crate::platform::secrets::SecretBytes;
use crate::platform::task_runtime::TaskClock;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::processing::model::{ProcessingLease, ProcessingStage};
use crate::tasks::processing::worker::{
    ProcessingHandlerOutcome, ProcessingStageHandler, ProcessingStopToken,
};

const PROVIDER_PIPELINE_VERSION: &str = "tmdb-v1";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 应用已验证增强提示后的冻结身份输入、辅助证据与确认保护。
pub struct AppliedEnhancement {
    pub hint: ParsedIdentityHint,
    pub evidence: Vec<EvidenceDraft>,
    pub guard: EnhancementGuard,
}

#[must_use]
/// 把严格增强提示映射为辅助证据；该函数不能表示或注入 provider ID。
pub fn apply_enhancement_hints(
    base: &ParsedIdentityHint,
    hints: &EnhancementHints,
    source_hash: [u8; 32],
) -> AppliedEnhancement {
    let mut hint = base.clone();
    let mut evidence = Vec::new();
    let mut guard = EnhancementGuard::default();

    if let Some(title) = &hints.title {
        let value = title.trim().to_owned();
        if value != base.normalized_title {
            guard.mark(EnhancedField::Title);
        }
        hint.normalized_title.clone_from(&value);
        evidence.push(enhancer_evidence(
            EvidenceKind::Title,
            value,
            "enhancer.hint.title",
            source_hash,
        ));
    }
    if let Some(year) = hints.year {
        if Some(year) != base.year {
            guard.mark(EnhancedField::Year);
        }
        hint.year = Some(year);
        evidence.push(enhancer_evidence(
            EvidenceKind::Year,
            year.to_string(),
            "enhancer.hint.year",
            source_hash,
        ));
    }
    if let Some(kind) = hints.media_kind {
        let kind = match kind {
            EnhancerMediaKind::Movie => MediaKind::Movie,
            EnhancerMediaKind::Episode => MediaKind::Episode,
        };
        if kind != base.media_kind {
            guard.mark(EnhancedField::MediaKind);
        }
        hint.media_kind = kind;
        evidence.push(enhancer_evidence(
            EvidenceKind::MediaType,
            match kind {
                MediaKind::Movie => "movie",
                MediaKind::Episode => "episode",
            }
            .to_owned(),
            "enhancer.hint.media-type",
            source_hash,
        ));
        match kind {
            MediaKind::Movie => {
                hint.season = None;
                hint.episodes.clear();
                hint.air_date = None;
            }
            MediaKind::Episode => {
                if hints.season != base.season || hints.episodes != base.episodes {
                    guard.mark(EnhancedField::Episode);
                }
                hint.season = hints.season;
                hint.episodes.clone_from(&hints.episodes);
                hint.air_date = None;
                if let Some(season) = hints.season {
                    for episode in &hints.episodes {
                        evidence.push(enhancer_evidence(
                            EvidenceKind::Episode,
                            format!("S{season:02}E{episode:02}"),
                            "enhancer.hint.episode",
                            source_hash,
                        ));
                    }
                }
            }
        }
    }

    AppliedEnhancement {
        hint,
        evidence,
        guard,
    }
}

fn enhancer_evidence(
    kind: EvidenceKind,
    normalized_value: String,
    reason: &str,
    source_hash: [u8; 32],
) -> EvidenceDraft {
    EvidenceDraft {
        source: EvidenceSource::Enhancer,
        source_version: "ollama-v1".to_owned(),
        kind,
        normalized_value,
        strength: EvidenceStrength::Supporting,
        reason: reason.to_owned(),
        source_hash,
    }
}

#[derive(Clone)]
/// 协调一次有界提供方查询，并把最终 revision 安全事务交给存储层。
pub struct IdentificationService {
    store: IdentificationStore,
    rules: DecisionRules,
}

impl IdentificationService {
    #[must_use]
    /// 使用当前决策规则和独立通知器创建识别服务。
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 使用共享通知器创建识别服务，使决策事务可唤醒 outbox/SSE 交付。
    pub fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self {
            store: IdentificationStore::new_with_notifier(pool, notifier),
            rules: DecisionRules::v1(),
        }
    }

    #[must_use]
    /// 借用底层识别存储，以便复用账户隔离详情读取。
    pub const fn store(&self) -> &IdentificationStore {
        &self.store
    }

    /// 对一个处理租约执行显式 ID 优先查询、有界搜索回退、集数核验、纯规则决策及提交时
    /// revision 复核。
    ///
    /// 提供方失败会转换为持久化 `blocked` 决策，而不是基础设施错误；Token 与原始提供方
    /// 响应不会进入证据或存储。
    ///
    /// # Errors
    ///
    /// 处理租约失效、输入校验失败或持久化事务失败时返回错误。
    #[allow(clippy::too_many_arguments)]
    pub async fn identify(
        &self,
        lease: &ProcessingLease,
        hint: &ParsedIdentityHint,
        local_evidence: &[EvidenceDraft],
        provider: &dyn MetadataProvider,
        token: &SecretBytes,
        locale: &str,
        region: Option<&str>,
        now_us: i64,
    ) -> Result<IdentificationCommitOutcome, AppError> {
        let attempt = self
            .store
            .begin_attempt(
                lease,
                hint.parser_version,
                PROVIDER_PIPELINE_VERSION,
                self.rules.version,
                now_us,
            )
            .await?;
        let provider_result = collect_candidates(provider, token, hint, locale, region).await;
        let (metadata, provider_error) = match provider_result {
            Ok(candidates) => (candidates, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.commit_result(
            lease,
            attempt.id,
            hint,
            local_evidence,
            &metadata,
            provider_error,
            EnhancementGuard::default(),
            now_us,
        )
        .await
    }

    /// 为 worker 执行提供方查询，并在 revision 安全决策事务前立即复查协作停止信号。
    ///
    /// # Errors
    ///
    /// worker 已停止时返回租约丢失；其他租约、校验与持久化失败也会原样返回。
    #[allow(clippy::too_many_arguments)]
    pub async fn identify_guarded(
        &self,
        lease: &ProcessingLease,
        hint: &ParsedIdentityHint,
        local_evidence: &[EvidenceDraft],
        enhancement_guard: EnhancementGuard,
        provider: &dyn MetadataProvider,
        token: &SecretBytes,
        locale: &str,
        region: Option<&str>,
        stop: &ProcessingStopToken,
        clock: &dyn TaskClock,
    ) -> Result<IdentificationCommitOutcome, AppError> {
        if stop.is_stopped() {
            return Err(stopped());
        }
        let attempt = self
            .store
            .begin_attempt(
                lease,
                hint.parser_version,
                PROVIDER_PIPELINE_VERSION,
                self.rules.version,
                clock.now_us(),
            )
            .await?;
        let provider_result = collect_candidates(provider, token, hint, locale, region).await;
        if stop.is_stopped() {
            return Err(stopped());
        }
        let (metadata, provider_error) = match provider_result {
            Ok(candidates) => (candidates, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        self.commit_result(
            lease,
            attempt.id,
            hint,
            local_evidence,
            &metadata,
            provider_error,
            enhancement_guard,
            clock.now_us(),
        )
        .await
    }

    /// 根据一个不可变人工选择器重新识别，并把所有提供方展示数据视为不可信。所选身份与精确
    /// 反馈会重新获取；修正线索仍经过常规有界候选与强规则流程。
    ///
    /// # Errors
    ///
    /// 返回停止/租约、校验或持久化失败；提供方失败会转换为可恢复的 blocked 决策。
    #[allow(clippy::too_many_arguments)]
    pub async fn identify_with_manual_context(
        &self,
        lease: &ProcessingLease,
        base_hint: &ParsedIdentityHint,
        local_evidence: &[EvidenceDraft],
        context: &ManualIdentificationContext,
        provider: &dyn MetadataProvider,
        token: &SecretBytes,
        locale: &str,
        region: Option<&str>,
        stop: &ProcessingStopToken,
        clock: &dyn TaskClock,
    ) -> Result<IdentificationCommitOutcome, AppError> {
        if stop.is_stopped() {
            return Err(stopped());
        }
        let (hint, mut manual_evidence, selected) = manual_request(base_hint, context)?;
        let attempt = self
            .store
            .begin_attempt(
                lease,
                hint.parser_version,
                PROVIDER_PIPELINE_VERSION,
                self.rules.version,
                clock.now_us(),
            )
            .await?;
        let provider_result = if selected {
            collect_selected_candidate(provider, token, &hint, context, locale).await
        } else {
            collect_candidates(provider, token, &hint, locale, region).await
        };
        if stop.is_stopped() {
            return Err(stopped());
        }
        let (metadata, provider_error) = match provider_result {
            Ok(candidates) => (candidates, None),
            Err(error) => (Vec::new(), Some(error)),
        };
        let candidates = decision_candidates(&metadata);
        let mut evidence = local_evidence.to_vec();
        evidence.append(&mut manual_evidence);
        append_provider_evidence(&mut evidence, &metadata);
        evidence.truncate(256);
        let decision = if selected {
            manual_selected_decision(
                lease.task.file_revision_id,
                &hint,
                &candidates,
                provider_error,
                matches!(context, ManualIdentificationContext::ExactFeedback { .. }),
                self.rules,
            )
        } else {
            let mut input = DecisionInput::new(lease.task.file_revision_id, &hint, &candidates);
            if let Some(error) = provider_error {
                input = input.with_provider_error(error);
            }
            decide(&input, &self.rules)
        };
        let mut outcome = self
            .store
            .commit(IdentificationCommit {
                lease,
                attempt_id: attempt.id,
                evidence: &evidence,
                candidates: &candidates,
                decision: &decision,
                title_hint: Some(&hint.normalized_title),
                now_us: clock.now_us(),
            })
            .await?;
        outcome.provider_failure = provider_error;
        Ok(outcome)
    }

    /// 无法加载已配置提供方凭据时持久化可恢复 blocked 决策。该路径与实时提供方失败共用
    /// 不可变尝试和 revision 安全提交，且不会虚构或持久化占位秘密。
    ///
    /// # Errors
    ///
    /// 处理租约、校验或持久化事务失败时返回错误。
    pub async fn identify_provider_failure(
        &self,
        lease: &ProcessingLease,
        hint: &ParsedIdentityHint,
        local_evidence: &[EvidenceDraft],
        provider_error: ProviderError,
        now_us: i64,
    ) -> Result<IdentificationCommitOutcome, AppError> {
        let attempt = self
            .store
            .begin_attempt(
                lease,
                hint.parser_version,
                PROVIDER_PIPELINE_VERSION,
                self.rules.version,
                now_us,
            )
            .await?;
        self.commit_result(
            lease,
            attempt.id,
            hint,
            local_evidence,
            &[],
            Some(provider_error),
            EnhancementGuard::default(),
            now_us,
        )
        .await
    }

    /// 人工重识别的提供方查询失败时，把人工选择器保存为有界证据；活动复核投影保持可恢复，
    /// 供后续重试。
    ///
    /// # Errors
    ///
    /// 校验、租约或持久化事务失败时返回错误。
    pub async fn identify_manual_provider_failure(
        &self,
        lease: &ProcessingLease,
        base_hint: &ParsedIdentityHint,
        local_evidence: &[EvidenceDraft],
        context: &ManualIdentificationContext,
        provider_error: ProviderError,
        now_us: i64,
    ) -> Result<IdentificationCommitOutcome, AppError> {
        let (hint, mut manual_evidence, selected) = manual_request(base_hint, context)?;
        let attempt = self
            .store
            .begin_attempt(
                lease,
                hint.parser_version,
                PROVIDER_PIPELINE_VERSION,
                self.rules.version,
                now_us,
            )
            .await?;
        let mut evidence = local_evidence.to_vec();
        evidence.append(&mut manual_evidence);
        evidence.truncate(256);
        let decision = if selected {
            manual_selected_decision(
                lease.task.file_revision_id,
                &hint,
                &[],
                Some(provider_error),
                matches!(context, ManualIdentificationContext::ExactFeedback { .. }),
                self.rules,
            )
        } else {
            decide(
                &DecisionInput::new(lease.task.file_revision_id, &hint, &[])
                    .with_provider_error(provider_error),
                &self.rules,
            )
        };
        let mut outcome = self
            .store
            .commit(IdentificationCommit {
                lease,
                attempt_id: attempt.id,
                evidence: &evidence,
                candidates: &[],
                decision: &decision,
                title_hint: Some(&hint.normalized_title),
                now_us,
            })
            .await?;
        outcome.provider_failure = Some(provider_error);
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    async fn commit_result(
        &self,
        lease: &ProcessingLease,
        attempt_id: uuid::Uuid,
        hint: &ParsedIdentityHint,
        local_evidence: &[EvidenceDraft],
        metadata: &[(MetadataCandidate, Vec<ExternalIdHint>)],
        provider_error: Option<ProviderError>,
        enhancement_guard: EnhancementGuard,
        now_us: i64,
    ) -> Result<IdentificationCommitOutcome, AppError> {
        let candidates = decision_candidates(metadata);
        let mut evidence = local_evidence.to_vec();
        append_provider_evidence(&mut evidence, metadata);
        evidence.truncate(256);
        let local_conflicts = local_evidence
            .iter()
            .filter(|item| {
                item.source == EvidenceSource::Nfo && item.strength == EvidenceStrength::Conflicting
            })
            .map(|_| crate::identification::decision::DecisionReason::NfoUnsafe)
            .collect::<Vec<_>>();
        let mut input = DecisionInput::new(lease.task.file_revision_id, hint, &candidates)
            .with_local_conflicts(local_conflicts)
            .with_enhancement_guard(enhancement_guard);
        if let Some(error) = provider_error {
            input = input.with_provider_error(error);
        }
        let decision = decide(&input, &self.rules);
        let mut outcome = self
            .store
            .commit(IdentificationCommit {
                lease,
                attempt_id,
                evidence: &evidence,
                candidates: &candidates,
                decision: &decision,
                title_hint: Some(&hint.normalized_title),
                now_us,
            })
            .await?;
        outcome.provider_failure = provider_error;
        Ok(outcome)
    }
}

fn manual_request(
    base_hint: &ParsedIdentityHint,
    context: &ManualIdentificationContext,
) -> Result<(ParsedIdentityHint, Vec<EvidenceDraft>, bool), AppError> {
    match context {
        ManualIdentificationContext::SelectedProvider {
            decision_id,
            media_kind,
            provider_id,
        } => {
            let mut hint = base_hint.clone();
            hint.media_kind = *media_kind;
            hint.external_ids.clear();
            if *media_kind == MediaKind::Movie {
                hint.season = None;
                hint.episodes.clear();
                hint.air_date = None;
            }
            Ok((
                hint,
                vec![manual_evidence(
                    EvidenceSource::ManualDecision,
                    *decision_id,
                    EvidenceKind::ExternalId,
                    format!("tmdb:{provider_id}"),
                    "manual.provider-selected",
                )],
                true,
            ))
        }
        ManualIdentificationContext::Rematch { decision_id, hint } => Ok((
            parsed_manual_hint(base_hint, hint)?,
            rematch_evidence(*decision_id, hint),
            false,
        )),
        ManualIdentificationContext::ExactFeedback {
            feedback_id,
            provider_id,
        } => Ok((
            base_hint.clone(),
            vec![manual_evidence(
                EvidenceSource::ManualFeedback,
                *feedback_id,
                EvidenceKind::ExternalId,
                format!("tmdb:{provider_id}"),
                "manual.feedback-exact",
            )],
            true,
        )),
    }
}

fn parsed_manual_hint(
    base: &ParsedIdentityHint,
    manual: &ManualIdentityHint,
) -> Result<ParsedIdentityHint, AppError> {
    if manual.normalized_title.is_empty()
        || manual.normalized_title.chars().count() > 200
        || manual.episodes.len() > 32
        || (manual.media_kind == MediaKind::Movie
            && (manual.season.is_some() || !manual.episodes.is_empty()))
        || (manual.media_kind == MediaKind::Episode
            && (manual.season.is_none() || manual.episodes.is_empty()))
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "manual identity hint is outside bounds",
        ));
    }
    Ok(ParsedIdentityHint {
        original: base.original.clone(),
        normalized_title: manual.normalized_title.clone(),
        media_kind: manual.media_kind,
        year: manual.year,
        season: manual.season,
        episodes: manual.episodes.clone(),
        air_date: None,
        external_ids: Vec::new(),
        version_tags: base.version_tags.clone(),
        parser_version: "manual-v1",
    })
}

fn rematch_evidence(decision_id: uuid::Uuid, hint: &ManualIdentityHint) -> Vec<EvidenceDraft> {
    let mut evidence = vec![manual_evidence(
        EvidenceSource::ManualDecision,
        decision_id,
        EvidenceKind::Title,
        hint.normalized_title.clone(),
        "manual.rematch-hint",
    )];
    if let Some(year) = hint.year {
        evidence.push(manual_evidence(
            EvidenceSource::ManualDecision,
            decision_id,
            EvidenceKind::Year,
            year.to_string(),
            "manual.rematch-hint",
        ));
    }
    if let Some(season) = hint.season {
        for episode in &hint.episodes {
            evidence.push(manual_evidence(
                EvidenceSource::ManualDecision,
                decision_id,
                EvidenceKind::Episode,
                format!("S{season:02}E{episode:02}"),
                "manual.rematch-hint",
            ));
        }
    }
    evidence
}

fn manual_evidence(
    source: EvidenceSource,
    source_id: uuid::Uuid,
    kind: EvidenceKind,
    normalized_value: String,
    reason: &str,
) -> EvidenceDraft {
    EvidenceDraft {
        source,
        source_version: source_id.to_string(),
        kind,
        source_hash: Sha256::digest(normalized_value.as_bytes()).into(),
        normalized_value,
        strength: EvidenceStrength::Strong,
        reason: reason.to_owned(),
    }
}

async fn collect_selected_candidate(
    provider: &dyn MetadataProvider,
    token: &SecretBytes,
    hint: &ParsedIdentityHint,
    context: &ManualIdentificationContext,
    locale: &str,
) -> Result<Vec<(MetadataCandidate, Vec<ExternalIdHint>)>, ProviderError> {
    let provider_id = match context {
        ManualIdentificationContext::SelectedProvider { provider_id, .. }
        | ManualIdentificationContext::ExactFeedback { provider_id, .. } => provider_id,
        ManualIdentificationContext::Rematch { .. } => return Ok(Vec::new()),
    };
    let provider_id = provider_id
        .parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or(ProviderError::InvalidResponse)?;
    let mut candidate = provider
        .details(token, provider_kind(hint.media_kind), provider_id, locale)
        .await?;
    if candidate.identity.provider_id != provider_id {
        return Err(ProviderError::InvalidResponse);
    }
    if hint.media_kind == MediaKind::Episode
        && candidate.identity.media_kind == ProviderMediaKind::Tv
    {
        let requested = hint
            .season
            .map(|season| {
                hint.episodes
                    .iter()
                    .map(|episode| (season, *episode))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        candidate.episodes = provider
            .verify_episodes(token, provider_id, &requested, locale)
            .await?;
    }
    Ok(vec![(
        candidate,
        vec![ExternalIdHint {
            provider: "tmdb".to_owned(),
            value: provider_id.to_string(),
            is_default: true,
        }],
    )])
}

fn manual_selected_decision(
    file_revision_id: uuid::Uuid,
    hint: &ParsedIdentityHint,
    candidates: &[DecisionCandidate],
    provider_error: Option<ProviderError>,
    feedback: bool,
    rules: DecisionRules,
) -> crate::identification::decision::IdentificationDecisionDraft {
    use crate::identification::decision::{
        DecisionLevel, DecisionReason, IdentificationDecisionDraft,
    };
    if let Some(error) = provider_error {
        return decide(
            &DecisionInput::new(file_revision_id, hint, &[]).with_provider_error(error),
            &rules,
        );
    }
    let Some(candidate) = candidates.first() else {
        return IdentificationDecisionDraft {
            level: DecisionLevel::Unidentified,
            selected_candidate: None,
            reasons: vec![DecisionReason::NoCandidate],
            retry_at_us: None,
            graph: None,
            rule_version: rules.version,
        };
    };
    let media_matches = matches!(
        (hint.media_kind, candidate.identity.media_kind),
        (MediaKind::Movie, ProviderMediaKind::Movie) | (MediaKind::Episode, ProviderMediaKind::Tv)
    );
    if !media_matches {
        return IdentificationDecisionDraft {
            level: DecisionLevel::Ambiguous,
            selected_candidate: None,
            reasons: vec![DecisionReason::MediaTypeConflict],
            retry_at_us: None,
            graph: None,
            rule_version: rules.version,
        };
    }
    if hint.media_kind == MediaKind::Episode {
        let complete = hint.season.is_some_and(|season| {
            !hint.episodes.is_empty()
                && hint.episodes.iter().all(|episode| {
                    candidate
                        .episodes
                        .iter()
                        .any(|item| item.season == season && item.episode == *episode)
                })
        });
        if !complete {
            return IdentificationDecisionDraft {
                level: DecisionLevel::Ambiguous,
                selected_candidate: None,
                reasons: vec![DecisionReason::EpisodeMissing],
                retry_at_us: None,
                graph: None,
                rule_version: rules.version,
            };
        }
    }
    let mut reasons = vec![if feedback {
        DecisionReason::ExactFeedbackMatched
    } else {
        DecisionReason::ManualSelectionVerified
    }];
    if !candidate
        .titles
        .iter()
        .chain(&candidate.aliases)
        .any(|title| title.eq_ignore_ascii_case(&hint.normalized_title))
    {
        reasons.push(DecisionReason::ManualTitleOverride);
    }
    IdentificationDecisionDraft {
        level: DecisionLevel::Confirmed,
        selected_candidate: Some(candidate.id),
        reasons,
        retry_at_us: None,
        graph: CandidateIdentityGraph::from_hint(file_revision_id, hint).ok(),
        rule_version: rules.version,
    }
}

/// 生产识别阶段：组合有界本地证据、加密连接器凭据、类型化提供方查询、不可变决策提交与
/// 脱敏健康投影。
pub struct IdentificationStageHandler {
    pool: SqlitePool,
    identification: IdentificationService,
    manual: ManualDecisionStore,
    connectors: ConnectorService,
    provider: Arc<dyn MetadataProvider>,
    fs: Option<Arc<dyn CapabilityFs>>,
    clock: Arc<dyn TaskClock>,
    enhancer: Option<EnhancerService>,
}

impl IdentificationStageHandler {
    #[must_use]
    /// 组装生产识别阶段依赖；`fs` 为 `None` 时跳过能力内 NFO 读取而继续文件名识别。
    pub fn new(
        pool: SqlitePool,
        notifier: OutboxNotifier,
        connectors: ConnectorService,
        provider: Arc<dyn MetadataProvider>,
        fs: Option<Arc<dyn CapabilityFs>>,
        clock: Arc<dyn TaskClock>,
    ) -> Self {
        Self {
            identification: IdentificationService::new_with_notifier(pool.clone(), notifier),
            manual: ManualDecisionStore::new(pool.clone()),
            pool,
            connectors,
            provider,
            fs,
            clock,
            enhancer: None,
        }
    }

    #[must_use]
    /// 为新识别 attempt 接入版本化本地增强服务；故障始终在阶段内降级。
    pub fn with_enhancer(mut self, enhancer: EnhancerService) -> Self {
        self.enhancer = Some(enhancer);
        self
    }
}

#[async_trait]
impl ProcessingStageHandler for IdentificationStageHandler {
    fn stage(&self) -> ProcessingStage {
        ProcessingStage::Identification
    }

    #[allow(clippy::too_many_lines)]
    async fn run(
        &self,
        lease: &ProcessingLease,
        stop: ProcessingStopToken,
    ) -> Result<ProcessingHandlerOutcome, AppError> {
        if stop.is_stopped() {
            return Err(stopped());
        }
        let base_hint = FilenameParser::default()
            .parse(&lease.task.relative_path)
            .map_err(|_| AppError::new(ErrorCode::ValidationFailed, "media filename is invalid"))?;
        let source_hash: [u8; 32] = Sha256::digest(lease.task.relative_path.as_bytes()).into();
        let mut hint = base_hint.clone();
        let mut evidence = filename_evidence(&hint, lease.task.relative_path.as_bytes());
        let mut enhancement_guard = EnhancementGuard::default();
        if let Some(enhancer) = &self.enhancer {
            match enhancement_input(&lease.task.relative_path, &base_hint) {
                Ok(input) => match enhancer.enhance(&input).await {
                    Ok(hints) => {
                        let mut applied = apply_enhancement_hints(&base_hint, &hints, source_hash);
                        hint = applied.hint;
                        enhancement_guard = applied.guard;
                        evidence.append(&mut applied.evidence);
                    }
                    Err(error) => evidence.push(enhancer_fallback(error, source_hash)),
                },
                Err(_) => evidence.push(enhancer_fallback(
                    EnhancerError::InvalidResponse,
                    source_hash,
                )),
            }
        }
        if stop.is_stopped() {
            return Err(stopped());
        }
        if let Some(fs) = &self.fs {
            append_nfo_evidence(
                &self.pool,
                fs.as_ref(),
                lease,
                &mut hint,
                &mut evidence,
                &mut enhancement_guard,
            )
            .await?;
        }
        if stop.is_stopped() {
            return Err(stopped());
        }
        let manual_context = if let Some(decision_id) = lease.task.current_task_decision_id {
            self.manual
                .decision_context_for_task(lease.task.id, decision_id)
                .await?
        } else {
            let base_feedback = self
                .manual
                .exact_feedback_for_task(lease.task.id, &base_hint)
                .await?;
            if base_feedback.is_some() || hint == base_hint {
                base_feedback
            } else {
                self.manual
                    .exact_feedback_for_task(lease.task.id, &hint)
                    .await?
            }
        };
        let outcome = match self.connectors.load_tmdb_credential().await {
            Ok((token, locale, region)) => {
                if let Some(context) = &manual_context {
                    self.identification
                        .identify_with_manual_context(
                            lease,
                            &hint,
                            &evidence,
                            context,
                            self.provider.as_ref(),
                            &token,
                            &locale,
                            region.as_deref(),
                            &stop,
                            self.clock.as_ref(),
                        )
                        .await?
                } else {
                    self.identification
                        .identify_guarded(
                            lease,
                            &hint,
                            &evidence,
                            enhancement_guard,
                            self.provider.as_ref(),
                            &token,
                            &locale,
                            region.as_deref(),
                            &stop,
                            self.clock.as_ref(),
                        )
                        .await?
                }
            }
            Err(error) if error.code() == ErrorCode::IntegrationNotConfigured => {
                if stop.is_stopped() {
                    return Err(stopped());
                }
                if let Some(context) = &manual_context {
                    self.identification
                        .identify_manual_provider_failure(
                            lease,
                            &hint,
                            &evidence,
                            context,
                            ProviderError::NotConfigured,
                            self.clock.now_us(),
                        )
                        .await?
                } else {
                    self.identification
                        .identify_provider_failure(
                            lease,
                            &hint,
                            &evidence,
                            ProviderError::NotConfigured,
                            self.clock.now_us(),
                        )
                        .await?
                }
            }
            Err(error) => return Err(error),
        };
        self.connectors
            .record_tmdb_health(outcome.provider_failure, self.clock.now_us())
            .await?;
        Ok(ProcessingHandlerOutcome::Committed)
    }
}

struct LocalFileFacts {
    root_id: RootId,
    inbox_relative_path: RelativePath,
    file_relative_bytes: Vec<u8>,
}

async fn append_nfo_evidence(
    pool: &SqlitePool,
    fs: &dyn CapabilityFs,
    lease: &ProcessingLease,
    hint: &mut ParsedIdentityHint,
    evidence: &mut Vec<EvidenceDraft>,
    enhancement_guard: &mut EnhancementGuard,
) -> Result<(), AppError> {
    let facts = load_file_facts(pool, lease).await?;
    let inbox = fs
        .preflight_directory(&facts.root_id, &facts.inbox_relative_path)
        .map_err(capability_error)?
        .into_capability();
    let components = raw_components(&facts.file_relative_bytes)?;
    let (media_name, directories) = components
        .split_last()
        .ok_or_else(|| AppError::new(ErrorCode::PathInvalid, "media path is empty"))?;
    let mut media_directory = inbox;
    let mut series_directory = None;
    for component in directories {
        series_directory = Some(media_directory.clone());
        let name = os_string_from_bytes(component.to_vec())?;
        media_directory = fs
            .open_child_directory(&media_directory, &name)
            .map_err(capability_error)?;
    }
    let media_name = os_string_from_bytes(media_name.to_vec())?;
    let references = hint
        .season
        .map(|season| {
            hint.episodes
                .iter()
                .map(|episode| (season, *episode))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let located = match hint.media_kind {
        MediaKind::Movie => {
            NfoLocator::new(fs).read_for_media(&media_directory, &media_name, hint.media_kind, None)
        }
        MediaKind::Episode => NfoLocator::new(fs).read_for_episode_references(
            &media_directory,
            &media_name,
            series_directory.as_ref(),
            &references,
        ),
    };
    match located {
        Ok(documents) => {
            for located in documents {
                match NfoParser::default().parse(&located.bytes) {
                    Ok(parsed) => apply_nfo(hint, evidence, &parsed, enhancement_guard),
                    Err(_) => evidence.push(nfo_conflict(Sha256::digest(&located.bytes).into())),
                }
            }
        }
        Err(_) => evidence.push(nfo_conflict(
            Sha256::digest(&facts.file_relative_bytes).into(),
        )),
    }
    evidence.truncate(128);
    Ok(())
}

async fn load_file_facts(
    pool: &SqlitePool,
    lease: &ProcessingLease,
) -> Result<LocalFileFacts, AppError> {
    let row = sqlx::query(
        "SELECT i.root_id,i.relative_path_display,f.relative_path_bytes
         FROM tasks_processing_tasks t
         JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
         JOIN discovery_inbox_directories i ON i.id=t.inbox_directory_id
         WHERE t.id=? AND t.file_revision_id=?",
    )
    .bind(lease.task.id.as_bytes().as_slice())
    .bind(lease.task.file_revision_id.as_bytes().as_slice())
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::with_source(ErrorCode::Internal, error))?
    .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing file facts missing"))?;
    Ok(LocalFileFacts {
        root_id: RootId::parse(&row.get::<String, _>("root_id")).map_err(capability_error)?,
        inbox_relative_path: RelativePath::parse(&row.get::<String, _>("relative_path_display"))
            .map_err(capability_error)?,
        file_relative_bytes: row.get("relative_path_bytes"),
    })
}

fn filename_evidence(hint: &ParsedIdentityHint, source: &[u8]) -> Vec<EvidenceDraft> {
    let source_hash: [u8; 32] = Sha256::digest(source).into();
    let mut evidence = vec![EvidenceDraft {
        source: EvidenceSource::Filename,
        source_version: hint.parser_version.to_owned(),
        kind: EvidenceKind::Title,
        normalized_value: hint.normalized_title.clone(),
        strength: EvidenceStrength::Strong,
        reason: "filename.title".to_owned(),
        source_hash,
    }];
    if let Some(year) = hint.year {
        evidence.push(EvidenceDraft {
            source: EvidenceSource::Filename,
            source_version: hint.parser_version.to_owned(),
            kind: EvidenceKind::Year,
            normalized_value: year.to_string(),
            strength: EvidenceStrength::Strong,
            reason: "filename.year".to_owned(),
            source_hash,
        });
    }
    if let Some(season) = hint.season {
        for episode in &hint.episodes {
            evidence.push(EvidenceDraft {
                source: EvidenceSource::Filename,
                source_version: hint.parser_version.to_owned(),
                kind: EvidenceKind::Episode,
                normalized_value: format!("S{season:02}E{episode:02}"),
                strength: EvidenceStrength::Strong,
                reason: "filename.episode".to_owned(),
                source_hash,
            });
        }
    }
    evidence
}

fn apply_nfo(
    hint: &mut ParsedIdentityHint,
    evidence: &mut Vec<EvidenceDraft>,
    parsed: &crate::identification::nfo::ParsedNfo,
    enhancement_guard: &mut EnhancementGuard,
) {
    if enhancement_guard.media_kind_only()
        && let Some(kind) = parsed.documents.first().map(|document| document.kind)
    {
        let kind = match kind {
            NfoKind::Movie => MediaKind::Movie,
            NfoKind::Episode | NfoKind::TvShow => MediaKind::Episode,
        };
        hint.media_kind = kind;
        enhancement_guard.clear_media_kind();
    }
    for document in &parsed.documents {
        let title = match document.kind {
            NfoKind::Episode => document.show_title.as_ref().or(document.title.as_ref()),
            NfoKind::Movie | NfoKind::TvShow => document.title.as_ref(),
        };
        if let Some(title) = title {
            if enhancement_guard.title_only() {
                title.trim().clone_into(&mut hint.normalized_title);
                enhancement_guard.clear_title();
            }
            evidence.push(nfo_evidence(
                EvidenceKind::Title,
                title.trim().to_lowercase(),
                "nfo.title",
                parsed.document_hash,
            ));
        }
        if let Some(year) = document.year {
            if enhancement_guard.year_only() {
                hint.year = Some(year);
                enhancement_guard.clear_year();
            } else if hint.year.is_some_and(|existing| existing != year) {
                evidence.push(nfo_conflict(parsed.document_hash));
            } else {
                hint.year = Some(year);
                evidence.push(nfo_evidence(
                    EvidenceKind::Year,
                    year.to_string(),
                    "nfo.year",
                    parsed.document_hash,
                ));
            }
        }
        if let (Some(season), Some(episode)) = (document.season, document.episode) {
            if enhancement_guard.episode_only() {
                if hint.season != Some(season) {
                    hint.episodes.clear();
                }
                hint.season = Some(season);
                if !hint.episodes.contains(&episode) {
                    hint.episodes.push(episode);
                    hint.episodes.sort_unstable();
                }
                enhancement_guard.clear_episode();
            }
            evidence.push(nfo_evidence(
                EvidenceKind::Episode,
                format!("S{season:02}E{episode:02}"),
                "nfo.episode",
                parsed.document_hash,
            ));
        }
        for external_id in &document.external_ids {
            if !hint.external_ids.iter().any(|existing| {
                existing.provider == external_id.provider && existing.value == external_id.value
            }) {
                hint.external_ids.push(external_id.clone());
            }
            evidence.push(nfo_evidence(
                EvidenceKind::ExternalId,
                format!("{}:{}", external_id.provider, external_id.value),
                "nfo.external-id",
                parsed.document_hash,
            ));
        }
    }
}

fn enhancement_input(
    relative_path: &str,
    base: &ParsedIdentityHint,
) -> Result<EnhancementInput, AppError> {
    let components = relative_path.split('/').collect::<Vec<_>>();
    let basename = components
        .last()
        .filter(|value| !value.is_empty() && **value != "." && **value != "..")
        .ok_or_else(|| AppError::new(ErrorCode::PathInvalid, "media path is invalid"))?;
    if components[..components.len().saturating_sub(1)]
        .iter()
        .any(|value| value.is_empty() || matches!(*value, "." | ".."))
    {
        return Err(AppError::new(
            ErrorCode::PathInvalid,
            "media path is invalid",
        ));
    }
    let parents = components[..components.len().saturating_sub(1)]
        .iter()
        .rev()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(str::to_owned)
        .collect();
    EnhancementInput::new(
        (*basename).to_owned(),
        parents,
        BaseIdentityHint {
            normalized_title: base.normalized_title.clone(),
            media_kind: match base.media_kind {
                MediaKind::Movie => EnhancerMediaKind::Movie,
                MediaKind::Episode => EnhancerMediaKind::Episode,
            },
            year: base.year,
            season: base.season,
            episodes: base.episodes.clone(),
        },
    )
}

fn enhancer_fallback(error: EnhancerError, source_hash: [u8; 32]) -> EvidenceDraft {
    EvidenceDraft {
        source: EvidenceSource::Enhancer,
        source_version: "ollama-v1".to_owned(),
        kind: EvidenceKind::Availability,
        normalized_value: error.as_str().to_owned(),
        strength: EvidenceStrength::Supporting,
        reason: format!("enhancer.fallback.{}", error.as_str()),
        source_hash,
    }
}

fn nfo_evidence(
    kind: EvidenceKind,
    normalized_value: String,
    reason: &str,
    source_hash: [u8; 32],
) -> EvidenceDraft {
    EvidenceDraft {
        source: EvidenceSource::Nfo,
        source_version: "kodi-v1".to_owned(),
        kind,
        normalized_value,
        strength: EvidenceStrength::Strong,
        reason: reason.to_owned(),
        source_hash,
    }
}

fn nfo_conflict(source_hash: [u8; 32]) -> EvidenceDraft {
    EvidenceDraft {
        source: EvidenceSource::Nfo,
        source_version: "kodi-v1".to_owned(),
        kind: EvidenceKind::Conflict,
        normalized_value: "unsafe-or-conflicting-nfo".to_owned(),
        strength: EvidenceStrength::Conflicting,
        reason: "system.conflict".to_owned(),
        source_hash,
    }
}

fn raw_components(path: &[u8]) -> Result<Vec<&[u8]>, AppError> {
    let components = path.split(|byte| *byte == b'/').collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, b"." | b".."))
    {
        return Err(AppError::new(
            ErrorCode::PathInvalid,
            "media path is invalid",
        ));
    }
    Ok(components)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn os_string_from_bytes(bytes: Vec<u8>) -> Result<OsString, AppError> {
    use std::os::unix::ffi::OsStringExt as _;
    Ok(OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn os_string_from_bytes(bytes: Vec<u8>) -> Result<OsString, AppError> {
    String::from_utf8(bytes)
        .map(OsString::from)
        .map_err(|_| AppError::new(ErrorCode::PathInvalid, "media path is invalid"))
}

fn capability_error(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::RootUnavailable, error.to_string())
}

fn stopped() -> AppError {
    AppError::new(
        ErrorCode::TaskLeaseLost,
        "identification worker was stopped",
    )
}

async fn collect_candidates(
    provider: &dyn MetadataProvider,
    token: &SecretBytes,
    hint: &ParsedIdentityHint,
    locale: &str,
    region: Option<&str>,
) -> Result<Vec<(MetadataCandidate, Vec<ExternalIdHint>)>, ProviderError> {
    let mut candidates = Vec::new();
    for external_id in &hint.external_ids {
        let found = match external_id.provider.as_str() {
            "tmdb" => {
                let Ok(provider_id) = external_id.value.parse::<i64>() else {
                    continue;
                };
                vec![
                    provider
                        .details(token, provider_kind(hint.media_kind), provider_id, locale)
                        .await?,
                ]
            }
            "imdb" | "tvdb" => {
                let source = if external_id.provider == "imdb" {
                    ExternalIdSource::Imdb
                } else {
                    ExternalIdSource::Tvdb
                };
                provider
                    .find_external(
                        token,
                        &TmdbExternalIdRequest {
                            source,
                            external_id: external_id.value.clone(),
                            locale: locale.to_owned(),
                        },
                    )
                    .await?
            }
            _ => Vec::new(),
        };
        for candidate in found {
            merge_candidate(&mut candidates, candidate, Some(external_id.clone()));
        }
    }
    if candidates.is_empty() {
        let found = provider
            .search(
                token,
                &TmdbSearchRequest {
                    media_kind: provider_kind(hint.media_kind),
                    title: hint.normalized_title.clone(),
                    year: hint.year,
                    locale: locale.to_owned(),
                    region: region.map(str::to_owned),
                },
            )
            .await?;
        for candidate in found {
            merge_candidate(&mut candidates, candidate, None);
        }
    }
    if hint.media_kind == MediaKind::Episode {
        let requested = hint
            .season
            .map(|season| {
                hint.episodes
                    .iter()
                    .map(|episode| (season, *episode))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for (candidate, _) in &mut candidates {
            if candidate.identity.media_kind == ProviderMediaKind::Tv {
                candidate.episodes = provider
                    .verify_episodes(token, candidate.identity.provider_id, &requested, locale)
                    .await?;
            }
        }
    }
    candidates.truncate(20);
    Ok(candidates)
}

fn merge_candidate(
    candidates: &mut Vec<(MetadataCandidate, Vec<ExternalIdHint>)>,
    candidate: MetadataCandidate,
    external_id: Option<ExternalIdHint>,
) {
    if let Some((_, external_ids)) = candidates
        .iter_mut()
        .find(|(existing, _)| existing.identity == candidate.identity)
    {
        if let Some(external_id) = external_id
            && !external_ids.iter().any(|existing| {
                existing.provider == external_id.provider && existing.value == external_id.value
            })
        {
            external_ids.push(external_id);
        }
    } else {
        candidates.push((candidate, external_id.into_iter().collect()));
    }
}

fn decision_candidates(
    metadata: &[(MetadataCandidate, Vec<ExternalIdHint>)],
) -> Vec<DecisionCandidate> {
    metadata
        .iter()
        .enumerate()
        .map(|(index, (candidate, external_ids))| DecisionCandidate {
            id: uuid::Uuid::now_v7(),
            identity: candidate.identity,
            titles: candidate
                .titles
                .iter()
                .map(|field| field.value.clone())
                .collect(),
            aliases: candidate
                .aliases
                .iter()
                .map(|field| field.value.clone())
                .collect(),
            year: candidate
                .release_dates
                .first()
                .and_then(|field| u16::try_from(field.value.year()).ok()),
            locale: candidate
                .titles
                .first()
                .map_or_else(|| "en-US".to_owned(), |field| field.language.clone()),
            original_title: candidate
                .titles
                .iter()
                .find(|field| {
                    field.source == crate::connectors::model::FieldLanguageSource::Original
                })
                .map(|field| field.value.clone()),
            release_dates: candidate
                .release_dates
                .iter()
                .map(|field| field.value)
                .collect(),
            episodes: candidate.episodes.clone(),
            external_ids: external_ids.clone(),
            ranking_score: u16::try_from(20_usize.saturating_sub(index)).unwrap_or_default(),
            provider_version: candidate.provider_version,
        })
        .collect()
}

fn append_provider_evidence(
    evidence: &mut Vec<EvidenceDraft>,
    metadata: &[(MetadataCandidate, Vec<ExternalIdHint>)],
) {
    for (candidate, external_ids) in metadata {
        let source_hash = metadata_hash(candidate);
        let source = if candidate.cache_status == CacheStatus::Miss {
            EvidenceSource::Tmdb
        } else {
            EvidenceSource::Cache
        };
        let source_version = format!("tmdb-v{}", candidate.provider_version);
        for external_id in external_ids {
            evidence.push(EvidenceDraft {
                source,
                source_version: source_version.clone(),
                kind: EvidenceKind::ExternalId,
                normalized_value: format!("{}:{}", external_id.provider, external_id.value),
                strength: EvidenceStrength::Strong,
                reason: "tmdb.external-id-verified".to_owned(),
                source_hash,
            });
        }
        if let Some(title) = candidate.titles.first() {
            evidence.push(EvidenceDraft {
                source,
                source_version: source_version.clone(),
                kind: EvidenceKind::Title,
                normalized_value: title.value.clone(),
                strength: EvidenceStrength::Strong,
                reason: "tmdb.title-match".to_owned(),
                source_hash,
            });
        }
        if let Some(date) = candidate.release_dates.first() {
            evidence.push(EvidenceDraft {
                source,
                source_version: source_version.clone(),
                kind: EvidenceKind::Year,
                normalized_value: date.value.year().to_string(),
                strength: EvidenceStrength::Supporting,
                reason: "tmdb.year-match".to_owned(),
                source_hash,
            });
        }
    }
}

fn metadata_hash(candidate: &MetadataCandidate) -> [u8; 32] {
    let mut stable = BTreeMap::new();
    stable.insert("provider_id", candidate.identity.provider_id.to_string());
    stable.insert(
        "media_kind",
        match candidate.identity.media_kind {
            ProviderMediaKind::Movie => "movie".to_owned(),
            ProviderMediaKind::Tv => "tv".to_owned(),
        },
    );
    stable.insert(
        "titles",
        candidate
            .titles
            .iter()
            .map(|field| field.value.as_str())
            .collect::<Vec<_>>()
            .join("\0"),
    );
    let encoded = serde_json::to_vec(&stable).unwrap_or_default();
    Sha256::digest(encoded).into()
}

const fn provider_kind(kind: MediaKind) -> ProviderMediaKind {
    match kind {
        MediaKind::Movie => ProviderMediaKind::Movie,
        MediaKind::Episode => ProviderMediaKind::Tv,
    }
}
