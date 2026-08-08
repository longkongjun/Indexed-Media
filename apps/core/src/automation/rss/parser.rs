use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use sha2::{Digest as _, Sha256};
use url::Url;

use crate::platform::secrets::SecretBytes;

/// Maximum decoded feed response size.
pub const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;
/// Maximum XML element nesting depth.
pub const MAX_XML_DEPTH: usize = 32;
/// Maximum number of feed entries examined in one document.
pub const MAX_FEED_ITEMS: usize = 500;
const MAX_ID_BYTES: usize = 2048;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SOURCE_BYTES: usize = 8192;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Supported feed wire format.
pub enum FeedFormat {
    /// RSS version 2.0.
    Rss20,
    /// IETF Atom feed.
    Atom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// Bounded feed parsing failure without source content.
pub enum FeedParseError {
    /// Response exceeds the fixed byte budget.
    #[error("feed response is too large")]
    TooLarge,
    /// XML nesting exceeds the fixed depth budget.
    #[error("feed XML is too deep")]
    TooDeep,
    /// Feed contains more than the fixed item budget.
    #[error("feed contains too many items")]
    TooManyItems,
    /// DTD, entity, processing instruction, or `XInclude` is present.
    #[error("feed XML contains active constructs")]
    UnsafeXml,
    /// XML or a bounded field is malformed.
    #[error("feed response is invalid")]
    InvalidXml,
    /// The document root is not RSS 2.0 or Atom.
    #[error("feed format is unsupported")]
    UnsupportedFormat,
}

/// One accepted feed entry; the download source is redacted and zeroized.
pub struct FeedItem {
    /// Stable GUID/ID or supported-source fingerprint.
    pub dedup_key: [u8; 32],
    /// Optional bounded display title.
    pub title: Option<String>,
    source: SecretBytes,
}

impl FeedItem {
    #[must_use]
    /// Borrow the source only at the durable event boundary.
    pub const fn source(&self) -> &SecretBytes {
        &self.source
    }

    #[must_use]
    /// Consume the item and transfer its secret source.
    pub fn into_source(self) -> SecretBytes {
        self.source
    }
}

impl std::fmt::Debug for FeedItem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeedItem")
            .field("dedup_key", &hex::encode(self.dedup_key))
            .field("title", &self.title)
            .field("source", &"[REDACTED]")
            .finish()
    }
}

impl PartialEq for FeedItem {
    fn eq(&self, other: &Self) -> bool {
        self.dedup_key == other.dedup_key
            && self.title == other.title
            && self.source.expose() == other.source.expose()
    }
}

impl Eq for FeedItem {}

#[derive(Debug, Eq, PartialEq)]
/// Parsed feed with only valid supported download entries.
pub struct FeedDocument {
    /// Detected supported wire format.
    pub format: FeedFormat,
    /// Valid entries, bounded to [`MAX_FEED_ITEMS`].
    pub items: Vec<FeedItem>,
    /// Entries ignored because their source is missing or unsafe.
    pub ignored_item_count: u16,
}

#[derive(Clone, Copy, Debug, Default)]
/// Stateless bounded feed parser.
pub struct FeedParser {
    _private: (),
}

