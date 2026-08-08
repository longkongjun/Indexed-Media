use std::ffi::{OsStr, OsString};

use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use sha2::{Digest, Sha256};

use crate::discovery::capability::{CapabilityFileError, CapabilityFs};
use crate::discovery::model::{DirectoryCapability, FsBoundaryError};
use crate::identification::model::ExternalIdHint;
use crate::identification::model::MediaKind;

/// 单个 NFO 文档允许的最大字节数。
pub const MAX_NFO_BYTES: usize = 1024 * 1024;
const MAX_XML_DEPTH: usize = 32;
const MAX_FIELD_BYTES: usize = 64 * 1024;
const MAX_EXTERNAL_IDS: usize = 16;
const MAX_STACKED_EPISODES: usize = 100;

/// 选中某个 NFO 文档的发现规则。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NfoSource {
    /// 与媒体文件共享原始文件名 stem 的电影 NFO。
    MovieSameName,
    /// 约定的 `movie.nfo` 回退文件。
    MovieDirectory,
    /// 约定的剧集级 `tvshow.nfo` 文档。
    TvShow,
    /// 与媒体文件共享原始文件名 stem 的单集 NFO。
    EpisodeSameName,
    /// 使用 `-SxxEyy` 后缀的 Kodi v22 单集 NFO。
    EpisodeSeparate,
}

/// 通过目录能力读取的字节及其确定性来源规则。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocatedNfo {
    /// 选中该文档的查找规则。
    pub source: NfoSource,
    /// 完整且已限制大小的文件内容。
    pub bytes: Vec<u8>,
}

/// 仅通过能力访问的 Kodi NFO 查找器，绝不重建主机绝对路径。
pub struct NfoLocator<'a> {
    fs: &'a dyn CapabilityFs,
}

impl<'a> NfoLocator<'a> {
    /// 将查找器绑定到签发目录能力的文件系统适配器。
    #[must_use]
    pub const fn new(fs: &'a dyn CapabilityFs) -> Self {
        Self { fs }
    }

    /// 按确定性优先级读取适用的电影或剧集 NFO 文档。
    ///
    /// 电影查找优先使用同名 NFO，仅以 `movie.nfo` 回退。剧集查找可先返回
    /// `series_directory` 中的 `tvshow.nfo`，再返回单集同名 NFO。若高优先级候选存在但不安全，
    /// 会失败关闭而不会继续尝试低优先级文件。
    ///
    /// # Errors
    ///
    /// 媒体路径分量无效、能力过期或来自其他适配器、目标为符号链接或非普通文件、读取失败，
    /// 或超过大小上限时返回 [`NfoLoadError`]。
    pub fn read_for_media(
        &self,
        media_directory: &DirectoryCapability,
        media_name: &OsStr,
        media_kind: MediaKind,
        series_directory: Option<&DirectoryCapability>,
    ) -> Result<Vec<LocatedNfo>, NfoLoadError> {
        let same_name = same_name_nfo(media_name)?;
        match media_kind {
            MediaKind::Movie => {
                let entries = self.fs.read_directory(media_directory)?;
                if contains_name(&entries, &same_name) {
                    return self
                        .read(media_directory, &same_name, NfoSource::MovieSameName)
                        .map(|nfo| vec![nfo]);
                }
                let fallback = OsStr::new("movie.nfo");
                if contains_name(&entries, fallback) {
                    self.read(media_directory, fallback, NfoSource::MovieDirectory)
                        .map(|nfo| vec![nfo])
                } else {
                    Ok(Vec::new())
                }
            }
            MediaKind::Episode => {
                self.read_for_episode_references(media_directory, media_name, series_directory, &[])
            }
        }
    }

