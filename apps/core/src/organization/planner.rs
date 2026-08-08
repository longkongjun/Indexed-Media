use std::collections::BTreeSet;

use serde::Serialize;
use unicode_normalization::UnicodeNormalization as _;
use uuid::Uuid;

use crate::discovery::model::{RelativePath, RootId};

use super::model::{
    ConfirmedNfoInput, ConfirmedNfoMedia, ConfirmedNfoMetadata, NfoConfidence,
    OrganizationNfoPolicy, OrganizationOperation, OrganizationTarget, OrganizationTargetKind,
};
use super::nfo::{NfoDecision, NfoGenerator};

/// API、计划和 journal 共用的安全逻辑位置。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrganizationLocation {
    /// 部署根逻辑 ID。
    pub root_id: RootId,
    /// 根能力内规范相对路径。
    pub relative_path: RelativePath,
}

/// planner 接受的已核对媒体身份，不包含未确认候选。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanningIdentity {
    /// 已核对电影。
    Movie {
        /// 规范展示标题。
        title: String,
        /// 可选发行年份。
        year: Option<u16>,
        /// 可选物理版本标签。
        version_label: Option<String>,
    },
    /// 已核对剧集单文件事实。
    SeriesEpisode {
        /// 剧集标题。
        series_title: String,
        /// 季号。
        season: u16,
        /// 同一物理文件覆盖的有界集号。
        episodes: Vec<u16>,
        /// 可选物理版本标签。
        version_label: Option<String>,
    },
    /// 管理员确认的受限通用视频意图。
    GenericVideo {
        /// 单项展示标题。
        title: String,
        /// 唯一确定的分组；歧义时为 `None`。
        group: Option<String>,
        /// 分组内连续编号；歧义时为 `None`。
        sequence: Option<u16>,
    },
}

impl PlanningIdentity {
    const fn kind(&self) -> OrganizationTargetKind {
        match self {
            Self::Movie { .. } => OrganizationTargetKind::Movie,
            Self::SeriesEpisode { .. } => OrganizationTargetKind::Series,
            Self::GenericVideo { .. } => OrganizationTargetKind::GenericVideo,
        }
    }
}

/// 纯 planner 的完整、有界输入。
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct PlanningInput {
    /// `ProcessingTask` 稳定 ID。
    pub task_id: Uuid,
    /// 不可变文件 revision ID。
    pub file_revision_id: Uuid,
    /// 已选择提供方身份或人工决定 ID。
    pub selected_identity_id: Option<Uuid>,
    /// 已核对来源逻辑位置。
    pub source: OrganizationLocation,
    /// 来源收件箱 ID。
    pub source_inbox_id: Uuid,
    /// move 删除来源所需的写策略是否具备。
    pub source_writable: bool,
    /// 规划瞬间来源 revision/身份是否仍为当前事实。
    pub source_unchanged: bool,
    /// 目标位置是否已由非本 operation 内容占用。
    pub destination_exists: bool,
    /// 来源与目标是否已确认处于同一文件系统。
    pub same_filesystem: bool,
    /// 当前任务携带的有界显式标签。
    pub explicit_tags: BTreeSet<String>,
    /// 是否已有只绑定当前计划意图的一次性授权。
    pub one_time_authorized: bool,
    /// 已核对身份。
    pub identity: PlanningIdentity,
    /// 已从选中身份收窄的可选 NFO 核心字段。
    pub nfo_metadata: ConfirmedNfoMetadata,
    /// 当前完整目标/profile/rule 聚合快照。
    pub target: OrganizationTarget,
}

/// 计划的持久授权结论。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanAuthorization {
    /// 明确规则、profile 和全部安全不变量允许自动执行。
    Automatic,
    /// 只绑定当前计划的一次性人工授权。
    OneTime,
    /// 计划可审查，但不得执行。
    Paused,
}

impl PlanAuthorization {
    /// 返回数据库与公开契约共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::OneTime => "one-time",
            Self::Paused => "paused",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "automatic" => Some(Self::Automatic),
            "one-time" => Some(Self::OneTime),
            "paused" => Some(Self::Paused),
            _ => None,
        }
    }
}

