use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};
use url::{Host, Url};
use uuid::Uuid;

use crate::automation::model::AutomationFailureCode;
use crate::connectors::model::IntegrationHealth;
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
/// Candidate or replacement configuration for the built-in local Ollama enhancer.
pub struct EnhancerConfigInput {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub timeout_ms: u32,
}

impl std::fmt::Debug for EnhancerConfigInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnhancerConfigInput")
            .field("enabled", &self.enabled)
            .field("base_url", &"[REDACTED]")
            .field("model", &"[REDACTED]")
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

impl EnhancerConfigInput {
    /// Validate and normalize a local-only origin and bounded model configuration.
    ///
    /// # Errors
    ///
    /// Rejects non-local hosts, credentials, URL suffixes, controls, and out-of-range values.
    pub fn validate(self) -> Result<ValidatedEnhancerConfig, AppError> {
        let url = Url::parse(&self.base_url).map_err(|_| validation())?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
            || url.cannot_be_a_base()
        {
            return Err(validation());
        }
        match url.host().ok_or_else(validation)? {
            Host::Domain(host) if host.eq_ignore_ascii_case("localhost") => {}
            Host::Ipv4(address) if allowed_ipv4(address) => {}
            Host::Ipv6(address) if allowed_ipv6(address) => {}
            _ => return Err(validation()),
        }
        let endpoint_summary = url.origin().ascii_serialization();
        let model = self.model.trim().to_owned();
        if endpoint_summary.len() > 255
            || !(1..=128).contains(&model.chars().count())
            || model.chars().any(char::is_whitespace)
            || !(100..=30_000).contains(&self.timeout_ms)
        {
            return Err(validation());
        }
        Ok(ValidatedEnhancerConfig {
            enabled: self.enabled,
            base_url: endpoint_summary.clone(),
            endpoint_summary,
            model,
            timeout_ms: self.timeout_ms,
        })
    }
}

#[derive(Clone)]
/// Normalized configuration that can be borrowed by a protocol adapter.
pub struct ValidatedEnhancerConfig {
    pub(crate) enabled: bool,
    pub(crate) base_url: String,
    pub(crate) endpoint_summary: String,
    pub(crate) model: String,
    pub(crate) timeout_ms: u32,
}

impl ValidatedEnhancerConfig {
    #[must_use]
    pub fn endpoint(&self) -> super::port::EnhancerEndpoint<'_> {
        super::port::EnhancerEndpoint {
            base_url: &self.base_url,
            model: &self.model,
            timeout_ms: self.timeout_ms,
        }
    }
}

impl std::fmt::Debug for ValidatedEnhancerConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ValidatedEnhancerConfig")
            .field("enabled", &self.enabled)
            .field("endpoint", &"[REDACTED]")
            .field("model", &"[REDACTED]")
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Durable singleton row used by the service and future identification integration.
pub struct EnhancerConfigRecord {
    pub id: Uuid,
    pub enabled: bool,
    pub base_url: String,
    pub endpoint_summary: String,
    pub model: String,
    pub timeout_ms: u32,
    pub config_version: i64,
    pub health: IntegrationHealth,
    pub checked_at_us: Option<i64>,
    pub fallback_code: Option<AutomationFailureCode>,
    pub projection_version: i64,
    pub updated_at_us: i64,
}