    /// 为显式集数节点读取剧集元数据以及 Kodi v21/v22 单集 NFO 变体。
    ///
    /// 同名 NFO 优先，因为其中可能包含 Kodi v21 的有序堆叠集文档。文件不存在时，每个引用
    /// 按 Kodi v22 的 `media-stem-SxxEyy.nfo` 约定映射，且不会把原始媒体 stem 转为 UTF-8。
    ///
    /// # Errors
    ///
    /// 引用无效或重复，或任何能力相对读取失败时返回 [`NfoLoadError`]；最多接受 100 个引用。
    pub fn read_for_episode_references(
        &self,
        media_directory: &DirectoryCapability,
        media_name: &OsStr,
        series_directory: Option<&DirectoryCapability>,
        episode_references: &[(u16, u16)],
    ) -> Result<Vec<LocatedNfo>, NfoLoadError> {
        validate_episode_references(episode_references)?;
        let mut located = Vec::with_capacity(episode_references.len().saturating_add(1));
        if let Some(series_directory) = series_directory {
            let series_entries = self.fs.read_directory(series_directory)?;
            let tv_show = OsStr::new("tvshow.nfo");
            if contains_name(&series_entries, tv_show) {
                located.push(self.read(series_directory, tv_show, NfoSource::TvShow)?);
            }
        }
        let entries = self.fs.read_directory(media_directory)?;
        let same_name = same_name_nfo(media_name)?;
        if contains_name(&entries, &same_name) {
            located.push(self.read(media_directory, &same_name, NfoSource::EpisodeSameName)?);
            return Ok(located);
        }
        for &(season, episode) in episode_references {
            let candidate = separate_episode_nfo(media_name, season, episode)?;
            if contains_name(&entries, &candidate) {
                located.push(self.read(media_directory, &candidate, NfoSource::EpisodeSeparate)?);
            }
        }
        Ok(located)
    }

    fn read(
        &self,
        directory: &DirectoryCapability,
        name: &OsStr,
        source: NfoSource,
    ) -> Result<LocatedNfo, NfoLoadError> {
        Ok(LocatedNfo {
            source,
            bytes: self
                .fs
                .read_bounded_relative_file(directory, name, MAX_NFO_BYTES)?,
        })
    }
}

fn contains_name(entries: &[crate::discovery::model::DirectoryEntry], name: &OsStr) -> bool {
    entries.iter().any(|entry| entry.name() == name)
}

fn same_name_nfo(media_name: &OsStr) -> Result<OsString, NfoLoadError> {
    let bytes = media_stem(media_name)?;
    let mut nfo = bytes.to_vec();
    nfo.extend_from_slice(b".nfo");
    os_string_from_bytes(nfo)
}

fn separate_episode_nfo(
    media_name: &OsStr,
    season: u16,
    episode: u16,
) -> Result<OsString, NfoLoadError> {
    let mut nfo = media_stem(media_name)?.to_vec();
    nfo.extend_from_slice(format!("-S{season:02}E{episode:02}.nfo").as_bytes());
    os_string_from_bytes(nfo)
}

fn media_stem(media_name: &OsStr) -> Result<&[u8], NfoLoadError> {
    let bytes = os_str_bytes(media_name)?;
    if bytes.is_empty()
        || matches!(bytes, b"." | b"..")
        || bytes.contains(&b'/')
        || bytes.contains(&b'\\')
        || bytes.contains(&0)
    {
        return Err(FsBoundaryError::PathInvalid.into());
    }
    let dot = bytes
        .iter()
        .rposition(|byte| *byte == b'.')
        .filter(|index| *index > 0)
        .ok_or(FsBoundaryError::PathInvalid)?;
    Ok(&bytes[..dot])
}

