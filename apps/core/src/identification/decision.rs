use std::collections::BTreeSet;

use chrono::{Datelike as _, NaiveDate};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization as _;
use uuid::Uuid;

use crate::connectors::model::{
    CandidateIdentity, EpisodeIdentity, ProviderError, ProviderMediaKind,
};
use crate::identification::model::{
    CandidateIdentityGraph, ExternalIdHint, MediaKind, ParsedIdentityHint,
};

/// 与每个不可变识别结果一同持久化的决策规则标识。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecisionRules {
    /// 单调递增的决策规则 Schema 版本。
    pub version: u16,
}

impl DecisionRules {
    #[must_use]
    /// 返回当前明确支持的第一版规则集。
    pub const fn v1() -> Self {
        Self { version: 1 }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 一次完整识别尝试的穷尽性结论等级。
pub enum DecisionLevel {
    /// 证据足以唯一确认候选身份。
    Confirmed,
    /// 证据倾向于单个候选，但不足以自动确认。
    Probable,
    /// 存在多个强候选或相互冲突的证据。
    Ambiguous,
    /// 当前证据无法找到可用候选。
    Unidentified,
    /// 外部依赖或安全条件阻止本次识别完成。
    Blocked,
}

impl DecisionLevel {
    #[must_use]
    /// 返回用于持久化和线协议的稳定 kebab-case 值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Probable => "probable",
            Self::Ambiguous => "ambiguous",
            Self::Unidentified => "unidentified",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 决策引擎使用并随结果保存的稳定、可解释事实或冲突。
pub enum DecisionReason {
    /// 显式外部 ID 已由提供方核验。
    ExternalIdVerified,
    /// 规范化标题与候选标题或别名匹配。
    TitleMatched,
    /// 本地年份与候选年份匹配。
    YearMatched,
    /// 本地年份与候选的区域发行日期匹配。
    RegionalReleaseDate,
    /// 本地线索未提供可用于强确认的年份。
    YearMissing,
    /// 同时存在多个强匹配候选，或多个只能达到 probable 的候选。
    MultipleStrongCandidates,
    /// 提供方结果中没有可接受候选。
    NoCandidate,
    /// 提供方暂时不可用，识别可在稍后恢复。
    ProviderUnavailable,
    /// 提供方拒绝了当前凭据。
    ProviderUnauthorized,
    /// 本地媒体类别与候选类别冲突。
    MediaTypeConflict,
    /// 显式外部 ID 指向冲突或不存在的候选。
    ExternalIdConflict,
    /// 剧集候选未包含本地线索要求的集数。
    EpisodeMissing,
    /// NFO 因边界或解析安全问题不能作为证据。
    NfoUnsafe,
    /// 本地标题与候选标题及别名均不匹配。
    TitleConflict,
    /// 本地年份与候选年份或区域日期冲突。
    YearConflict,
    /// 管理员选择的提供方身份已重新核验。
    ManualSelectionVerified,
    /// 人工选择或重新核验的提供方候选覆盖了不一致的本地标题线索。
    ManualTitleOverride,
    /// 当前文件精确命中了已保存的人工反馈。
    ExactFeedbackMatched,
    /// 唯一强匹配依赖仅由增强器提供的关键字段，因此不能自动确认。
    EnhancerOnlyMatch,
}

impl DecisionReason {
    #[must_use]
    /// 返回用于持久化和事件协议的稳定 kebab-case 值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExternalIdVerified => "external-id-verified",
            Self::TitleMatched => "title-matched",
            Self::YearMatched => "year-matched",
            Self::RegionalReleaseDate => "regional-release-date",
            Self::YearMissing => "year-missing",
            Self::MultipleStrongCandidates => "multiple-strong-candidates",
            Self::NoCandidate => "no-candidate",
            Self::ProviderUnavailable => "provider-unavailable",
            Self::ProviderUnauthorized => "provider-unauthorized",
            Self::MediaTypeConflict => "media-type-conflict",
            Self::ExternalIdConflict => "external-id-conflict",
            Self::EpisodeMissing => "episode-missing",
            Self::NfoUnsafe => "nfo-unsafe",
            Self::TitleConflict => "title-conflict",
            Self::YearConflict => "year-conflict",
            Self::ManualSelectionVerified => "manual-selection-verified",
            Self::ManualTitleOverride => "manual-title-override",
            Self::ExactFeedbackMatched => "exact-feedback-matched",
            Self::EnhancerOnlyMatch => "enhancer-only-match",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// 可由增强器独占提供、因而需要限制确认权限的身份字段。
pub enum EnhancedField {
    Title = 1,
    Year = 2,
    MediaKind = 4,
    Episode = 8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 记录当前身份关键字段中哪些只由本地增强器提供。
pub struct EnhancementGuard {
    fields: u8,
}

impl EnhancementGuard {
    #[must_use]
    /// 创建不依赖增强器字段的保护状态。
    pub const fn empty() -> Self {
        Self { fields: 0 }
    }

    #[must_use]
    /// 由提示应用阶段创建一组有界保护标记。
    pub fn from_fields(fields: impl IntoIterator<Item = EnhancedField>) -> Self {
        let mut guard = Self::default();
        for field in fields {
            guard.mark(field);
        }
        guard
    }

    #[must_use]
    /// 任一关键字段仅有模型来源时返回 `true`。
    pub const fn relies_on_enhancer(self) -> bool {
        self.fields != 0
    }

    pub(crate) const fn title_only(self) -> bool {
        self.contains(EnhancedField::Title)
    }

    pub(crate) const fn year_only(self) -> bool {
        self.contains(EnhancedField::Year)
    }

    pub(crate) const fn media_kind_only(self) -> bool {
        self.contains(EnhancedField::MediaKind)
    }

    pub(crate) const fn episode_only(self) -> bool {
        self.contains(EnhancedField::Episode)
    }

    pub(crate) const fn mark(&mut self, field: EnhancedField) {
        self.fields |= field as u8;
    }

    const fn contains(self, field: EnhancedField) -> bool {
        self.fields & field as u8 != 0
    }

    pub(crate) const fn clear_title(&mut self) {
        self.fields &= !(EnhancedField::Title as u8);
    }

    pub(crate) const fn clear_year(&mut self) {
        self.fields &= !(EnhancedField::Year as u8);
    }

    pub(crate) const fn clear_episode(&mut self) {
        self.fields &= !(EnhancedField::Episode as u8);
    }

    pub(crate) const fn clear_media_kind(&mut self) {
        self.fields &= !(EnhancedField::MediaKind as u8);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 映射到纯决策边界、且所有集合均已限制大小的提供方候选。
pub struct DecisionCandidate {
    /// 本次识别尝试内稳定引用候选的 UUID。
    pub id: Uuid,
    /// 提供方确认的稳定外部身份和媒体类别。
    pub identity: CandidateIdentity,
    /// 按回退顺序保留的有界提供方标题原值；仅在比较时执行规范化。
    pub titles: Vec<String>,
    /// 可参与标题匹配的有界别名集合。
    pub aliases: Vec<String>,
    /// 提供方确认的发行或首播年份。
    pub year: Option<u16>,
    /// 生成主要本地化字段的实际 locale。
    pub locale: String,
    /// 提供方的原始语言标题；不可用时为 `None`。
    pub original_title: Option<String>,
    /// 可用于年份核验的有界发行或首播日期集合。
    pub release_dates: Vec<NaiveDate>,
    /// 提供方核验的剧集集数身份；电影候选为空。
    pub episodes: Vec<EpisodeIdentity>,
    /// 与该候选关联的外部命名空间标识。
    pub external_ids: Vec<ExternalIdHint>,
    /// 仅用于显示排序；决策引擎有意不读取该值，避免流行度影响确认。
    pub ranking_score: u16,
    /// 生成候选时使用的提供方领域映射版本。
    pub provider_version: u16,
}

/// 一次确定性决策借用的完整输入；调用期间必须保持线索和候选有效。
pub struct DecisionInput<'a> {
    /// 决策所绑定的不可变物理文件 revision UUID。
    pub file_revision_id: Uuid,
    /// 作为本地事实输入的解析线索。
    pub hint: &'a ParsedIdentityHint,
    /// 已完成有界映射的提供方候选切片。
    pub candidates: &'a [DecisionCandidate],
    /// 候选获取失败时的安全分类；成功时为 `None`。
    pub provider_error: Option<ProviderError>,
    /// 在调用决策引擎前发现的本地证据冲突。
    pub local_conflicts: Vec<DecisionReason>,
    /// 模型独占关键字段的确认上限。
    pub enhancement_guard: EnhancementGuard,
}

impl<'a> DecisionInput<'a> {
    #[must_use]
    /// 创建没有提供方错误和本地冲突的决策输入。
    pub const fn new(
        file_revision_id: Uuid,
        hint: &'a ParsedIdentityHint,
        candidates: &'a [DecisionCandidate],
    ) -> Self {
        Self {
            file_revision_id,
            hint,
            candidates,
            provider_error: None,
            local_conflicts: Vec::new(),
            enhancement_guard: EnhancementGuard::empty(),
        }
    }

    #[must_use]
    /// 附加已脱敏的提供方失败，使规则可选择阻塞或重试语义。
    pub const fn with_provider_error(mut self, error: ProviderError) -> Self {
        self.provider_error = Some(error);
        self
    }

    #[must_use]
    /// 附加调用方已确认的本地冲突；非空集合会阻止自动确认。
    pub fn with_local_conflicts(mut self, conflicts: Vec<DecisionReason>) -> Self {
        self.local_conflicts = conflicts;
        self
    }

    #[must_use]
    /// 附加模型字段保护；不影响已核验外部 ID、NFO 或人工专用路径。
    pub const fn with_enhancement_guard(mut self, guard: EnhancementGuard) -> Self {
        self.enhancement_guard = guard;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 在持久化分配决策 ID 与时间戳之前的不可变决策内容。
pub struct IdentificationDecisionDraft {
    /// 本次尝试的穷尽性结论等级。
    pub level: DecisionLevel,
    /// 唯一选中的候选 UUID；结论为 confirmed 或唯一 probable 时为 `Some`，其余为 `None`。
    pub selected_candidate: Option<Uuid>,
    /// 支撑结论的稳定事实与冲突，按决策规则确定顺序保存。
    pub reasons: Vec<DecisionReason>,
    /// 暂时性阻塞允许再次尝试的 Unix epoch 微秒时间。
    pub retry_at_us: Option<i64>,
    /// 确认身份后产生的本地候选图；其他等级为 `None`。
    pub graph: Option<CandidateIdentityGraph>,
    /// 产生结论的决策规则 Schema 版本。
    pub rule_version: u16,
}

/// 应用显式身份规则；候选流行度永远不参与确认判定。
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn decide(input: &DecisionInput<'_>, rules: &DecisionRules) -> IdentificationDecisionDraft {
    if !input.local_conflicts.is_empty() {
        return draft(
            DecisionLevel::Ambiguous,
            None,
            input.local_conflicts.clone(),
            None,
            None,
            rules,
        );
    }
    let external_matches = matching_external_candidate_ids(input);
    if external_matches.len() > 1 {
        return draft(
            DecisionLevel::Ambiguous,
            None,
            vec![DecisionReason::ExternalIdConflict],
            None,
            None,
            rules,
        );
    }
    if let Some(candidate_id) = external_matches.first().copied()
        && let Some(candidate) = input
            .candidates
            .iter()
            .find(|candidate| candidate.id == candidate_id)
    {
        match basic_identity(input.hint, candidate) {
            Ok(mut reasons) => {
                reasons.insert(0, DecisionReason::ExternalIdVerified);
                return confirmed(input, candidate.id, reasons, rules);
            }
            Err(reason) => {
                return draft(
                    DecisionLevel::Ambiguous,
                    None,
                    vec![reason],
                    None,
                    None,
                    rules,
                );
            }
        }
    }

    if !input.hint.external_ids.is_empty() && input.provider_error.is_none() {
        return draft(
            DecisionLevel::Ambiguous,
            None,
            vec![DecisionReason::ExternalIdConflict],
            None,
            None,
            rules,
        );
    }

    let mut strong = Vec::new();
    let mut probable = Vec::new();
    let mut conflicts = BTreeSet::new();
    for candidate in input.candidates {
        match basic_identity(input.hint, candidate) {
            Ok(reasons) => {
                if reasons.iter().any(|reason| {
                    matches!(
                        reason,
                        DecisionReason::YearMatched | DecisionReason::RegionalReleaseDate
                    )
                }) {
                    strong.push((candidate, reasons));
                } else {
                    probable.push((candidate, reasons));
                }
            }
            Err(reason) => {
                conflicts.insert(reason);
            }
        }
    }
    if strong.len() > 1 {
        return draft(
            DecisionLevel::Ambiguous,
            None,
            vec![DecisionReason::MultipleStrongCandidates],
            None,
            None,
            rules,
        );
    }
    if let Some((candidate, reasons)) = strong.first() {
        if input.enhancement_guard.relies_on_enhancer() {
            let mut reasons = reasons.clone();
            reasons.push(DecisionReason::EnhancerOnlyMatch);
            return draft(
                DecisionLevel::Probable,
                Some(candidate.id),
                reasons,
                None,
                None,
                rules,
            );
        }
        return confirmed(input, candidate.id, reasons.clone(), rules);
    }

    if let Some(error) = input.provider_error {
        let (reason, retry_at_us) = provider_failure(error);
        return draft(
            DecisionLevel::Blocked,
            None,
            vec![reason],
            retry_at_us,
            None,
            rules,
        );
    }

    if probable.len() == 1 {
        let (candidate, reasons) = &probable[0];
        return draft(
            DecisionLevel::Probable,
            Some(candidate.id),
            reasons.clone(),
            None,
            None,
            rules,
        );
    }
    if probable.len() > 1 {
        return draft(
            DecisionLevel::Ambiguous,
            None,
            vec![DecisionReason::MultipleStrongCandidates],
            None,
            None,
            rules,
        );
    }
    if conflicts.contains(&DecisionReason::MediaTypeConflict)
        || conflicts.contains(&DecisionReason::EpisodeMissing)
        || conflicts.contains(&DecisionReason::YearConflict)
    {
        return draft(
            DecisionLevel::Ambiguous,
            None,
            conflicts.into_iter().collect(),
            None,
            None,
            rules,
        );
    }
    draft(
        DecisionLevel::Unidentified,
        None,
        vec![DecisionReason::NoCandidate],
        None,
        None,
        rules,
    )
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn confirmed(
    input: &DecisionInput<'_>,
    candidate_id: Uuid,
    reasons: Vec<DecisionReason>,
    rules: &DecisionRules,
) -> IdentificationDecisionDraft {
    let graph = CandidateIdentityGraph::from_hint(input.file_revision_id, input.hint).ok();
    draft(
        DecisionLevel::Confirmed,
        Some(candidate_id),
        reasons,
        None,
        graph,
        rules,
    )
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn draft(
    level: DecisionLevel,
    selected_candidate: Option<Uuid>,
    reasons: Vec<DecisionReason>,
    retry_at_us: Option<i64>,
    graph: Option<CandidateIdentityGraph>,
    rules: &DecisionRules,
) -> IdentificationDecisionDraft {
    IdentificationDecisionDraft {
        level,
        selected_candidate,
        reasons,
        retry_at_us,
        graph,
        rule_version: rules.version,
    }
}

fn matching_external_candidate_ids(input: &DecisionInput<'_>) -> Vec<Uuid> {
    input
        .candidates
        .iter()
        .filter(|candidate| {
            input.hint.external_ids.iter().any(|hint| {
                (hint.provider == "tmdb"
                    && hint.value.parse::<i64>().ok() == Some(candidate.identity.provider_id))
                    || candidate.external_ids.iter().any(|candidate_id| {
                        candidate_id.provider.eq_ignore_ascii_case(&hint.provider)
                            && candidate_id.value.eq_ignore_ascii_case(&hint.value)
                    })
            })
        })
        .map(|candidate| candidate.id)
        .collect()
}

fn basic_identity(
    hint: &ParsedIdentityHint,
    candidate: &DecisionCandidate,
) -> Result<Vec<DecisionReason>, DecisionReason> {
    if !media_kind_matches(hint.media_kind, candidate.identity.media_kind) {
        return Err(DecisionReason::MediaTypeConflict);
    }
    let title = normalized(&hint.normalized_title);
    if !candidate
        .titles
        .iter()
        .chain(&candidate.aliases)
        .any(|candidate_title| normalized(candidate_title) == title)
    {
        return Err(DecisionReason::TitleConflict);
    }
    if hint.media_kind == MediaKind::Episode {
        let Some(season) = hint.season else {
            return Err(DecisionReason::EpisodeMissing);
        };
        if hint.episodes.is_empty()
            || hint.episodes.iter().any(|episode| {
                !candidate.episodes.contains(&EpisodeIdentity {
                    season,
                    episode: *episode,
                })
            })
        {
            return Err(DecisionReason::EpisodeMissing);
        }
    }
    let mut reasons = vec![DecisionReason::TitleMatched];
    match (hint.year, candidate.year) {
        (Some(local), Some(provider)) if local == provider => {
            reasons.push(DecisionReason::YearMatched);
        }
        (Some(local), Some(provider))
            if local.abs_diff(provider) == 1
                && candidate
                    .release_dates
                    .iter()
                    .any(|date| date.year() == i32::from(local)) =>
        {
            reasons.push(DecisionReason::RegionalReleaseDate);
        }
        (Some(_), Some(_)) => return Err(DecisionReason::YearConflict),
        _ => reasons.push(DecisionReason::YearMissing),
    }
    Ok(reasons)
}

const fn media_kind_matches(local: MediaKind, provider: ProviderMediaKind) -> bool {
    matches!(
        (local, provider),
        (MediaKind::Movie, ProviderMediaKind::Movie) | (MediaKind::Episode, ProviderMediaKind::Tv)
    )
}

fn normalized(value: &str) -> String {
    let folded = value
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let mut normalized = String::with_capacity(folded.len());
    let mut space = false;
    for character in folded.chars() {
        if character.is_whitespace()
            || matches!(
                character,
                '.' | '_' | '-' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        {
            space = !normalized.is_empty();
        } else {
            if space {
                normalized.push(' ');
                space = false;
            }
            normalized.push(character);
        }
    }
    normalized.trim().to_owned()
}

const fn provider_failure(error: ProviderError) -> (DecisionReason, Option<i64>) {
    match error {
        ProviderError::CredentialsInvalid => (DecisionReason::ProviderUnauthorized, None),
        ProviderError::RateLimited { retry_at_us }
        | ProviderError::TemporarilyUnavailable { retry_at_us } => {
            (DecisionReason::ProviderUnavailable, Some(retry_at_us))
        }
        ProviderError::NotConfigured
        | ProviderError::Timeout
        | ProviderError::ResponseTooLarge
        | ProviderError::InvalidResponse => (DecisionReason::ProviderUnavailable, None),
    }
}