impl EnhancerConfigRecord {
    #[must_use]
    pub fn endpoint(&self) -> super::port::EnhancerEndpoint<'_> {
        super::port::EnhancerEndpoint {
            base_url: &self.base_url,
            model: &self.model,
            timeout_ms: self.timeout_ms,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Redacted RFC3339 API projection without prompts or model responses.
pub struct EnhancerConfigView {
    pub kind: &'static str,
    pub enabled: bool,
    pub endpoint_summary: String,
    pub model: String,
    pub timeout_ms: u32,
    pub config_version: i64,
    pub health: IntegrationHealth,
    pub checked_at: Option<String>,
    pub fallback_code: Option<AutomationFailureCode>,
    pub projection_version: i64,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Non-persistent bounded candidate probe result.
pub struct EnhancerConnectionTestResult {
    pub reachable: bool,
    pub health: IntegrationHealth,
    pub adapter_version: String,
    pub model_available: bool,
    pub fallback_code: Option<AutomationFailureCode>,
    pub checked_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Safe health data that can be committed without protocol text.
pub struct EnhancerHealthProjection {
    pub health: IntegrationHealth,
    pub fallback_code: Option<AutomationFailureCode>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Product-neutral media kind accepted from the local enhancer.
pub enum EnhancerMediaKind {
    Movie,
    Episode,
}

#[derive(Clone, Serialize)]
/// Deterministic parser result supplied as context, never as model authority.
pub struct BaseIdentityHint {
    pub normalized_title: String,
    pub media_kind: EnhancerMediaKind,
    pub year: Option<u16>,
    pub season: Option<u16>,
    pub episodes: Vec<u16>,
}

impl BaseIdentityHint {
    #[must_use]
    pub fn movie(normalized_title: String, year: Option<u16>) -> Self {
        Self {
            normalized_title,
            media_kind: EnhancerMediaKind::Movie,
            year,
            season: None,
            episodes: Vec::new(),
        }
    }

    fn validate(&self) -> bool {
        valid_text(&self.normalized_title, 512)
            && self.year.is_none_or(valid_year)
            && coherent_numbers(self.media_kind, self.season, &self.episodes)
    }
}

impl std::fmt::Debug for BaseIdentityHint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BaseIdentityHint([REDACTED])")
    }
}

#[derive(Clone, Serialize)]
/// Bounded filename context sent to the local enhancer.
pub struct EnhancementInput {
    basename: String,
    parent_segments: Vec<String>,
    base_hint: BaseIdentityHint,
}

impl EnhancementInput {
    /// Construct a path-free, bounded model request input.
    ///
    /// # Errors
    ///
    /// Rejects empty/control text, more than two parents, and incoherent base hints.
    pub fn new(
        basename: String,
        parent_segments: Vec<String>,
        base_hint: BaseIdentityHint,
    ) -> Result<Self, AppError> {
        if !valid_segment_text(&basename, 512)
            || parent_segments.len() > 2
            || parent_segments
                .iter()
                .any(|value| !valid_segment_text(value, 255))
            || !base_hint.validate()
        {
            return Err(validation());
        }
        Ok(Self {
            basename,
            parent_segments,
            base_hint,
        })
    }

    pub(crate) fn basename(&self) -> &str {
        &self.basename
    }

    pub(crate) fn parent_segments(&self) -> &[String] {
        &self.parent_segments
    }

    pub(crate) const fn base_hint(&self) -> &BaseIdentityHint {
        &self.base_hint
    }
}

impl std::fmt::Debug for EnhancementInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EnhancementInput([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
/// Validated supporting hints; no provider identifier or arbitrary action can be represented.
pub struct EnhancementHints {
    pub title: Option<String>,
    pub year: Option<u16>,
    pub media_kind: Option<EnhancerMediaKind>,
    pub season: Option<u16>,
    pub episodes: Vec<u16>,
}

impl EnhancementHints {
    pub(crate) fn validate(self) -> Result<Self, super::port::EnhancerError> {
        if self
            .title
            .as_deref()
            .is_some_and(|value| !valid_text(value, 512))
            || self.year.is_some_and(|value| !valid_year(value))
            || self.episodes.len() > 100
            || self.episodes.windows(2).any(|pair| pair[0] >= pair[1])
            || !coherent_optional_numbers(self.media_kind, self.season, &self.episodes)
            || (self.title.is_none()
                && self.year.is_none()
                && self.media_kind.is_none()
                && self.season.is_none()
                && self.episodes.is_empty())
        {
            return Err(super::port::EnhancerError::InvalidResponse);
        }
        Ok(self)
    }
}

impl std::fmt::Debug for EnhancementHints {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EnhancementHints([REDACTED])")
    }
}

fn coherent_numbers(kind: EnhancerMediaKind, season: Option<u16>, episodes: &[u16]) -> bool {
    match kind {
        EnhancerMediaKind::Movie => season.is_none() && episodes.is_empty(),
        EnhancerMediaKind::Episode => {
            season.is_some_and(valid_number)
                && !episodes.is_empty()
                && episodes.iter().copied().all(valid_number)
                && episodes.len() <= 100
        }
    }
}

fn coherent_optional_numbers(
    kind: Option<EnhancerMediaKind>,
    season: Option<u16>,
    episodes: &[u16],
) -> bool {
    match kind {
        Some(kind) => coherent_numbers(kind, season, episodes),
        None => season.is_none() && episodes.is_empty(),
    }
}

fn valid_text(value: &str, max_chars: usize) -> bool {
    (1..=max_chars).contains(&value.chars().count()) && !value.chars().any(char::is_control)
}

fn valid_segment_text(value: &str, max_chars: usize) -> bool {
    valid_text(value, max_chars) && !value.contains(['/', '\\'])
}

const fn valid_year(value: u16) -> bool {
    value >= 1878 && value <= 2100
}

const fn valid_number(value: u16) -> bool {
    value >= 1 && value <= 10_000
}

pub(crate) const fn allowed_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => allowed_ipv4(address),
        IpAddr::V6(address) => allowed_ipv6(address),
    }
}

const fn allowed_ipv4(address: Ipv4Addr) -> bool {
    address.is_loopback() || address.is_private() || address.is_link_local()
}

const fn allowed_ipv6(address: Ipv6Addr) -> bool {
    address.is_loopback()
        || address.is_unicast_link_local()
        || (address.segments()[0] & 0xfe00) == 0xfc00
}

fn validation() -> AppError {
    AppError::new(
        ErrorCode::ValidationFailed,
        "invalid identification enhancer",
    )
}