fn validate_episode_references(references: &[(u16, u16)]) -> Result<(), NfoLoadError> {
    if references.len() > 100
        || references
            .iter()
            .any(|(season, episode)| *season > 999 || *episode == 0 || *episode > 999)
    {
        return Err(FsBoundaryError::PathInvalid.into());
    }
    let mut unique = std::collections::BTreeSet::new();
    if references
        .iter()
        .any(|reference| !unique.insert(*reference))
    {
        return Err(FsBoundaryError::PathInvalid.into());
    }
    Ok(())
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn os_str_bytes(value: &OsStr) -> Result<&[u8], NfoLoadError> {
    use std::os::unix::ffi::OsStrExt;
    Ok(value.as_bytes())
}

#[cfg(not(unix))]
fn os_str_bytes(value: &OsStr) -> Result<&[u8], NfoLoadError> {
    value
        .to_str()
        .map(str::as_bytes)
        .ok_or_else(|| FsBoundaryError::PathInvalid.into())
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn os_string_from_bytes(value: Vec<u8>) -> Result<OsString, NfoLoadError> {
    use std::os::unix::ffi::OsStringExt;
    Ok(OsString::from_vec(value))
}

#[cfg(not(unix))]
fn os_string_from_bytes(value: Vec<u8>) -> Result<OsString, NfoLoadError> {
    String::from_utf8(value)
        .map(OsString::from)
        .map_err(|_| FsBoundaryError::PathInvalid.into())
}

/// 不泄露主机绝对路径的稳定查找失败类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NfoLoadError {
    /// 能力文件系统拒绝或无法完成操作。
    #[error(transparent)]
    Boundary(#[from] FsBoundaryError),
    /// 选中的 NFO 超过 [`MAX_NFO_BYTES`]。
    #[error("NFO exceeds the maximum size")]
    TooLarge,
}

impl From<CapabilityFileError> for NfoLoadError {
    fn from(error: CapabilityFileError) -> Self {
        match error {
            CapabilityFileError::Boundary(error) => Self::Boundary(error),
            CapabilityFileError::TooLarge => Self::TooLarge,
        }
    }
}

/// 支持的 Kodi NFO 文档根类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NfoKind {
    /// `movie` 文档。
    Movie,
    /// `tvshow` 文档。
    TvShow,
    /// `episodedetails` 文档。
    Episode,
}

/// 从一个 NFO 文档提取的有界身份线索。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NfoDocument {
    /// 根文档类型。
    pub kind: NfoKind,
    /// 文档中存在的展示标题。
    pub title: Option<String>,
    /// 文档中存在的原始语言标题。
    pub original_title: Option<String>,
    /// 单集文档中存在的剧集标题。
    pub show_title: Option<String>,
    /// 语法有效的四位发行年份。
    pub year: Option<u16>,
    /// 单集文档中的季号。
    pub season: Option<u16>,
    /// 单集文档中的集号。
    pub episode: Option<u16>,
    /// `YYYY-MM-DD` 形式的单集播出日期。
    pub aired: Option<String>,
    /// `YYYY-MM-DD` 形式的作品首映日期。
    pub premiered: Option<String>,
    /// 显式提供方标识，包括尚未消解的默认提供方冲突。
    pub external_ids: Vec<ExternalIdHint>,
}

impl NfoDocument {
    fn new(kind: NfoKind) -> Self {
        Self {
            kind,
            title: None,
            original_title: None,
            show_title: None,
            year: None,
            season: None,
            episode: None,
            aired: None,
            premiered: None,
            external_ids: Vec::new(),
        }
    }
}

/// 一个物理 NFO 文件的解析结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedNfo {
    /// 完整有界源文件的 SHA-256。
    pub document_hash: [u8; 32],
    /// 文件内所有逻辑文档共享的根类型。
    pub root_kind: NfoKind,
    /// 通常只有一个文档；Kodi v21 及更早版本可包含有序堆叠单集文档。
    pub documents: Vec<NfoDocument>,
}

/// 无状态、有界的 Kodi NFO 解析器。
#[derive(Clone, Copy, Debug, Default)]
pub struct NfoParser {
    _private: (),
}