impl FeedParser {
    /// Parse one complete bounded UTF-8 RSS 2.0 or Atom response.
    ///
    /// # Errors
    ///
    /// Returns a stable error for size, XML, complexity, or format violations.
    #[allow(clippy::too_many_lines)]
    pub fn parse(&self, bytes: &[u8]) -> Result<FeedDocument, FeedParseError> {
        if bytes.len() > MAX_FEED_BYTES {
            return Err(FeedParseError::TooLarge);
        }
        std::str::from_utf8(bytes).map_err(|_| FeedParseError::InvalidXml)?;
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(true);
        reader.config_mut().check_end_names = true;
        let mut stack = Vec::<Vec<u8>>::new();
        let mut format = None;
        let mut current = None::<ItemBuilder>;
        let mut item_depth = 0_usize;
        let mut active = None::<ActiveField>;
        let mut active_depth = 0_usize;
        let mut seen_items = 0_usize;
        let mut items = Vec::new();
        let mut ignored = 0_u16;
        let mut declaration_seen = false;
        let mut xml_version = XmlVersion::Implicit1_0;

        loop {
            let event = reader
                .read_event()
                .map_err(|_| FeedParseError::InvalidXml)?;
            match event {
                Event::Start(start) => {
                    let name = local_name(start.name().as_ref()).to_vec();
                    reject_active_element(start.name().as_ref())?;
                    if stack.is_empty() {
                        format = Some(match name.as_slice() {
                            b"rss"
                                if attribute(
                                    &start,
                                    b"version",
                                    xml_version,
                                    reader.decoder(),
                                )?
                                .as_deref()
                                    == Some("2.0") =>
                            {
                                FeedFormat::Rss20
                            }
                            b"feed" => FeedFormat::Atom,
                            _ => return Err(FeedParseError::UnsupportedFormat),
                        });
                    }
                    if is_item_start(format, &name) {
                        if current.is_some() {
                            return Err(FeedParseError::InvalidXml);
                        }
                        seen_items += 1;
                        if seen_items > MAX_FEED_ITEMS {
                            return Err(FeedParseError::TooManyItems);
                        }
                        current = Some(ItemBuilder::default());
                        item_depth = stack.len() + 1;
                    } else if current.is_some() && stack.len() == item_depth {
                        active = direct_field(format, &name);
                        active_depth = stack.len() + 1;
                        capture_source_attribute(
                            current.as_mut().ok_or(FeedParseError::InvalidXml)?,
                            format,
                            &name,
                            &start,
                            xml_version,
                            reader.decoder(),
                        )?;
                    } else if active.is_some() {
                        return Err(FeedParseError::InvalidXml);
                    }
                    stack.push(name);
                    if stack.len() > MAX_XML_DEPTH {
                        return Err(FeedParseError::TooDeep);
                    }
                }
                Event::Empty(start) => {
                    reject_active_element(start.name().as_ref())?;
                    let qualified_name = start.name();
                    let name = local_name(qualified_name.as_ref());
                    if current.is_some() && stack.len() == item_depth {
                        capture_source_attribute(
                            current.as_mut().ok_or(FeedParseError::InvalidXml)?,
                            format,
                            name,
                            &start,
                            xml_version,
                            reader.decoder(),
                        )?;
                    }
                }
                Event::End(end) => {
                    let qualified_name = end.name();
                    let name = local_name(qualified_name.as_ref());
                    if active.is_some() && stack.len() == active_depth {
                        active = None;
                    }
                    if current.is_some() && stack.len() == item_depth && is_item_start(format, name)
                    {
                        finish_item(
                            current.take().ok_or(FeedParseError::InvalidXml)?,
                            format.ok_or(FeedParseError::UnsupportedFormat)?,
                            &mut items,
                            &mut ignored,
                        )?;
                    }
                    let popped = stack.pop().ok_or(FeedParseError::InvalidXml)?;
                    if popped.as_slice() != name {
                        return Err(FeedParseError::InvalidXml);
                    }
                }
                Event::Text(text) => {
                    let value = text
                        .xml_content(xml_version)
                        .map_err(|_| FeedParseError::InvalidXml)?;
                    if let Some(field) = active {
                        current
                            .as_mut()
                            .ok_or(FeedParseError::InvalidXml)?
                            .push(field, &value)?;
                    } else if stack.is_empty() && !value.trim().is_empty() {
                        return Err(FeedParseError::InvalidXml);
                    }
                }
                Event::CData(text) => {
                    let value = text
                        .xml_content(xml_version)
                        .map_err(|_| FeedParseError::InvalidXml)?;
                    if let Some(field) = active {
                        current
                            .as_mut()
                            .ok_or(FeedParseError::InvalidXml)?
                            .push(field, &value)?;
                    }
                }
                Event::Decl(declaration) => {
                    if declaration_seen || !stack.is_empty() || format.is_some() {
                        return Err(FeedParseError::InvalidXml);
                    }
                    xml_version = match declaration
                        .version()
                        .map_err(|_| FeedParseError::InvalidXml)?
                        .as_ref()
                    {
                        b"1.0" => XmlVersion::Explicit1_0,
                        b"1.1" => XmlVersion::Explicit1_1,
                        _ => return Err(FeedParseError::InvalidXml),
                    };
                    declaration_seen = true;
                }
                Event::DocType(_) | Event::GeneralRef(_) | Event::PI(_) => {
                    return Err(FeedParseError::UnsafeXml);
                }
                Event::Comment(_) => {}
                Event::Eof => break,
            }
        }
        if !stack.is_empty() || current.is_some() {
            return Err(FeedParseError::InvalidXml);
        }
        Ok(FeedDocument {
            format: format.ok_or(FeedParseError::UnsupportedFormat)?,
            items,
            ignored_item_count: ignored,
        })
    }
}

#[derive(Clone, Copy)]
enum ActiveField {
    Identity,
    Title,
    Link,
}

#[derive(Default)]
struct ItemBuilder {
    identity: String,
    title: String,
    enclosure: Option<String>,
    link: String,
}

