use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 正式目录中可独立浏览的媒体聚合类型。
pub enum MediaItemKind {
    /// 以一部电影为根的媒体树。
    Movie,
    /// 以一部剧集为根、可包含季与集的媒体树。
    Series,
    /// 无法或无需映射到电影、剧集语义的普通视频集合。
    GenericVideo,
}

impl MediaItemKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Series => "series",
            Self::GenericVideo => "generic-video",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 媒体树中根项目之下的节点类型。
pub enum MediaNodeKind {
    /// 聚合若干剧集集数的季节点。
    Season,
    /// 表示单个剧集集数的叶节点。
    Episode,
    /// 普通视频集合中的单个视频节点。
    GenericVideoItem,
}

impl MediaNodeKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Season => "season",
            Self::Episode => "episode",
            Self::GenericVideoItem => "generic-video-item",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 本地媒体资料相对于预期产物的完整程度。
pub enum LocalStatus {
    /// 所需本地资料均已就绪。
    Complete,
    /// 已有可用资料，但仍缺少部分预期内容。
    Partial,
}

impl LocalStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// NFO 产物在本地处理流程中的结果状态。
pub enum NfoStatus {
    /// 当前流程没有请求生成 NFO。
    NotRequested,
    /// 请求的 NFO 已全部生成。
    Complete,
    /// 仅有部分请求的 NFO 成功生成。
    Partial,
    /// NFO 生成已失败且没有可声明为完整的结果。
    Failed,
}

