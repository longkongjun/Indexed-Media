use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use sha2::{Digest as _, Sha256};

use super::model::{ConfirmedNfoInput, ConfirmedNfoMedia, NfoConfidence};

/// 新生成 NFO 的硬字节上限。
pub const MAX_NFO_BYTES: usize = 1024 * 1024;
const MAX_TITLE_CHARS: usize = 512;
const MAX_PLOT_BYTES: usize = MAX_NFO_BYTES;
const MAX_PROVIDER_ID_BYTES: usize = 64;
const MAX_EPISODES: usize = 32;

/// 对已有或缺失 NFO 作出的无副作用结论。
#[derive(Clone, Eq, PartialEq)]
pub enum NfoDecision {
    /// 已有内容必须逐字节保留，只记录其摘要。
    Preserve {
        /// 已有原始字节的 SHA-256。
        sha256: [u8; 32],
    },
    /// 生成一个固定文件名和有界 XML 文档。
    Generate {
        /// planner 已确定的同目录 NFO 文件名。
        file_name: String,
        /// 固定 serializer 的完整 UTF-8 XML 字节。
        bytes: Vec<u8>,
    },
    /// 输入未确认，禁止写入。
    Skip,
}

impl std::fmt::Debug for NfoDecision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Preserve { .. } => formatter.write_str("NfoDecision::Preserve([REDACTED])"),
            Self::Generate { bytes, .. } => formatter
                .debug_struct("NfoDecision::Generate")
                .field("file_name", &"[REDACTED]")
                .field("byte_len", &bytes.len())
                .finish(),
            Self::Skip => formatter.write_str("NfoDecision::Skip"),
        }
    }
}

/// 固定 NFO 生成边界的稳定错误；不会携带输入正文。
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NfoGenerationError {
    /// 文件名、核心字段或 provider ID 越界或不符合固定语法。
    #[error("confirmed NFO input is invalid")]
    InvalidInput,
    /// 最终 XML 超过 1 MiB。
    #[error("generated NFO exceeds the byte limit")]
    TooLarge,
    /// 固定 XML writer 未能完成内存序列化。
    #[error("NFO serialization failed")]
    Serialization,
}

/// 只接受已映射领域字段的确定性固定 serializer。
#[derive(Clone, Copy, Debug, Default)]
pub struct NfoGenerator;

impl NfoGenerator {
    /// 保留已有字节，或为确认输入生成缺失 NFO。
    ///
    /// 已有内容优先于置信度判断，确保任何现存用户文件都不会因解析或身份状态而被改写。
    ///
    /// # Errors
    ///
    /// 确认输入不符合边界或生成结果超过 1 MiB 时返回稳定错误。
    pub fn decide(
        &self,
        input: &ConfirmedNfoInput,
        existing: Option<&[u8]>,
    ) -> Result<NfoDecision, NfoGenerationError> {
        if let Some(existing) = existing {
            return Ok(NfoDecision::Preserve {
                sha256: Sha256::digest(existing).into(),
            });
        }
        if input.confidence != NfoConfidence::Confirmed {
            return Ok(NfoDecision::Skip);
        }
        validate(input)?;
        let bytes = serialize(&input.media, input.provider_id.as_ref())?;
        if bytes.len() > MAX_NFO_BYTES {
            return Err(NfoGenerationError::TooLarge);
        }
        Ok(NfoDecision::Generate {
            file_name: input.file_name.clone(),
            bytes,
        })
    }
}