/// 阻止自动或一次性执行的稳定风险分类。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum PlanningRiskCode {
    /// 没有启用规则覆盖当前任务。
    #[serde(rename = "organization.rule-not-matched")]
    RuleNotMatched,
    /// profile 未开启低风险自动执行。
    #[serde(rename = "organization.automatic-disabled")]
    AutomaticDisabled,
    /// 目标已被禁用。
    #[serde(rename = "organization.target-disabled")]
    TargetDisabled,
    /// 来源 revision 或身份已改变。
    #[serde(rename = "organization.source-changed")]
    SourceChanged,
    /// 目标已有非本 operation 内容。
    #[serde(rename = "organization.target-exists")]
    TargetExists,
    /// move 要求的来源写能力不存在。
    #[serde(rename = "organization.source-read-only")]
    SourceReadOnly,
    /// hardlink 的来源与目标不在同一文件系统。
    #[serde(rename = "organization.hardlink-cross-device")]
    HardlinkCrossDevice,
}

impl PlanningRiskCode {
    /// 返回数据库与公开投影共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RuleNotMatched => "organization.rule-not-matched",
            Self::AutomaticDisabled => "organization.automatic-disabled",
            Self::TargetDisabled => "organization.target-disabled",
            Self::SourceChanged => "organization.source-changed",
            Self::TargetExists => "organization.target-exists",
            Self::SourceReadOnly => "organization.source-read-only",
            Self::HardlinkCrossDevice => "organization.hardlink-cross-device",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "organization.rule-not-matched" => Some(Self::RuleNotMatched),
            "organization.automatic-disabled" => Some(Self::AutomaticDisabled),
            "organization.target-disabled" => Some(Self::TargetDisabled),
            "organization.source-changed" => Some(Self::SourceChanged),
            "organization.target-exists" => Some(Self::TargetExists),
            "organization.source-read-only" => Some(Self::SourceReadOnly),
            "organization.hardlink-cross-device" => Some(Self::HardlinkCrossDevice),
            _ => None,
        }
    }
}

/// 纯 planner 无法构造安全计划时的暂停结论。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningDecision {
    /// 通用视频分组或编号不唯一。
    GenericGroupingAmbiguous,
    /// 命名无法安全表达为目标根内路径。
    PathOutsideRoot,
    /// 已核对身份与目标类型不一致。
    TargetKindMismatch,
    /// 已核对剧集身份缺少有界集号。
    IdentityIncomplete,
}

/// 配置字段来源类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceSourceKind {
    /// 来源于目标配置。
    Target,
    /// 来源于固定 profile。
    Profile,
    /// 来源于命中的结构化规则。
    Rule,
    /// 来源于只绑定当前计划的人工决定。
    ManualDecision,
}

impl ProvenanceSourceKind {
    /// 返回数据库稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Profile => "profile",
            Self::Rule => "rule",
            Self::ManualDecision => "manual-decision",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "target" => Some(Self::Target),
            "profile" => Some(Self::Profile),
            "rule" => Some(Self::Rule),
            "manual-decision" => Some(Self::ManualDecision),
            _ => None,
        }
    }
}

/// 一个生效配置字段的可追踪来源。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConfigProvenance {
    /// 有界稳定字段名。
    pub field: String,
    /// 产生字段的配置层。
    pub source_kind: ProvenanceSourceKind,
    /// 来源实体；系统默认没有实体时为 `None`。
    pub source_id: Option<Uuid>,
    /// 来源聚合版本。
    pub source_version: i64,
}

/// 不可变计划中一个待 journal 化的操作类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlannedOperationKind {
    /// 执行 profile 固定的文件操作。
    File,
    /// 仅在缺失时生成有界 NFO。
    EnsureMissingNfo,
}

impl PlannedOperationKind {
    /// 返回数据库稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::EnsureMissingNfo => "ensure-missing-nfo",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "file" => Some(Self::File),
            "ensure-missing-nfo" => Some(Self::EnsureMissingNfo),
            _ => None,
        }
    }
}

/// 尚未分配持久 operation ID 的计划步骤。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlannedOperationDraft {
    /// 固定步骤类别。
    pub kind: PlannedOperationKind,
    /// 步骤的能力安全目标位置。
    pub destination: OrganizationLocation,
}

