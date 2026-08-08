use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 本地解析推断的媒体类别；在提供方核验前只作为线索使用。
pub enum MediaKind {
    /// 文件名线索指向一部电影。
    Movie,
    /// 文件名线索指向一集或多集剧集内容。
    Episode,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 从作品标题中剥离并单独保留的技术或版本标记。
pub enum VersionTag {
    /// 画面分辨率为 2160p。
    Resolution2160p,
    /// 画面分辨率为 1080p。
    Resolution1080p,
    /// 来源标记为 Blu-ray。
    BluRay,
    /// 来源标记为 WEB-DL。
    WebDl,
    /// 版本为未经重新编码的 Remux。
    Remux,
    /// 视频编码为 HEVC。
    Hevc,
    /// 视频编码为 H.264。
    H264,
    /// 画面带有 HDR 标记。
    Hdr,
    /// 画面带有 Dolby Vision 标记。
    DolbyVision,
    /// 版本为加长版。
    Extended,
    /// 版本为导演剪辑版。
    DirectorsCut,
    /// 发布为修订重打包版本。
    Repack,
    /// 发布为替换原错误版本的正式修正版。
    Proper,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 从文件名或 NFO 中提取的有界显式提供方 ID。
pub struct ExternalIdHint {
    /// 提供方命名空间，例如 `imdb` 或 `tvdb`。
    pub provider: String,
    /// 在该命名空间内用于核验的原始标识。
    pub value: String,
    /// 是否为没有显式命名空间时推断出的默认提供方。
    pub is_default: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 作为识别证据保留的解析结果，不代表已经确定正式目录身份。
pub struct ParsedIdentityHint {
    /// 参与解析的原始文件名或路径片段。
    pub original: String,
    /// 去除技术标签并完成规范化的作品标题。
    pub normalized_title: String,
    /// 本地规则推断出的媒体类别。
    pub media_kind: MediaKind,
    /// 从名称中明确解析出的发行年份。
    pub year: Option<u16>,
    /// 剧集线索中的季号；电影或未提供时为 `None`。
    pub season: Option<u16>,
    /// 剧集线索中的有界集号集合。
    pub episodes: Vec<u16>,
    /// 按日期播出的剧集日期文本；不适用时为 `None`。
    pub air_date: Option<String>,
    /// 文件名或 NFO 携带的显式外部标识。
    pub external_ids: Vec<ExternalIdHint>,
    /// 与作品标题分离的技术和版本标签。
    pub version_tags: Vec<VersionTag>,
    /// 生成当前线索所用解析规则的稳定版本。
    pub parser_version: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 候选身份图中的作品节点；本层不会创建正式目录实体。
pub enum CandidateWork {
    /// 带规范化标题及可选年份的电影候选。
    Movie {
        /// 从本地证据得到的规范化电影标题。
        title: String,
        /// 明确解析出的发行年份；未知时为 `None`。
        year: Option<u16>,
    },
    /// 带规范化标题的剧集候选。
    Series {
        /// 从本地证据得到的规范化剧集标题。
        title: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 由单个物理文件 revision 提示的候选剧集集数。
pub struct CandidateEpisode {
    /// 集数所属的季号。
    pub season: u16,
    /// 季内集号。
    pub episode: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 同一物理文件 revision 中多个集数边共享的候选季节点。
pub struct CandidateSeason {
    /// 从本地线索解析出的季号。
    pub season: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 候选媒体版本及其已解析技术标签。
pub struct CandidateMediaVersion {
    /// 与作品身份无关、仅描述物理版本的标签。
    pub tags: Vec<VersionTag>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 精确根植于一个不可变物理文件 revision 的类型化候选图。
pub struct CandidateIdentityGraph {
    /// 作为全部候选证据根的不可变文件 revision UUID。
    pub file_revision_id: Uuid,
    /// 本地解析推断的作品类别。
    pub media_kind: MediaKind,
    /// 图中唯一的候选作品节点。
    pub work: CandidateWork,
    /// 剧集文件涉及的候选季节点；电影为空。
    pub seasons: Vec<CandidateSeason>,
    /// 该物理文件对应的候选媒体版本。
    pub media_versions: Vec<CandidateMediaVersion>,
    /// 剧集文件涉及的候选集节点；电影为空。
    pub episodes: Vec<CandidateEpisode>,
}

impl CandidateIdentityGraph {
    /// 仅从本地解析线索构建候选图，不创建目录记录。
    ///
    /// # Errors
    ///
    /// 剧集线索缺少季号、集号集合为空或集号数量超过 100 时返回错误。
    pub fn from_hint(
        file_revision_id: Uuid,
        hint: &ParsedIdentityHint,
    ) -> Result<Self, IdentityModelError> {
        let (work, seasons, episodes) = match hint.media_kind {
            MediaKind::Movie => (
                CandidateWork::Movie {
                    title: hint.normalized_title.clone(),
                    year: hint.year,
                },
                Vec::new(),
                Vec::new(),
            ),
            MediaKind::Episode => {
                let season = hint.season.ok_or(IdentityModelError::IncompleteEpisode)?;
                if hint.episodes.is_empty() || hint.episodes.len() > 100 {
                    return Err(IdentityModelError::IncompleteEpisode);
                }
                (
                    CandidateWork::Series {
                        title: hint.normalized_title.clone(),
                    },
                    vec![CandidateSeason { season }],
                    hint.episodes
                        .iter()
                        .copied()
                        .map(|episode| CandidateEpisode { season, episode })
                        .collect(),
                )
            }
        };
        Ok(Self {
            file_revision_id,
            media_kind: hint.media_kind,
            work,
            seasons,
            media_versions: vec![CandidateMediaVersion {
                tags: hint.version_tags.clone(),
            }],
            episodes,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// 本地候选图无法保持结构不变量时的错误。
pub enum IdentityModelError {
    #[error("episode hint is incomplete")]
    /// 剧集线索缺少季号、集号集合为空或集号数量超过 100。
    IncompleteEpisode,
}
