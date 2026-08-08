use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::discovery::model::{RelativePath, RootId};
use crate::shared::error::{AppError, ErrorCode};

/// 一个整理目标能够接收的固定媒体类型。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrganizationTargetKind {
    /// 电影资源库。
    Movie,
    /// 剧集资源库。
    Series,
    /// 不伪造外部身份的受限通用视频资源库。
    GenericVideo,
}

impl OrganizationTargetKind {
    /// 返回数据库与公开契约共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::GenericVideo => "generic-video",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "movie" => Some(Self::Movie),
            "series" => Some(Self::Series),
            "generic-video" => Some(Self::GenericVideo),
            _ => None,
        }
    }
}

/// 一个 profile 固定选择的文件操作；执行期不得静默降级。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OrganizationOperation {
    /// 核对后移动来源文件。
    Move,
    /// 以 no-clobber 语义复制来源文件。
    Copy,
    /// 只在同文件系统内创建硬链接。
    Hardlink,
}

impl OrganizationOperation {
    /// 返回数据库与公开契约共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Copy => "copy",
            Self::Hardlink => "hardlink",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "move" => Some(Self::Move),
            "copy" => Some(Self::Copy),
            "hardlink" => Some(Self::Hardlink),
            _ => None,
        }
    }
}

/// 首版支持的确定性命名模式。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrganizationNamingPattern {
    /// 电影名称与年份模式。
    Movie,
    /// 剧集季/集模式。
    Series,
    /// 通用视频规范化名称与连续编号模式。
    GenericNumbered,
}

impl OrganizationNamingPattern {
    /// 返回数据库与公开契约共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::GenericNumbered => "generic-numbered",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "movie" => Some(Self::Movie),
            "series" => Some(Self::Series),
            "generic-numbered" => Some(Self::GenericNumbered),
            _ => None,
        }
    }
}

/// 已有 NFO 始终逐字节保留时可选择的缺失文件策略。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrganizationNfoPolicy {
    /// 只保留已有 NFO，不创建新文件。
    PreserveOnly,
    /// 已有 NFO 仍保持不变，仅创建缺失的有界 NFO。
    GenerateMissing,
}

/// 进入 NFO 生成边界前已由 identification 或人工流程确认的置信度。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NfoConfidence {
    /// 唯一身份已经核对，可生成缺失 NFO。
    Confirmed,
    /// 只有低置信度候选，不得写 NFO。
    LowConfidence,
    /// 尚未确认身份，不得写 NFO。
    Unconfirmed,
}

/// 首版允许写入 Kodi NFO 的固定外部 ID 命名空间。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NfoProvider {
    /// The Movie Database。
    Tmdb,
    /// `TheTVDB`。
    Tvdb,
    /// `IMDb`。
    Imdb,
}

impl NfoProvider {
    /// 返回固定 XML `type` 属性值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tmdb => "tmdb",
            Self::Tvdb => "tvdb",
            Self::Imdb => "imdb",
        }
    }
}

/// 从候选集合中唯一确认后留下的单一 provider ID。
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedProviderId {
    /// 固定 provider 命名空间。
    pub provider: NfoProvider,
    /// 已核对的有界 provider 实体 ID。
    pub value: String,
}

impl std::fmt::Debug for ConfirmedProviderId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfirmedProviderId")
            .field("provider", &self.provider)
            .field("value", &"[REDACTED]")
            .finish()
    }
}

/// 固定 NFO serializer 可消费的已确认媒体核心字段。
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ConfirmedNfoMedia {
    /// 电影核心字段。
    Movie {
        /// 规范标题。
        title: String,
        /// 可选原语言标题。
        original_title: Option<String>,
        /// 可选发行年份。
        year: Option<u16>,
        /// 可选已映射简介；不接收 provider 原始正文对象。
        plot: Option<String>,
    },
    /// 单个物理文件承载的一集或多集。
    Episode {
        /// 规范集标题。
        title: String,
        /// 可选原语言标题。
        original_title: Option<String>,
        /// 可选首播年份。
        year: Option<u16>,
        /// 已核对季号。
        season: u16,
        /// 已核对、输出前排序去重的集号。
        episodes: Vec<u16>,
        /// 可选已映射简介。
        plot: Option<String>,
    },
    /// 不伪造外部作品身份的受限通用视频。
    GenericVideo {
        /// 管理员确认标题。
        title: String,
        /// 分组内确定序号。
        sequence: u16,
    },
}