/// 由纯 planner 生成、可在一个数据库事务内提交的完整计划草稿。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanDraft {
    /// `ProcessingTask` 稳定 ID。
    pub task_id: Uuid,
    /// 不可变文件 revision ID。
    pub file_revision_id: Uuid,
    /// 已选择身份或人工决定 ID。
    pub selected_identity_id: Option<Uuid>,
    /// 来源逻辑位置。
    pub source: OrganizationLocation,
    /// 目标媒体文件逻辑位置。
    pub destination: OrganizationLocation,
    /// 固定文件操作。
    pub operation: OrganizationOperation,
    /// 人类可读的确定性文件名。
    pub naming: String,
    /// 当前授权结论。
    pub authorization: PlanAuthorization,
    /// 阻止执行的有序风险集合。
    pub risk_codes: Vec<PlanningRiskCode>,
    /// 当前完整目标/profile/rule 快照。
    pub target: OrganizationTarget,
    /// 仅在计划生成缺失 NFO 时保存的已确认、有界输入。
    pub nfo_input: Option<ConfirmedNfoInput>,
    /// 逐字段配置来源。
    pub provenance: Vec<ConfigProvenance>,
    /// 在任何文件影响前必须全部分配稳定 ID 的步骤。
    pub operations: Vec<PlannedOperationDraft>,
}

/// 无 I/O 的确定性组织计划器。
pub struct OrganizationPlanner;

impl OrganizationPlanner {
    /// 从已核对身份、安全事实和固定目标配置生成计划草稿。
    ///
    /// # Errors
    ///
    /// 身份不完整、类型不匹配或命名无法安全约束在目标根内时返回暂停结论。
    pub fn plan(input: &PlanningInput) -> Result<PlanDraft, PlanningDecision> {
        if input.identity.kind() != input.target.kind {
            return Err(PlanningDecision::TargetKindMismatch);
        }
        let extension = source_extension(&input.source.relative_path)?;
        let (directories, filename) = planned_name(&input.identity, extension)?;
        let destination_path = join_target(&input.target.relative_path, &directories, &filename)?;
        let destination = OrganizationLocation {
            root_id: input.target.root_id.clone(),
            relative_path: destination_path,
        };
        let matching_rule = input.target.rules.iter().any(|rule| {
            rule.enabled
                && rule.media_kind == input.target.kind
                && rule
                    .inbox_directory_id
                    .is_none_or(|id| id == input.source_inbox_id)
                && rule
                    .explicit_tag
                    .as_ref()
                    .is_none_or(|tag| input.explicit_tags.contains(tag))
        });
        let mut safety_risks = Vec::new();
        if !input.source_unchanged {
            safety_risks.push(PlanningRiskCode::SourceChanged);
        }
        if input.destination_exists {
            safety_risks.push(PlanningRiskCode::TargetExists);
        }
        if input.target.operation == OrganizationOperation::Move && !input.source_writable {
            safety_risks.push(PlanningRiskCode::SourceReadOnly);
        }
        if input.target.operation == OrganizationOperation::Hardlink && !input.same_filesystem {
            safety_risks.push(PlanningRiskCode::HardlinkCrossDevice);
        }
        if !input.target.enabled {
            safety_risks.push(PlanningRiskCode::TargetDisabled);
        }
        let (authorization, risk_codes) = if !safety_risks.is_empty() {
            (PlanAuthorization::Paused, safety_risks)
        } else if input.one_time_authorized {
            (PlanAuthorization::OneTime, Vec::new())
        } else if input.target.automatic && matching_rule {
            (PlanAuthorization::Automatic, Vec::new())
        } else {
            let mut risks = Vec::new();
            if !matching_rule {
                risks.push(PlanningRiskCode::RuleNotMatched);
            }
            if !input.target.automatic {
                risks.push(PlanningRiskCode::AutomaticDisabled);
            }
            (PlanAuthorization::Paused, risks)
        };
        let mut operations = vec![PlannedOperationDraft {
            kind: PlannedOperationKind::File,
            destination: destination.clone(),
        }];
        let nfo_input = if input.target.nfo_policy == OrganizationNfoPolicy::GenerateMissing {
            let nfo_destination = OrganizationLocation {
                root_id: destination.root_id.clone(),
                relative_path: nfo_path(&destination.relative_path)?,
            };
            operations.push(PlannedOperationDraft {
                kind: PlannedOperationKind::EnsureMissingNfo,
                destination: nfo_destination.clone(),
            });
            Some(confirmed_nfo_input(
                &input.identity,
                &input.nfo_metadata,
                &nfo_destination,
            )?)
        } else {
            None
        };
        Ok(PlanDraft {
            task_id: input.task_id,
            file_revision_id: input.file_revision_id,
            selected_identity_id: input.selected_identity_id,
            source: input.source.clone(),
            destination,
            operation: input.target.operation,
            naming: filename,
            authorization,
            risk_codes,
            target: input.target.clone(),
            nfo_input,
            provenance: provenance(input, matching_rule),
            operations,
        })
    }
}

