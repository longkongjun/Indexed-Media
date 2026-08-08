use serde::{Deserialize, Serialize, Serializer};
use url::Url;
use uuid::Uuid;

use crate::connectors::model::{IntegrationHealth, SecretString};
use crate::platform::secrets::{IntegrationKind, SecretBytes};
use crate::shared::error::{AppError, ErrorCode};

pub(crate) const SECRET_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Fixed built-in automation source kind.
pub enum AutomationSourceKind {
    /// RSS 2.0 or Atom polling source.
    Rss,
    /// HMAC-signed inbound webhook source.
    Webhook,
    /// Downloader completion to inbox mapping.
    DownloadCompletion,
}

impl AutomationSourceKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rss => "rss",
            Self::Webhook => "webhook",
            Self::DownloadCompletion => "download-completion",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "rss" => Some(Self::Rss),
            "webhook" => Some(Self::Webhook),
            "download-completion" => Some(Self::DownloadCompletion),
            _ => None,
        }
    }

    pub(crate) const fn integration_kind(self) -> Option<IntegrationKind> {
        match self {
            Self::Rss => Some(IntegrationKind::AutomationRss),
            Self::Webhook => Some(IntegrationKind::AutomationWebhook),
            Self::DownloadCompletion => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
/// Closed actions accepted by an inbound webhook source.
pub enum WebhookAction {
    #[serde(rename = "download.create")]
    /// Create one managed download task.
    DownloadCreate,
    #[serde(rename = "inbox.reconcile")]
    /// Reconcile one registered inbox.
    InboxReconcile,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
/// Strict secret-bearing input union for built-in automation sources.
pub enum AutomationSourceInput {
    /// RSS or Atom polling configuration.
    Rss {
        display_name: String,
        enabled: bool,
        feed_url: SecretString,
        downloader_connection_id: Uuid,
        poll_interval_seconds: u32,
    },
    /// Signed inbound webhook configuration; Core generates the secret.
    Webhook {
        display_name: String,
        enabled: bool,
        allowed_actions: Vec<WebhookAction>,
    },
    /// Downloader completion mapping without any remote path.
    DownloadCompletion {
        display_name: String,
        enabled: bool,
        downloader_connection_id: Uuid,
        inbox_directory_id: Uuid,
    },
}

impl std::fmt::Debug for AutomationSourceInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rss {
                display_name,
                enabled,
                downloader_connection_id,
                poll_interval_seconds,
                ..
            } => formatter
                .debug_struct("Rss")
                .field("display_name", display_name)
                .field("enabled", enabled)
                .field("feed_url", &"[REDACTED]")
                .field("downloader_connection_id", downloader_connection_id)
                .field("poll_interval_seconds", poll_interval_seconds)
                .finish(),
            Self::Webhook {
                display_name,
                enabled,
                allowed_actions,
            } => formatter
                .debug_struct("Webhook")
                .field("display_name", display_name)
                .field("enabled", enabled)
                .field("allowed_actions", allowed_actions)
                .finish(),
            Self::DownloadCompletion {
                display_name,
                enabled,
                downloader_connection_id,
                inbox_directory_id,
            } => formatter
                .debug_struct("DownloadCompletion")
                .field("display_name", display_name)
                .field("enabled", enabled)
                .field("downloader_connection_id", downloader_connection_id)
                .field("inbox_directory_id", inbox_directory_id)
                .finish(),
        }
    }
}

pub(crate) struct ValidatedAutomationSourceInput {
    pub kind: AutomationSourceKind,
    pub display_name: String,
    pub enabled: bool,
    pub downloader_connection_id: Option<Uuid>,
    pub inbox_directory_id: Option<Uuid>,
    pub endpoint_summary: Option<String>,
    pub poll_interval_seconds: Option<u32>,
    pub allowed_actions: Vec<WebhookAction>,
    pub secret: Option<SecretBytes>,
}

impl AutomationSourceInput {
    #[must_use]
    pub(crate) const fn kind(&self) -> AutomationSourceKind {
        match self {
            Self::Rss { .. } => AutomationSourceKind::Rss,
            Self::Webhook { .. } => AutomationSourceKind::Webhook,
            Self::DownloadCompletion { .. } => AutomationSourceKind::DownloadCompletion,
        }
    }