impl ItemBuilder {
    fn push(&mut self, field: ActiveField, value: &str) -> Result<(), FeedParseError> {
        let (target, limit) = match field {
            ActiveField::Identity => (&mut self.identity, MAX_ID_BYTES),
            ActiveField::Title => (&mut self.title, MAX_TITLE_BYTES),
            ActiveField::Link => (&mut self.link, MAX_SOURCE_BYTES),
        };
        if target.len().saturating_add(value.len()) > limit {
            return Err(FeedParseError::InvalidXml);
        }
        target.push_str(value);
        Ok(())
    }
}

fn direct_field(format: Option<FeedFormat>, name: &[u8]) -> Option<ActiveField> {
    match (format, name) {
        (Some(FeedFormat::Rss20), b"guid") | (Some(FeedFormat::Atom), b"id") => {
            Some(ActiveField::Identity)
        }
        (Some(_), b"title") => Some(ActiveField::Title),
        (Some(FeedFormat::Rss20), b"link") => Some(ActiveField::Link),
        _ => None,
    }
}

fn capture_source_attribute(
    item: &mut ItemBuilder,
    format: Option<FeedFormat>,
    name: &[u8],
    start: &BytesStart<'_>,
    xml_version: XmlVersion,
    decoder: quick_xml::encoding::Decoder,
) -> Result<(), FeedParseError> {
    let source = match (format, name) {
        (Some(FeedFormat::Rss20), b"enclosure") => attribute(start, b"url", xml_version, decoder)?,
        (Some(FeedFormat::Atom), b"link") => {
            let relation = attribute(start, b"rel", xml_version, decoder)?;
            if relation.as_deref() == Some("enclosure") {
                attribute(start, b"href", xml_version, decoder)?
            } else {
                None
            }
        }
        _ => None,
    };
    if let Some(source) = source {
        if source.len() > MAX_SOURCE_BYTES {
            return Err(FeedParseError::InvalidXml);
        }
        item.enclosure = Some(source);
    }
    Ok(())
}

fn attribute(
    start: &BytesStart<'_>,
    name: &[u8],
    xml_version: XmlVersion,
    decoder: quick_xml::encoding::Decoder,
) -> Result<Option<String>, FeedParseError> {
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute.map_err(|_| FeedParseError::InvalidXml)?;
        if local_name(attribute.key.as_ref()) == name {
            return attribute
                .decoded_and_normalized_value(xml_version, decoder)
                .map(|value| Some(value.into_owned()))
                .map_err(|_| FeedParseError::InvalidXml);
        }
    }
    Ok(None)
}

fn finish_item(
    item: ItemBuilder,
    format: FeedFormat,
    items: &mut Vec<FeedItem>,
    ignored: &mut u16,
) -> Result<(), FeedParseError> {
    let source = item.enclosure.unwrap_or(item.link);
    let source = source.trim();
    if !valid_download_source(source) {
        *ignored = ignored.checked_add(1).ok_or(FeedParseError::TooManyItems)?;
        return Ok(());
    }
    let identity = item.identity.trim();
    let dedup_key = if identity.is_empty() {
        digest(b"source", source.as_bytes())
    } else {
        let domain = match format {
            FeedFormat::Rss20 => b"rss-guid".as_slice(),
            FeedFormat::Atom => b"atom-id".as_slice(),
        };
        digest(domain, identity.as_bytes())
    };
    let title = item.title.trim();
    items.push(FeedItem {
        dedup_key,
        title: (!title.is_empty()).then(|| title.to_owned()),
        source: SecretBytes::new(source.as_bytes().to_vec()),
    });
    Ok(())
}

fn valid_download_source(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_SOURCE_BYTES {
        return false;
    }
    let Ok(parsed) = Url::parse(value) else {
        return false;
    };
    match parsed.scheme() {
        "magnet" => parsed
            .query_pairs()
            .any(|(key, value)| key == "xt" && value.starts_with("urn:btih:") && value.len() > 9),
        "https" => {
            parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.fragment().is_none()
        }
        _ => false,
    }
}

fn digest(domain: &[u8], value: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"mediaflow.rss-item.v1\0");
    digest.update(domain);
    digest.update([0]);
    digest.update(value);
    digest.finalize().into()
}

fn is_item_start(format: Option<FeedFormat>, name: &[u8]) -> bool {
    matches!(
        (format, name),
        (Some(FeedFormat::Rss20), b"item") | (Some(FeedFormat::Atom), b"entry")
    )
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn reject_active_element(name: &[u8]) -> Result<(), FeedParseError> {
    if local_name(name).eq_ignore_ascii_case(b"include")
        && name
            .split(|byte| *byte == b':')
            .next()
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"xi"))
    {
        return Err(FeedParseError::UnsafeXml);
    }
    Ok(())
}