fn confirmed_nfo_input(
    identity: &PlanningIdentity,
    metadata: &ConfirmedNfoMetadata,
    destination: &OrganizationLocation,
) -> Result<ConfirmedNfoInput, PlanningDecision> {
    let file_name = destination
        .relative_path
        .as_str()
        .rsplit('/')
        .next()
        .ok_or(PlanningDecision::PathOutsideRoot)?
        .to_owned();
    let media = match identity {
        PlanningIdentity::Movie { title, year, .. } => ConfirmedNfoMedia::Movie {
            title: title.clone(),
            original_title: metadata.original_title.clone(),
            year: year.or(metadata.year),
            plot: metadata.plot.clone(),
        },
        PlanningIdentity::SeriesEpisode {
            series_title,
            season,
            episodes,
            ..
        } => ConfirmedNfoMedia::Episode {
            title: series_title.clone(),
            original_title: metadata.original_title.clone(),
            year: metadata.year,
            season: *season,
            episodes: episodes.clone(),
            plot: metadata.plot.clone(),
        },
        PlanningIdentity::GenericVideo {
            title,
            sequence: Some(sequence),
            ..
        } => ConfirmedNfoMedia::GenericVideo {
            title: title.clone(),
            sequence: *sequence,
        },
        PlanningIdentity::GenericVideo { sequence: None, .. } => {
            return Err(PlanningDecision::GenericGroupingAmbiguous);
        }
    };
    let input = ConfirmedNfoInput {
        confidence: NfoConfidence::Confirmed,
        file_name,
        media,
        provider_id: (!matches!(identity, PlanningIdentity::GenericVideo { .. }))
            .then(|| metadata.provider_id.clone())
            .flatten(),
    };
    if matches!(
        NfoGenerator.decide(&input, None),
        Ok(NfoDecision::Generate { .. })
    ) {
        Ok(input)
    } else {
        Err(PlanningDecision::IdentityIncomplete)
    }
}

fn planned_name(
    identity: &PlanningIdentity,
    extension: &str,
) -> Result<(Vec<String>, String), PlanningDecision> {
    match identity {
        PlanningIdentity::Movie {
            title,
            year,
            version_label,
        } => {
            let title = safe_component(title)?;
            let base = year.map_or_else(|| title.clone(), |year| format!("{title} ({year})"));
            let filename = with_version(&base, version_label.as_deref())?;
            Ok((vec![base], format!("{filename}.{extension}")))
        }
        PlanningIdentity::SeriesEpisode {
            series_title,
            season,
            episodes,
            version_label,
        } => {
            if episodes.is_empty() || episodes.len() > 32 {
                return Err(PlanningDecision::IdentityIncomplete);
            }
            let mut episodes = episodes.clone();
            episodes.sort_unstable();
            episodes.dedup();
            let series = safe_component(series_title)?;
            let episode_label = if episodes.len() == 1 {
                format!("S{season:02}E{:02}", episodes[0])
            } else {
                format!(
                    "S{season:02}E{:02}-E{:02}",
                    episodes[0],
                    episodes[episodes.len() - 1]
                )
            };
            let base = format!("{series} - {episode_label}");
            let filename = with_version(&base, version_label.as_deref())?;
            Ok((
                vec![series, format!("Season {season:02}")],
                format!("{filename}.{extension}"),
            ))
        }
        PlanningIdentity::GenericVideo {
            title,
            group,
            sequence,
        } => {
            let (Some(group), Some(sequence)) = (group.as_deref(), *sequence) else {
                return Err(PlanningDecision::GenericGroupingAmbiguous);
            };
            if sequence == 0 {
                return Err(PlanningDecision::GenericGroupingAmbiguous);
            }
            let group = safe_component(group)?;
            let title = safe_component(title)?;
            Ok((
                vec![group.clone()],
                format!("{group} - {sequence:03} - {title}.{extension}"),
            ))
        }
    }
}

