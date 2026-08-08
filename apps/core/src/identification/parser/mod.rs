mod auxiliary;
mod episode;
mod movie;
mod normalize;

use std::sync::LazyLock;

use regex::Regex;

use crate::identification::model::{ExternalIdHint, MediaKind, ParsedIdentityHint};

const MAX_PATH_BYTES: usize = 4096;
const MAX_TITLE_CHARS: usize = 512;
const MAX_EXTERNAL_IDS: usize = 8;
/// 写入解析证据、用于区分文件名规则语义的稳定版本。
pub const PARSER_VERSION: &str = "filename-v1";

static EXTERNAL_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\[\{(]\s*(tmdb|imdb|tvdb)(?:id)?\s*[-:=]\s*([a-z0-9]+)\s*[\]\})]")
        .expect("static provider ID regex")
});
static RELEASE_GROUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)-[a-z0-9][a-z0-9._-]{1,30}$").expect("static release group regex")
});

#[derive(Clone, Debug, Default)]
/// 使用预编译规则的确定性 Unicode 文件名解析器。
pub struct FilenameParser {
    _private: (),
}

impl FilenameParser {
    /// 把安全相对展示路径解析为有界本地身份线索。
    ///
    /// # Errors
    ///
    /// 路径为空、不安全或过长，标题为空或过长，剧集范围/日期无效，或显式外部 ID 过多时
    /// 返回 [`FilenameParseError`]。
    pub fn parse(&self, relative_path: &str) -> Result<ParsedIdentityHint, FilenameParseError> {
        validate_path(relative_path)?;
        let original = relative_path.to_owned();
        let filename = relative_path
            .rsplit('/')
            .next()
            .ok_or(FilenameParseError::InvalidPath)?;
        let stem = strip_media_extension(filename);
        if stem.is_empty() {
            return Err(FilenameParseError::MissingTitle);
        }
        let mut working = normalize::nfkc_lower(stem);
        let external_ids = take_external_ids(&mut working)?;
        let episode =
            episode::take_episode(&mut working).map_err(|()| FilenameParseError::InvalidEpisode)?;
        let year = movie::take_year(&mut working);
        let (version_tags, removed_tokens) = auxiliary::extract_version_tags(&working);
        if let Some(group) = RELEASE_GROUP.find(&working) {
            working.replace_range(group.range(), " ");
        }
        let folded = normalize::fold_separators(&working);
        let mut title = folded
            .split_whitespace()
            .filter(|token| !removed_tokens.contains(*token))
            .collect::<Vec<_>>()
            .join(" ");
        if title.is_empty() {
            title =
                fallback_directory_title(relative_path).ok_or(FilenameParseError::MissingTitle)?;
        }
        if title.chars().count() > MAX_TITLE_CHARS {
            return Err(FilenameParseError::TitleTooLong);
        }
        let (media_kind, season, episodes, air_date) =
            episode.map_or((MediaKind::Movie, None, Vec::new(), None), |episode| {
                (
                    MediaKind::Episode,
                    episode.season,
                    episode.episodes,
                    episode.air_date,
                )
            });
        Ok(ParsedIdentityHint {
            original,
            normalized_title: title,
            media_kind,
            year,
            season,
            episodes,
            air_date,
            external_ids,
            version_tags,
            parser_version: PARSER_VERSION,
        })
    }
}

fn validate_path(path: &str) -> Result<(), FilenameParseError> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        Err(FilenameParseError::InvalidPath)
    } else {
        Ok(())
    }
}

fn strip_media_extension(filename: &str) -> &str {
    let Some((stem, extension)) = filename.rsplit_once('.') else {
        return filename;
    };
    if ["mkv", "mp4", "m4v", "avi", "mov", "ts", "m2ts", "wmv"]
        .contains(&extension.to_ascii_lowercase().as_str())
    {
        stem
    } else {
        filename
    }
}

fn take_external_ids(value: &mut String) -> Result<Vec<ExternalIdHint>, FilenameParseError> {
    let mut ids = Vec::new();
    for captures in EXTERNAL_ID.captures_iter(value) {
        let provider = captures
            .get(1)
            .ok_or(FilenameParseError::InvalidExternalId)?;
        let id = captures
            .get(2)
            .ok_or(FilenameParseError::InvalidExternalId)?;
        if id.as_str().len() > 64 {
            return Err(FilenameParseError::InvalidExternalId);
        }
        ids.push(ExternalIdHint {
            provider: provider.as_str().to_ascii_lowercase(),
            value: id.as_str().to_ascii_lowercase(),
            is_default: false,
        });
        if ids.len() > MAX_EXTERNAL_IDS {
            return Err(FilenameParseError::TooManyExternalIds);
        }
    }
    *value = EXTERNAL_ID.replace_all(value, " ").into_owned();
    Ok(ids)
}

fn fallback_directory_title(path: &str) -> Option<String> {
    path.rsplit('/')
        .skip(1)
        .find(|component| {
            let lower = component.to_ascii_lowercase();
            !lower.starts_with("season ") && !lower.starts_with("series ")
        })
        .map(normalize::nfkc_lower)
        .map(|value| normalize::fold_separators(&value))
        .filter(|value| !value.is_empty())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// 相对媒体路径无法安全转换为有界身份线索时的错误。
pub enum FilenameParseError {
    #[error("relative media path is invalid")]
    /// 路径为空、过长、为绝对路径、含控制字符或不安全分量。
    InvalidPath,
    #[error("media title is missing")]
    /// 文件名及可用父目录均无法提供作品标题。
    MissingTitle,
    #[error("normalized media title is too long")]
    /// 规范化标题超过解析边界允许的字符数。
    TitleTooLong,
    #[error("episode marker is invalid")]
    /// 剧集标记、范围或按日期播出标记无法安全解析。
    InvalidEpisode,
    #[error("external ID marker is invalid")]
    /// 外部 ID 标记缺少命名空间、值或超过单值长度限制。
    InvalidExternalId,
    #[error("too many external ID markers")]
    /// 路径携带的显式外部 ID 数量超过有界上限。
    TooManyExternalIds,
}