impl ConfirmedNfoMedia {
    /// 返回不含用户或 provider 内容的稳定类别。
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Movie { .. } => "movie",
            Self::Episode { .. } => "episode",
            Self::GenericVideo { .. } => "generic-video",
        }
    }
}

impl std::fmt::Debug for ConfirmedNfoMedia {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfirmedNfoMedia")
            .field("kind", &self.kind_name())
            .field("content", &"[REDACTED]")
            .finish()
    }
}

/// 不可变 organization plan 持有的有界 NFO 生成输入。
#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedNfoInput {
    /// 只有 `Confirmed` 允许生成；其他值 fail closed 为 `Skip`。
    pub confidence: NfoConfidence,
    /// 已由 planner 约束在目标目录内的 NFO 文件名。
    pub file_name: String,
    /// 固定核心字段集合。
    pub media: ConfirmedNfoMedia,
    /// 唯一确认的 provider ID；通用视频必须为空。
    pub provider_id: Option<ConfirmedProviderId>,
}

/// planning 端口从已选中身份映射出的可选 NFO 核心补充字段。
///
/// 它不包含候选集合、provider 响应正文或任意 XML；provider ID 在进入本类型前必须已折叠为一个。
#[derive(Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedNfoMetadata {
    /// 可选原语言标题。
    pub original_title: Option<String>,
    /// 电影身份没有年份或剧集需要首播年份时使用的已核对年份。
    pub year: Option<u16>,
    /// 可选已映射简介纯文本。
    pub plot: Option<String>,
    /// 唯一确认的 provider ID。
    pub provider_id: Option<ConfirmedProviderId>,
}

impl std::fmt::Debug for ConfirmedNfoMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfirmedNfoMetadata")
            .field(
                "original_title",
                &self.original_title.as_ref().map(|_| "[REDACTED]"),
            )
            .field("year", &self.year)
            .field("plot", &self.plot.as_ref().map(|_| "[REDACTED]"))
            .field("provider_id", &self.provider_id)
            .finish()
    }
}

impl std::fmt::Debug for ConfirmedNfoInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfirmedNfoInput")
            .field("confidence", &self.confidence)
            .field("media_kind", &self.media.kind_name())
            .field("file_name", &"[REDACTED]")
            .field("provider_id", &self.provider_id)
            .finish()
    }
}

impl OrganizationNfoPolicy {
    /// 返回数据库与公开契约共用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreserveOnly => "preserve-only",
            Self::GenerateMissing => "generate-missing",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "preserve-only" => Some(Self::PreserveOnly),
            "generate-missing" => Some(Self::GenerateMissing),
            _ => None,
        }
    }
}

/// 不含脚本、正则或外部命令的结构化匹配规则输入。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationRuleInput {
    /// 规则覆盖的媒体类型。
    pub media_kind: OrganizationTargetKind,
    /// 可选的稳定来源收件箱 ID。
    pub inbox_directory_id: Option<Uuid>,
    /// 可选的安全显式标签。
    pub explicit_tag: Option<String>,
    /// 是否允许新计划使用该规则。
    pub enabled: bool,
}

/// 从 HTTP 边界接收、尚未解析逻辑根和相对路径的严格目标命令。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationTargetCommand {
    /// 目标媒体类型。
    pub kind: OrganizationTargetKind,
    /// 管理员可读名称。
    pub display_name: String,
    /// 已配置部署根的稳定逻辑 ID。
    pub root_id: String,
    /// 根能力内的相对目录。
    pub relative_path: String,
    /// 固定文件操作。
    pub operation: OrganizationOperation,
    /// 固定命名模式。
    pub naming_pattern: OrganizationNamingPattern,
    /// 缺失 NFO 策略。
    pub nfo_policy: OrganizationNfoPolicy,
    /// 是否允许低风险自动执行。
    pub automatic: bool,
    /// 是否允许新计划使用该目标。
    pub enabled: bool,
    /// 有界结构化规则。
    pub rules: Vec<OrganizationRuleInput>,
}