fn validate(input: &ConfirmedNfoInput) -> Result<(), NfoGenerationError> {
    if !valid_file_name(&input.file_name) {
        return Err(NfoGenerationError::InvalidInput);
    }
    if let Some(provider) = &input.provider_id
        && (!(1..=MAX_PROVIDER_ID_BYTES).contains(&provider.value.len())
            || !provider
                .value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
    {
        return Err(NfoGenerationError::InvalidInput);
    }
    match &input.media {
        ConfirmedNfoMedia::Movie {
            title,
            original_title,
            plot,
            ..
        } => validate_common(title, original_title.as_deref(), plot.as_deref()),
        ConfirmedNfoMedia::Episode {
            title,
            original_title,
            episodes,
            plot,
            ..
        } => {
            validate_common(title, original_title.as_deref(), plot.as_deref())?;
            let unique = episodes
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            if unique.is_empty() || unique.len() > MAX_EPISODES {
                return Err(NfoGenerationError::InvalidInput);
            }
            Ok(())
        }
        ConfirmedNfoMedia::GenericVideo { title, sequence } => {
            if input.provider_id.is_some() || *sequence == 0 {
                return Err(NfoGenerationError::InvalidInput);
            }
            validate_text(title, MAX_TITLE_CHARS)
        }
    }
}

fn validate_common(
    title: &str,
    original_title: Option<&str>,
    plot: Option<&str>,
) -> Result<(), NfoGenerationError> {
    validate_text(title, MAX_TITLE_CHARS)?;
    if let Some(value) = original_title {
        validate_text(value, MAX_TITLE_CHARS)?;
    }
    if let Some(value) = plot
        && (value.len() > MAX_PLOT_BYTES || value.contains('\0'))
    {
        return Err(NfoGenerationError::InvalidInput);
    }
    Ok(())
}

fn validate_text(value: &str, max_chars: usize) -> Result<(), NfoGenerationError> {
    if value.is_empty() || value.chars().count() > max_chars || value.contains('\0') {
        return Err(NfoGenerationError::InvalidInput);
    }
    Ok(())
}

fn valid_file_name(value: &str) -> bool {
    std::path::Path::new(value)
        .extension()
        .is_some_and(|extension| extension == "nfo")
        && (1..=255).contains(&value.len())
        && value != ".nfo"
        && !value.contains(['/', '\\', '\0'])
        && !value.chars().any(char::is_control)
}

fn serialize(
    media: &ConfirmedNfoMedia,
    provider_id: Option<&super::model::ConfirmedProviderId>,
) -> Result<Vec<u8>, NfoGenerationError> {
    let root = match media {
        ConfirmedNfoMedia::Movie { .. } => "movie",
        ConfirmedNfoMedia::Episode { .. } => "episodedetails",
        ConfirmedNfoMedia::GenericVideo { .. } => "video",
    };
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    write_event(
        &mut writer,
        Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), Some("yes"))),
    )?;
    write_event(&mut writer, Event::Start(BytesStart::new(root)))?;
    match media {
        ConfirmedNfoMedia::Movie {
            title,
            original_title,
            year,
            plot,
        } => {
            text_element(&mut writer, "title", title)?;
            optional_text_element(&mut writer, "originaltitle", original_title.as_deref())?;
            optional_number_element(&mut writer, "year", *year)?;
            optional_text_element(&mut writer, "plot", plot.as_deref())?;
        }
        ConfirmedNfoMedia::Episode {
            title,
            original_title,
            year,
            season,
            episodes,
            plot,
        } => {
            text_element(&mut writer, "title", title)?;
            optional_text_element(&mut writer, "originaltitle", original_title.as_deref())?;
            optional_number_element(&mut writer, "year", *year)?;
            number_element(&mut writer, "season", *season)?;
            for episode in episodes
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
            {
                number_element(&mut writer, "episode", episode)?;
            }
            optional_text_element(&mut writer, "plot", plot.as_deref())?;
        }
        ConfirmedNfoMedia::GenericVideo { title, sequence } => {
            text_element(&mut writer, "title", title)?;
            number_element(&mut writer, "sequence", *sequence)?;
        }
    }
    if let Some(provider) = provider_id {
        let mut element = BytesStart::new("uniqueid");
        element.push_attribute(("type", provider.provider.as_str()));
        element.push_attribute(("default", "true"));
        write_event(&mut writer, Event::Start(element))?;
        write_event(&mut writer, Event::Text(BytesText::new(&provider.value)))?;
        write_event(&mut writer, Event::End(BytesEnd::new("uniqueid")))?;
    }
    write_event(&mut writer, Event::End(BytesEnd::new(root)))?;
    let mut bytes = writer.into_inner();
    bytes.push(b'\n');
    Ok(bytes)
}

fn optional_text_element(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    value: Option<&str>,
) -> Result<(), NfoGenerationError> {
    if let Some(value) = value {
        text_element(writer, name, value)?;
    }
    Ok(())
}

fn optional_number_element(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    value: Option<u16>,
) -> Result<(), NfoGenerationError> {
    if let Some(value) = value {
        number_element(writer, name, value)?;
    }
    Ok(())
}

fn number_element(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    value: u16,
) -> Result<(), NfoGenerationError> {
    text_element(writer, name, &value.to_string())
}

fn text_element(
    writer: &mut Writer<Vec<u8>>,
    name: &str,
    value: &str,
) -> Result<(), NfoGenerationError> {
    write_event(writer, Event::Start(BytesStart::new(name)))?;
    write_event(writer, Event::Text(BytesText::new(value)))?;
    write_event(writer, Event::End(BytesEnd::new(name)))
}

fn write_event(writer: &mut Writer<Vec<u8>>, event: Event<'_>) -> Result<(), NfoGenerationError> {
    writer
        .write_event(event)
        .map_err(|_| NfoGenerationError::Serialization)
}