impl NfoParser {
    /// 解析一个 UTF-8 NFO，不解析外部实体或主动 XML 构造。
    ///
    /// # Errors
    ///
    /// 文档超出边界、格式错误、包含主动 XML 构造或根类型不受支持时，返回稳定的
    /// [`NfoParseError`]。
    pub fn parse(&self, bytes: &[u8]) -> Result<ParsedNfo, NfoParseError> {
        if bytes.len() > MAX_NFO_BYTES {
            return Err(NfoParseError::TooLarge);
        }
        std::str::from_utf8(bytes).map_err(|_| NfoParseError::InvalidUtf8)?;
        let document_hash = Sha256::digest(bytes).into();

        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(true);
        reader.config_mut().check_end_names = true;
        let mut stack = Vec::<Vec<u8>>::new();
        let mut documents = Vec::<NfoDocument>::new();
        let mut document = None::<NfoDocument>;
        let mut active = None::<ActiveField>;
        let mut declaration_seen = false;
        let mut xml_version = XmlVersion::Implicit1_0;

        loop {
            let event = reader.read_event().map_err(|_| NfoParseError::InvalidXml)?;
            match event {
                Event::Start(start) => {
                    reject_xinclude(&start)?;
                    if active.is_some() {
                        return Err(NfoParseError::InvalidXml);
                    }
                    if stack.is_empty() {
                        let kind = parse_root(start.name().as_ref())?;
                        validate_next_root(&documents, kind)?;
                        document = Some(NfoDocument::new(kind));
                    } else if stack.len() == 1 {
                        active = ActiveField::from_start(&start, xml_version, reader.decoder())?;
                    }
                    stack.push(start.name().as_ref().to_vec());
                    if stack.len() > MAX_XML_DEPTH {
                        return Err(NfoParseError::TooDeep);
                    }
                }
                Event::Empty(start) => {
                    reject_xinclude(&start)?;
                    if stack.is_empty() {
                        let kind = parse_root(start.name().as_ref())?;
                        validate_next_root(&documents, kind)?;
                        documents.push(NfoDocument::new(kind));
                    } else if stack.len() == 1
                        && let Some(field) =
                            ActiveField::from_start(&start, xml_version, reader.decoder())?
                    {
                        apply_field(document.as_mut().ok_or(NfoParseError::InvalidXml)?, field)?;
                    }
                }
                Event::End(end) => {
                    if stack.len() == 2
                        && let Some(field) = active.take()
                    {
                        apply_field(document.as_mut().ok_or(NfoParseError::InvalidXml)?, field)?;
                    }
                    stack.pop().ok_or(NfoParseError::InvalidXml)?;
                    if stack.is_empty() {
                        documents.push(document.take().ok_or(NfoParseError::InvalidXml)?);
                    }
                    if end.name().as_ref().is_empty() {
                        return Err(NfoParseError::InvalidXml);
                    }
                }
                Event::Text(text) => {
                    let content = text
                        .xml_content(xml_version)
                        .map_err(|_| NfoParseError::InvalidUtf8)?;
                    if let Some(field) = active.as_mut() {
                        field.push(&content)?;
                    } else if stack.is_empty() && !content.trim().is_empty() {
                        return Err(NfoParseError::InvalidXml);
                    }
                }
                Event::CData(text) => {
                    let content = text
                        .xml_content(xml_version)
                        .map_err(|_| NfoParseError::InvalidUtf8)?;
                    if let Some(field) = active.as_mut() {
                        field.push(&content)?;
                    }
                }
                Event::PI(_) | Event::DocType(_) | Event::GeneralRef(_) => {
                    return Err(NfoParseError::UnsafeXml);
                }
                Event::Decl(declaration) => {
                    if declaration_seen || !documents.is_empty() || !stack.is_empty() {
                        return Err(NfoParseError::InvalidXml);
                    }
                    xml_version = declaration_version(&declaration)?;
                    declaration_seen = true;
                }
                Event::Comment(_) => {}
                Event::Eof => break,
            }
        }
        finish_parse(
            document_hash,
            documents,
            !stack.is_empty() || document.is_some(),
        )
    }
}

fn finish_parse(
    document_hash: [u8; 32],
    documents: Vec<NfoDocument>,
    unfinished_document: bool,
) -> Result<ParsedNfo, NfoParseError> {
    if unfinished_document {
        return Err(NfoParseError::InvalidXml);
    }
    let root_kind = documents
        .first()
        .map(|document| document.kind)
        .ok_or(NfoParseError::UnsupportedRoot)?;
    Ok(ParsedNfo {
        document_hash,
        root_kind,
        documents,
    })
}