impl OrganizationTargetCommand {
    /// 解析逻辑根与相对路径并构造领域输入。
    ///
    /// # Errors
    ///
    /// 根 ID 或相对路径语法无效时返回稳定路径校验错误。
    pub fn into_input(self) -> Result<OrganizationTargetInput, AppError> {
        let root_id = RootId::parse(&self.root_id)
            .map_err(|error| AppError::new(ErrorCode::PathInvalid, error.to_string()))?;
        let relative_path = RelativePath::parse(&self.relative_path)
            .map_err(|error| AppError::new(ErrorCode::PathInvalid, error.to_string()))?;
        Ok(OrganizationTargetInput {
            kind: self.kind,
            display_name: self.display_name,
            root_id,
            relative_path,
            operation: self.operation,
            naming_pattern: self.naming_pattern,
            nfo_policy: self.nfo_policy,
            automatic: self.automatic,
            enabled: self.enabled,
            rules: self.rules,
        })
    }
}

/// 无副作用目标预检接受的严格逻辑位置。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OrganizationTargetPreflightCommand {
    /// 已配置部署根的稳定逻辑 ID。
    pub root_id: String,
    /// 根能力内的相对目录。
    pub relative_path: String,
}

/// 目标、固定 profile 与规则的单次聚合写入输入。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrganizationTargetInput {
    /// 目标媒体类型。
    pub kind: OrganizationTargetKind,
    /// 管理员可读名称。
    pub display_name: String,
    /// 已配置部署根的稳定逻辑 ID。
    pub root_id: RootId,
    /// 根能力内的规范相对目录。
    pub relative_path: RelativePath,
    /// 固定文件操作。
    pub operation: OrganizationOperation,
    /// 固定命名模式。
    pub naming_pattern: OrganizationNamingPattern,
    /// 缺失 NFO 策略。
    pub nfo_policy: OrganizationNfoPolicy,
    /// 安全门禁全部通过时是否允许低风险自动执行。
    pub automatic: bool,
    /// 是否允许新计划使用该目标。
    pub enabled: bool,
    /// 按顺序求值的有界结构化规则。
    pub rules: Vec<OrganizationRuleInput>,
}

impl OrganizationTargetInput {
    pub(crate) fn validate(mut self) -> Result<Self, AppError> {
        self.display_name = self.display_name.trim().to_owned();
        if !(1..=100).contains(&self.display_name.chars().count())
            || self.display_name.chars().any(char::is_control)
            || !(1..=4096).contains(&self.relative_path.as_str().len())
            || self.rules.len() > 100
            || !matches!(
                (self.kind, self.naming_pattern),
                (
                    OrganizationTargetKind::Movie,
                    OrganizationNamingPattern::Movie
                ) | (
                    OrganizationTargetKind::Series,
                    OrganizationNamingPattern::Series
                ) | (
                    OrganizationTargetKind::GenericVideo,
                    OrganizationNamingPattern::GenericNumbered
                )
            )
            || self.rules.iter().any(|rule| {
                rule.media_kind != self.kind
                    || rule
                        .explicit_tag
                        .as_deref()
                        .is_some_and(|tag| !valid_explicit_tag(tag))
            })
        {
            return Err(validation_error());
        }
        Ok(self)
    }
}

/// 不含宿主路径的版本化整理目标投影。
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OrganizationTarget {
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
    /// 是否允许新计划使用该目标。
    pub enabled: bool,
    /// 有界规则，按持久 ordinal 排序。
    pub rules: Vec<OrganizationRuleInput>,
    /// 聚合配置版本。
    pub config_version: i64,
    /// 最近一次聚合提交的微秒时间戳。
    pub updated_at_us: i64,
}

fn valid_explicit_tag(tag: &str) -> bool {
    let bytes = tag.as_bytes();
    (1..=100).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validation_error() -> AppError {
    AppError::new(
        ErrorCode::ValidationFailed,
        "invalid organization target aggregate",
    )
}