fn with_version(base: &str, version: Option<&str>) -> Result<String, PlanningDecision> {
    version.map_or_else(
        || Ok(base.to_owned()),
        |version| Ok(format!("{base} - {}", safe_component(version)?)),
    )
}

fn safe_component(value: &str) -> Result<String, PlanningDecision> {
    let mapped = value
        .nfkc()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let collapsed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches([' ', '.']).to_owned();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return Err(PlanningDecision::PathOutsideRoot);
    }
    let mut bounded = trimmed.chars().take(180).collect::<String>();
    if is_windows_reserved(&bounded) {
        bounded.push('_');
    }
    if bounded.is_empty() {
        Err(PlanningDecision::PathOutsideRoot)
    } else {
        Ok(bounded)
    }
}

fn is_windows_reserved(value: &str) -> bool {
    let stem = value
        .split_once('.')
        .map_or(value, |(stem, _)| stem)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit() && suffix != "0"
            })
}

fn source_extension(path: &RelativePath) -> Result<&str, PlanningDecision> {
    let extension = path
        .as_str()
        .rsplit_once('.')
        .map(|(_, extension)| extension)
        .filter(|value| {
            (1..=10).contains(&value.len())
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .ok_or(PlanningDecision::PathOutsideRoot)?;
    Ok(extension)
}

fn join_target(
    base: &RelativePath,
    directories: &[String],
    filename: &str,
) -> Result<RelativePath, PlanningDecision> {
    let mut components = Vec::new();
    if base.as_str() != "." {
        components.push(base.as_str());
    }
    components.extend(directories.iter().map(String::as_str));
    components.push(filename);
    let value = components.join("/");
    if value.chars().count() > 4096 {
        return Err(PlanningDecision::PathOutsideRoot);
    }
    RelativePath::parse(&value).map_err(|_| PlanningDecision::PathOutsideRoot)
}

fn nfo_path(path: &RelativePath) -> Result<RelativePath, PlanningDecision> {
    let (stem, _) = path
        .as_str()
        .rsplit_once('.')
        .ok_or(PlanningDecision::PathOutsideRoot)?;
    RelativePath::parse(&format!("{stem}.nfo")).map_err(|_| PlanningDecision::PathOutsideRoot)
}

fn provenance(input: &PlanningInput, matching_rule: bool) -> Vec<ConfigProvenance> {
    let target_id = Some(input.target.id);
    let version = input.target.config_version;
    let mut values = vec![
        ConfigProvenance {
            field: "target".to_owned(),
            source_kind: ProvenanceSourceKind::Target,
            source_id: target_id,
            source_version: version,
        },
        ConfigProvenance {
            field: "operation".to_owned(),
            source_kind: ProvenanceSourceKind::Profile,
            source_id: target_id,
            source_version: version,
        },
        ConfigProvenance {
            field: "naming-pattern".to_owned(),
            source_kind: ProvenanceSourceKind::Profile,
            source_id: target_id,
            source_version: version,
        },
        ConfigProvenance {
            field: "nfo-policy".to_owned(),
            source_kind: ProvenanceSourceKind::Profile,
            source_id: target_id,
            source_version: version,
        },
        ConfigProvenance {
            field: "automatic".to_owned(),
            source_kind: ProvenanceSourceKind::Profile,
            source_id: target_id,
            source_version: version,
        },
    ];
    if matching_rule {
        values.push(ConfigProvenance {
            field: "rule".to_owned(),
            source_kind: ProvenanceSourceKind::Rule,
            source_id: target_id,
            source_version: version,
        });
    }
    if input.one_time_authorized {
        values.push(ConfigProvenance {
            field: "authorization".to_owned(),
            source_kind: ProvenanceSourceKind::ManualDecision,
            source_id: input.selected_identity_id,
            source_version: 1,
        });
    }
    values
}
