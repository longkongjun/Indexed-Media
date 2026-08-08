use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 一条有界、规范化证据的来源。
pub enum EvidenceSource {
    /// 证据从媒体文件名解析得到。
    Filename,
    /// 证据从能力边界内的本地 NFO 得到。
    Nfo,
    /// 证据来自实时 TMDB 响应。
    Tmdb,
    /// 证据来自持久化提供方缓存。
    Cache,
    /// 证据由本地确定性规则生成。
    System,
    /// 证据来自管理员本次不可变决定。
    ManualDecision,
    /// 证据来自此前保存的管理员反馈。
    ManualFeedback,
    /// 证据来自受限本地模型，只能作为辅助提示。
    Enhancer,
}

impl EvidenceSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Filename => "filename",
            Self::Nfo => "nfo",
            Self::Tmdb => "tmdb",
            Self::Cache => "cache",
            Self::System => "system",
            Self::ManualDecision => "manual-decision",
            Self::ManualFeedback => "manual-feedback",
            Self::Enhancer => "enhancer",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 单条证据值在身份判断中的语义类别。
pub enum EvidenceKind {
    /// 提供方命名空间中的外部实体标识。
    ExternalId,
    /// 作品主标题。
    Title,
    /// 可用于匹配的作品别名。
    Alias,
    /// 发行或首播年份。
    Year,
    /// 季号与集号身份。
    Episode,
    /// 电影或剧集媒体类别。
    MediaType,
    /// 两项或多项事实之间的冲突。
    Conflict,
    /// 提供方或本地数据源的可用性结论。
    Availability,
}

impl EvidenceKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ExternalId => "external-id",
            Self::Title => "title",
            Self::Alias => "alias",
            Self::Year => "year",
            Self::Episode => "episode",
            Self::MediaType => "media-type",
            Self::Conflict => "conflict",
            Self::Availability => "availability",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 证据对身份规则的支持或冲突强度。
pub enum EvidenceStrength {
    /// 单独或与少量事实结合即可支撑确认的强证据。
    Strong,
    /// 只用于增加可信度、不能单独确认的辅助证据。
    Supporting,
    /// 阻止当前候选被自动确认的冲突证据。
    Conflicting,
}

impl EvidenceStrength {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Supporting => "supporting",
            Self::Conflicting => "conflicting",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 可直接持久化且不携带无界来源正文的证据草稿。
pub struct EvidenceDraft {
    /// 产生该证据的系统或人工来源。
    pub source: EvidenceSource,
    /// 来源解析器、映射器或反馈格式的稳定版本。
    pub source_version: String,
    /// 证据值参与身份规则的语义类别。
    pub kind: EvidenceKind,
    /// 用于比较与展示的有界证据值，规范化程度由来源决定；TMDB 标题仅做 trim 与长度限制，
    /// 到决策比较时才执行标题规范化。
    pub normalized_value: String,
    /// 该证据对当前候选的支持或冲突程度。
    pub strength: EvidenceStrength,
    /// 面向审计与人工复核的稳定原因文本。
    pub reason: String,
    /// 用于审计关联的来源摘要：文件名/NFO 使用有界源字节，TMDB 使用提供方 ID、媒体类别与
    /// 标题集合，人工证据使用规范化证据值。
    pub source_hash: [u8; 32],
}