fn declaration_version(declaration: &BytesDecl<'_>) -> Result<XmlVersion, NfoParseError> {
    match declaration
        .version()
        .map_err(|_| NfoParseError::InvalidXml)?
        .as_ref()
    {
        b"1.0" => Ok(XmlVersion::Explicit1_0),
        b"1.1" => Ok(XmlVersion::Explicit1_1),
        _ => Err(NfoParseError::InvalidXml),
    }
}

fn validate_next_root(documents: &[NfoDocument], next_kind: NfoKind) -> Result<(), NfoParseError> {
    if documents.len() >= MAX_STACKED_EPISODES {
        return Err(NfoParseError::TooManyDocuments);
    }
    if let Some(first) = documents.first()
        && (first.kind != NfoKind::Episode || next_kind != NfoKind::Episode)
    {
        return Err(NfoParseError::InvalidXml);
    }
    Ok(())
}

fn reject_xinclude(start: &BytesStart<'_>) -> Result<(), NfoParseError> {
    if start.local_name().as_ref().eq_ignore_ascii_case(b"include")
        && start.name().as_ref().contains(&b':')
    {
        Err(NfoParseError::UnsafeXml)
    } else {
        Ok(())
    }
}

fn parse_root(name: &[u8]) -> Result<NfoKind, NfoParseError> {
    if name.eq_ignore_ascii_case(b"movie") {
        Ok(NfoKind::Movie)
    } else if name.eq_ignore_ascii_case(b"tvshow") {
        Ok(NfoKind::TvShow)
    } else if name.eq_ignore_ascii_case(b"episodedetails") {
        Ok(NfoKind::Episode)
    } else {
        Err(NfoParseError::UnsupportedRoot)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FieldKind {
    Title,
    OriginalTitle,
    ShowTitle,
    Year,
    Season,
    Episode,
    Aired,
    Premiered,
    UniqueId,
    TmdbId,
    ImdbId,
}

struct ActiveField {
    kind: FieldKind,
    value: String,
    provider: Option<String>,
    is_default: bool,
}

impl ActiveField {
    fn from_start(
        start: &BytesStart<'_>,
        xml_version: XmlVersion,
        decoder: quick_xml::encoding::Decoder,
    ) -> Result<Option<Self>, NfoParseError> {
        let Some(kind) = field_kind(start.name().as_ref()) else {
            return Ok(None);
        };
        let mut provider = None;
        let mut is_default = false;
        if kind == FieldKind::UniqueId {
            for attribute in start.attributes() {
                let attribute = attribute.map_err(|_| NfoParseError::InvalidXml)?;
                let value = attribute
                    .decoded_and_normalized_value(xml_version, decoder)
                    .map_err(|_| NfoParseError::InvalidXml)?;
                match attribute.key.as_ref() {
                    b"type" => provider = Some(value.into_owned().to_ascii_lowercase()),
                    b"default" => is_default = matches!(value.as_ref(), "true" | "1"),
                    _ => {}
                }
            }
        }
        Ok(Some(Self {
            kind,
            value: String::new(),
            provider,
            is_default,
        }))
    }

    fn push(&mut self, value: &str) -> Result<(), NfoParseError> {
        if self.value.len().saturating_add(value.len()) > MAX_FIELD_BYTES {
            return Err(NfoParseError::FieldTooLarge);
        }
        self.value.push_str(value);
        Ok(())
    }
}

fn field_kind(name: &[u8]) -> Option<FieldKind> {
    match name {
        name if name.eq_ignore_ascii_case(b"title") => Some(FieldKind::Title),
        name if name.eq_ignore_ascii_case(b"originaltitle") => Some(FieldKind::OriginalTitle),
        name if name.eq_ignore_ascii_case(b"showtitle") => Some(FieldKind::ShowTitle),
        name if name.eq_ignore_ascii_case(b"year") => Some(FieldKind::Year),
        name if name.eq_ignore_ascii_case(b"season") => Some(FieldKind::Season),
        name if name.eq_ignore_ascii_case(b"episode") => Some(FieldKind::Episode),
        name if name.eq_ignore_ascii_case(b"aired") => Some(FieldKind::Aired),
        name if name.eq_ignore_ascii_case(b"premiered") => Some(FieldKind::Premiered),
        name if name.eq_ignore_ascii_case(b"uniqueid") => Some(FieldKind::UniqueId),
        name if name.eq_ignore_ascii_case(b"tmdbid") => Some(FieldKind::TmdbId),
        name if name.eq_ignore_ascii_case(b"imdbid") => Some(FieldKind::ImdbId),
        _ => None,
    }
}

fn apply_field(document: &mut NfoDocument, field: ActiveField) -> Result<(), NfoParseError> {
    let value = field.value.trim();
    if value.is_empty() {
        return Ok(());
    }
    match field.kind {
        FieldKind::Title => set_text(&mut document.title, value),
        FieldKind::OriginalTitle => set_text(&mut document.original_title, value),
        FieldKind::ShowTitle => set_text(&mut document.show_title, value),
        FieldKind::Year => document.year = parse_bounded_number(value, 1888, 9999),
        FieldKind::Season => document.season = parse_bounded_number(value, 0, 9999),
        FieldKind::Episode => document.episode = parse_bounded_number(value, 0, 9999),
        FieldKind::Aired => document.aired = parse_date(value),
        FieldKind::Premiered => document.premiered = parse_date(value),
        FieldKind::UniqueId => {
            if let Some(provider) = field.provider {
                push_external_id(document, &provider, value, field.is_default)?;
            }
        }
        FieldKind::TmdbId => push_external_id(document, "tmdb", value, false)?,
        FieldKind::ImdbId => push_external_id(document, "imdb", value, false)?,
    }
    Ok(())
}

fn set_text(target: &mut Option<String>, value: &str) {
    if target.is_none() {
        *target = Some(value.to_owned());
    }
}

fn parse_bounded_number(value: &str, minimum: u16, maximum: u16) -> Option<u16> {
    value
        .parse::<u16>()
        .ok()
        .filter(|number| (*number >= minimum) && (*number <= maximum))
}

fn parse_date(value: &str) -> Option<String> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn push_external_id(
    document: &mut NfoDocument,
    provider: &str,
    value: &str,
    is_default: bool,
) -> Result<(), NfoParseError> {
    if document.external_ids.len() >= MAX_EXTERNAL_IDS {
        return Err(NfoParseError::TooManyExternalIds);
    }
    if provider.is_empty()
        || provider.len() > 32
        || !provider
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || value.len() > 64
        || !value.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(NfoParseError::InvalidExternalId);
    }
    document.external_ids.push(ExternalIdHint {
        provider: provider.to_owned(),
        value: value.to_owned(),
        is_default,
    });
    Ok(())
}

/// 适合诊断且不含主机细节的稳定 NFO 拒绝类别。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NfoParseError {
    /// 输入超过 [`MAX_NFO_BYTES`]。
    #[error("NFO exceeds the maximum size")]
    TooLarge,
    /// 输入不是有效 UTF-8。
    #[error("NFO is not valid UTF-8")]
    InvalidUtf8,
    /// XML 语法或结构无效。
    #[error("NFO XML is invalid")]
    InvalidXml,
    /// 文档包含 DTD、processing instruction、entity 或 `XInclude` 语法。
    #[error("NFO contains an unsafe XML construct")]
    UnsafeXml,
    /// 根元素不是 `movie`、`tvshow` 或 `episodedetails`。
    #[error("NFO root is unsupported")]
    UnsupportedRoot,
    /// 元素嵌套超过解析器上限。
    #[error("NFO nesting is too deep")]
    TooDeep,
    /// 某个已识别字段超过解析器上限。
    #[error("NFO field is too large")]
    FieldTooLarge,
    /// 提供方标识数量超过有界证据模型上限。
    #[error("NFO has too many external IDs")]
    TooManyExternalIds,
    /// 堆叠单集文档数量超过有界模型上限。
    #[error("NFO has too many stacked episode documents")]
    TooManyDocuments,
    /// 提供方名称或标识值不符合已接受的有界语法。
    #[error("NFO external ID is invalid")]
    InvalidExternalId,
}