    pub(crate) fn validate(
        self,
        generated_secret: Option<SecretString>,
    ) -> Result<ValidatedAutomationSourceInput, AppError> {
        match self {
            Self::Rss {
                display_name,
                enabled,
                feed_url,
                downloader_connection_id,
                poll_interval_seconds,
            } => {
                if generated_secret.is_some() || !(60..=86_400).contains(&poll_interval_seconds) {
                    return Err(validation());
                }
                let display_name = valid_name(&display_name)?;
                if !(1..=4096).contains(&feed_url.len()) {
                    return Err(validation());
                }
                let secret = feed_url.into_secret_bytes();
                let value = std::str::from_utf8(secret.expose()).map_err(|_| validation())?;
                let parsed = Url::parse(value).map_err(|_| validation())?;
                if parsed.scheme() != "https"
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                    || parsed.cannot_be_a_base()
                {
                    return Err(validation());
                }
                let endpoint_summary = parsed.origin().ascii_serialization();
                if !(1..=255).contains(&endpoint_summary.len()) {
                    return Err(validation());
                }
                Ok(ValidatedAutomationSourceInput {
                    kind: AutomationSourceKind::Rss,
                    display_name,
                    enabled,
                    downloader_connection_id: Some(downloader_connection_id),
                    inbox_directory_id: None,
                    endpoint_summary: Some(endpoint_summary),
                    poll_interval_seconds: Some(poll_interval_seconds),
                    allowed_actions: Vec::new(),
                    secret: Some(secret),
                })
            }
            Self::Webhook {
                display_name,
                enabled,
                mut allowed_actions,
            } => {
                let generated_secret = generated_secret.ok_or_else(validation)?;
                if !(32..=128).contains(&generated_secret.len())
                    || !(1..=2).contains(&allowed_actions.len())
                {
                    return Err(validation());
                }
                allowed_actions.sort_unstable();
                allowed_actions.dedup();
                if allowed_actions.is_empty() || allowed_actions.len() > 2 {
                    return Err(validation());
                }
                Ok(ValidatedAutomationSourceInput {
                    kind: AutomationSourceKind::Webhook,
                    display_name: valid_name(&display_name)?,
                    enabled,
                    downloader_connection_id: None,
                    inbox_directory_id: None,
                    endpoint_summary: None,
                    poll_interval_seconds: None,
                    allowed_actions,
                    secret: Some(generated_secret.into_secret_bytes()),
                })
            }
            Self::DownloadCompletion {
                display_name,
                enabled,
                downloader_connection_id,
                inbox_directory_id,
            } => {
                if generated_secret.is_some() {
                    return Err(validation());
                }
                Ok(ValidatedAutomationSourceInput {
                    kind: AutomationSourceKind::DownloadCompletion,
                    display_name: valid_name(&display_name)?,
                    enabled,
                    downloader_connection_id: Some(downloader_connection_id),
                    inbox_directory_id: Some(inbox_directory_id),
                    endpoint_summary: None,
                    poll_interval_seconds: None,
                    allowed_actions: Vec::new(),
                    secret: None,
                })
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Internal redacted source record with epoch-microsecond timestamps.
pub struct AutomationSource {
    pub id: Uuid,
    pub kind: AutomationSourceKind,
    pub display_name: String,
    pub enabled: bool,
    pub downloader_connection_id: Option<Uuid>,
    pub inbox_directory_id: Option<Uuid>,
    pub endpoint_summary: Option<String>,
    pub poll_interval_seconds: Option<u32>,
    pub allowed_actions: Vec<WebhookAction>,
    pub secret_fingerprint: Option<String>,
    pub config_version: i64,
    pub health: IntegrationHealth,
    pub checked_at_us: Option<i64>,
    pub failure_code: Option<AutomationFailureCode>,
    pub projection_version: i64,
    pub updated_at_us: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Public redacted source projection.
pub struct AutomationSourceView {
    pub id: Uuid,
    pub kind: AutomationSourceKind,
    pub display_name: String,
    pub enabled: bool,
    pub downloader_connection_id: Option<Uuid>,
    pub inbox_directory_id: Option<Uuid>,
    pub endpoint_summary: Option<String>,
    pub poll_interval_seconds: Option<u32>,
    pub allowed_actions: Vec<WebhookAction>,
    pub secret_fingerprint: Option<String>,
    pub config_version: i64,
    pub health: IntegrationHealth,
    pub checked_at: Option<String>,
    pub failure_code: Option<AutomationFailureCode>,
    pub projection_version: i64,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Stable redacted automation failure category.
pub enum AutomationFailureCode {
    #[serde(rename = "automation.source-disabled")]
    SourceDisabled,
    #[serde(rename = "automation.signature-invalid")]
    SignatureInvalid,
    #[serde(rename = "automation.replay")]
    Replay,
    #[serde(rename = "automation.action-invalid")]
    ActionInvalid,
    #[serde(rename = "automation.payload-invalid")]
    PayloadInvalid,
    #[serde(rename = "automation.downstream-conflict")]
    DownstreamConflict,
    #[serde(rename = "integration.not-configured")]
    IntegrationNotConfigured,
    #[serde(rename = "integration.unauthorized")]
    IntegrationUnauthorized,
    #[serde(rename = "integration.rate-limited")]
    IntegrationRateLimited,
    #[serde(rename = "integration.unavailable")]
    IntegrationUnavailable,
    #[serde(rename = "provider.timeout")]
    ProviderTimeout,
    #[serde(rename = "provider.response-too-large")]
    ResponseTooLarge,
    #[serde(rename = "provider.invalid-response")]
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Closed durable automation action set.
pub enum AutomationAction {
    /// Create one managed download task.
    CreateDownload,
    /// Request reconciliation of one registered inbox.
    ReconcileInbox,
}

impl AutomationAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateDownload => "create-download",
            Self::ReconcileInbox => "reconcile-inbox",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "create-download" => Some(Self::CreateDownload),
            "reconcile-inbox" => Some(Self::ReconcileInbox),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
/// Durable automation event lifecycle.
pub enum AutomationEventStatus {
    Pending,
    Running,
    RetryWait,
    Completed,
    Failed,
    Cancelled,
}

impl AutomationEventStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::RetryWait => "retry-wait",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "retry-wait" => Some(Self::RetryWait),
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// Optional public automation event list filters.
pub struct AutomationEventFilter {
    pub source_id: Option<Uuid>,
    pub status: Option<AutomationEventStatus>,
    pub action: Option<AutomationAction>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Public event projection without action payload or lease material.
pub struct AutomationEventView {
    pub id: Uuid,
    pub source_id: Uuid,
    pub source_display_name: String,
    pub action: AutomationAction,
    pub status: AutomationEventStatus,
    pub downstream_kind: Option<String>,
    pub downstream_id: Option<Uuid>,
    pub result_count: Option<i64>,
    pub failure_code: Option<AutomationFailureCode>,
    pub attempt_count: u8,
    pub retry_at: Option<String>,
    pub allowed_actions: Vec<String>,
    pub projection_version: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl AutomationFailureCode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SourceDisabled => "automation.source-disabled",
            Self::SignatureInvalid => "automation.signature-invalid",
            Self::Replay => "automation.replay",
            Self::ActionInvalid => "automation.action-invalid",
            Self::PayloadInvalid => "automation.payload-invalid",
            Self::DownstreamConflict => "automation.downstream-conflict",
            Self::IntegrationNotConfigured => "integration.not-configured",
            Self::IntegrationUnauthorized => "integration.unauthorized",
            Self::IntegrationRateLimited => "integration.rate-limited",
            Self::IntegrationUnavailable => "integration.unavailable",
            Self::ProviderTimeout => "provider.timeout",
            Self::ResponseTooLarge => "provider.response-too-large",
            Self::InvalidResponse => "provider.invalid-response",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(value.to_owned())).ok()
    }
}

/// Secret returned only by the current response; Debug redacts and Drop clears it.
pub struct OneTimeSecret(Vec<u8>);

impl OneTimeSecret {
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value.into_bytes())
    }
}

impl Serialize for OneTimeSecret {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let value = std::str::from_utf8(&self.0).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(value)
    }
}

impl std::fmt::Debug for OneTimeSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OneTimeSecret([REDACTED])")
    }
}

impl Drop for OneTimeSecret {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[derive(Debug, Serialize)]
/// Webhook creation or rotation response containing a one-time secret.
pub struct WebhookSecretReceipt {
    pub source: AutomationSourceView,
    pub secret: OneTimeSecret,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
/// Source creation response matching the contract union.
pub enum AutomationSourceCreateResult {
    Source(AutomationSourceView),
    Webhook(WebhookSecretReceipt),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
/// Bounded candidate source test result without endpoint or response content.
pub struct AutomationSourceConnectionTestResult {
    /// Whether all checks available for this source type succeeded.
    pub reachable: bool,
    /// Redacted candidate health.
    pub health: IntegrationHealth,
    /// Parsed feed format when an RSS adapter performed a probe.
    pub detected_format: Option<String>,
    /// Accepted feed item count.
    pub item_count: u16,
    /// Safely ignored feed item count.
    pub ignored_item_count: u16,
    /// Stable redacted failure classification.
    pub failure_code: Option<AutomationFailureCode>,
    /// RFC3339 completion time.
    pub checked_at: String,
}

fn valid_name(value: &str) -> Result<String, AppError> {
    let value = value.trim().to_owned();
    if !(1..=120).contains(&value.chars().count()) || value.chars().any(char::is_control) {
        return Err(validation());
    }
    Ok(value)
}

fn validation() -> AppError {
    AppError::new(ErrorCode::ValidationFailed, "invalid automation source")
}