impl NfoStatus {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequested => "not-requested",
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 目录读模型支持的图片用途。
pub enum ArtworkKind {
    /// 适合竖向展示的海报图。
    Poster,
    /// 适合横向背景展示的剧照或背景图。
    Backdrop,
}

impl ArtworkKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Poster => "poster",
            Self::Backdrop => "backdrop",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 已核对图片引用在本地的可用性。
pub enum ArtworkState {
    /// 引用对应的本地图片可以使用。
    Available,
    /// 已知应有该图片，但本地文件尚不可用。
    Missing,
}

impl ArtworkState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Missing => "missing",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 一个元数据字段是否具有已核对的值。
pub enum MetadataFieldState {
    /// 字段具有可供目录展示的值。
    Present,
    /// 已核对该字段，但没有可用值。
    Missing,
}

impl MetadataFieldState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// 目录元数据的权威来源类别。
pub enum MetadataSourceType {
    /// 值来自 TMDB 提供方结果。
    Tmdb,
    /// 值来自本地 NFO 文件。
    Nfo,
    /// 值由管理员人工确认或覆盖。
    Manual,
    /// 值由 `MediaFlow` 的确定性规则产生。
    System,
}

impl MetadataSourceType {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Tmdb => "tmdb",
            Self::Nfo => "nfo",
            Self::Manual => "manual",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 描述一个已核对元数据值的来源与可选版本信息。
pub struct MetadataSource {
    /// 来源所属的稳定类别。
    pub kind: MetadataSourceType,
    /// 来源内部的实体标识；来源没有实体概念时为 `None`。
    pub id: Option<String>,
    /// 生成该值时使用的来源或映射版本；无法提供时为 `None`。
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 进入正式目录投影前已经核对的单个元数据字段。
pub struct VerifiedMetadataValue {
    /// 与目录字段契约对应的稳定字段名。
    pub field: String,
    /// 已核对的文本值；缺失状态下为 `None`。
    pub value: Option<String>,
    /// 明确区分“有值”和“已确认缺失”。
    pub state: MetadataFieldState,
    /// 值的来源；`Present` 时必须为 `Some`，`Missing` 时为 `None`。
    pub source: Option<MetadataSource>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 进入正式目录投影前已经核对的本地图片引用。
pub struct VerifiedArtworkRef {
    /// 在媒体树内稳定引用该图片的 UUID。
    pub id: Uuid,
    /// 图片承担的展示用途。
    pub kind: ArtworkKind,
    /// 本地文件当前是否可用。
    pub state: ArtworkState,
    /// 能力根内的相对路径；文件缺失时为 `None`。
    pub local_relative_path: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 已核对媒体树中的层级节点，不直接拥有文件内容。
pub struct VerifiedMediaNode {
    /// 节点在本次正式投影中的稳定 UUID。
    pub id: Uuid,
    /// 父节点 UUID；直接隶属根媒体项目时为 `None`。
    pub parent_id: Option<Uuid>,
    /// 节点在媒体层级中的业务角色。
    pub kind: MediaNodeKind,
    /// 已核对且用于展示的节点标题。
    pub title: String,
    /// 同级节点的确定性排序序号。
    pub ordinal: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 将一个可播放版本关联到所属节点及其文件资产。
pub struct VerifiedMediaVersion {
    /// 版本在媒体树内的稳定 UUID。
    pub id: Uuid,
    /// 所属子节点；直接属于根媒体项目时为 `None`。
    pub owner_node_id: Option<Uuid>,
    /// 面向用户的版本标签；没有额外区分时为 `None`。
    pub label: Option<String>,
    /// 构成该版本的已核对文件资产 UUID，顺序保持输入语义。
    pub file_asset_ids: Vec<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 由不可变文件 revision 证明的已核对本地文件资产。
pub struct VerifiedFileAsset {
    /// 资产在正式媒体树中的稳定 UUID。
    pub id: Uuid,
    /// 证明来源内容的不可变文件 revision UUID。
    pub file_revision_id: Uuid,
    /// 进入处理流程时相对于能力根的路径。
    pub source_relative_path: String,
    /// 完成本地处理后相对于能力根的当前路径。
    pub current_relative_path: String,
    /// 已核对文件长度，单位为字节。
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 可原子写入正式目录的已核对媒体树。
pub struct VerifiedMediaTree {
    /// 根媒体项目的稳定 UUID。
    pub id: Uuid,
    /// 根媒体项目的聚合类型。
    pub kind: MediaItemKind,
    /// 已核对的主展示标题。
    pub title: String,
    /// 已核对的发行或首播年份；未知时为 `None`。
    pub year: Option<u16>,
    /// 本地资料相对于预期产物的完整程度。
    pub local_status: LocalStatus,
    /// 已核对字段集合；字段名在同一树中必须唯一。
    pub metadata: Vec<VerifiedMetadataValue>,
    /// 已核对图片引用集合。
    pub artwork_refs: Vec<VerifiedArtworkRef>,
    /// 根项目下的层级节点集合。
    pub nodes: Vec<VerifiedMediaNode>,
    /// 根项目或子节点拥有的媒体版本集合。
    pub versions: Vec<VerifiedMediaVersion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// 生产处理链提交给目录边界的完整、可重放本地结果。
pub struct VerifiedLocalResult {
    /// 标识本次提交的幂等 UUID；相同内容可安全重放。
    pub result_id: Uuid,
    /// 产生该结果且归属于当前账户的处理任务 UUID。
    pub task_id: Uuid,
    /// 正式媒体项目所属媒体库 UUID。
    pub library_id: Uuid,
    /// 待投影的已核对媒体树。
    pub media: VerifiedMediaTree,
    /// 树中所有版本可引用的已核对文件资产。
    pub file_assets: Vec<VerifiedFileAsset>,
    /// 本次本地结果包含的 NFO 处理结论。
    pub nfo_status: NfoStatus,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 目录列表查询可组合的账户内过滤条件。
pub struct MediaItemFilter {
    /// 仅返回指定媒体聚合类型；`None` 不限制类型。
    pub kind: Option<MediaItemKind>,
    /// 仅返回指定媒体库的项目；`None` 不限制媒体库。
    pub library_id: Option<Uuid>,
    /// 仅返回指定本地完整状态；`None` 不限制状态。
    pub local_status: Option<LocalStatus>,
    /// 对规范化标题执行包含匹配的查询文本。
    pub query: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 目录列表中用于代表媒体项目的图片摘要。
pub struct ArtworkRefView {
    /// 图片引用的稳定 UUID。
    pub id: Uuid,
    /// 图片承担的展示用途。
    pub kind: ArtworkKind,
    /// 图片对应的本地文件是否可用。
    pub state: ArtworkState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 媒体项目列表返回的账户隔离摘要。
pub struct MediaItemSummary {
    /// 正式媒体项目的稳定 UUID。
    pub id: Uuid,
    #[serde(rename = "type")]
    /// 序列化为协议字段 `type` 的媒体聚合类型。
    pub kind: MediaItemKind,
    /// 项目所属媒体库 UUID。
    pub library_id: Uuid,
    /// 面向用户的已核对主标题。
    pub title: String,
    /// 发行或首播年份；未知时为 `None`。
    pub year: Option<u16>,
    /// 本地资料当前的完整程度。
    pub local_status: LocalStatus,
    /// 列表优先展示的图片引用；没有可用选择时为 `None`。
    pub artwork_ref: Option<ArtworkRefView>,
    /// 最近一次投影更新时间，采用 UTC RFC3339 文本。
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 媒体详情中携带来源归因的元数据字段。
pub struct MediaMetadataValue {
    /// 与目录契约对应的稳定字段名。
    pub field: String,
    /// 对外展示的字段值；缺失状态下为 `None`。
    pub value: Option<String>,
    /// 区分有值与已确认缺失的状态。
    pub state: MetadataFieldState,
    /// 来源类别；没有可归因来源时为 `None`。
    pub source_type: Option<MetadataSourceType>,
    /// 来源实体标识；来源未提供时为 `None`。
    pub source_id: Option<String>,
    /// 来源或映射版本；不可用时为 `None`。
    pub source_version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 媒体详情中经过路径裁剪的本地文件资产视图。
pub struct MediaFileAsset {
    /// 文件资产在媒体项目内的稳定 UUID。
    pub id: Uuid,
    /// 文件进入处理流程时相对于能力根的路径。
    pub source_relative_path: String,
    /// 文件当前相对于能力根的路径。
    pub current_relative_path: String,
    /// 已核对文件长度，单位为字节。
    pub size_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 一个媒体项目或子节点可用的本地版本视图。
pub struct MediaVersion {
    /// 版本在媒体项目内的稳定 UUID。
    pub id: Uuid,
    /// 区分剪辑、画质等版本的展示标签；未区分时为 `None`。
    pub label: Option<String>,
    /// 构成该版本的本地文件资产。
    pub files: Vec<MediaFileAsset>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 媒体详情中的季、集或普通视频子节点。
pub struct MediaChild {
    /// 子节点的稳定 UUID。
    pub id: Uuid,
    /// 上级节点 UUID；直接隶属根项目时为 `None`。
    pub parent_id: Option<Uuid>,
    #[serde(rename = "type")]
    /// 序列化为协议字段 `type` 的节点角色。
    pub kind: MediaNodeKind,
    /// 面向用户的已核对子节点标题。
    pub title: String,
    /// 同级节点的确定性排序序号。
    pub ordinal: u32,
    /// 直接归属于该节点的媒体版本。
    pub versions: Vec<MediaVersion>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// 单个正式媒体项目的完整账户隔离读模型。
pub struct MediaItemDetail {
    /// 与列表接口一致的项目摘要。
    pub item: MediaItemSummary,
    /// 已核对元数据及其来源归因。
    pub metadata: Vec<MediaMetadataValue>,
    /// 直接归属于根项目的媒体版本。
    pub versions: Vec<MediaVersion>,
    /// 根项目下的层级节点及其版本。
    pub children: Vec<MediaChild>,
    /// 与该项目关联的 NFO 处理状态。
    pub nfo_status: NfoStatus,
    /// 参与生成或最近更新该投影的处理任务 UUID。
    pub related_task_ids: Vec<Uuid>,
}
