use std::path::Path;
use std::sync::Arc;

use sqlx::SqlitePool;
use uuid::Uuid;

use crate::connectors::model::{IntegrationHealth, SecretString};
use crate::platform::random;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

use super::model::{
    AutomationFailureCode, AutomationSource, AutomationSourceConnectionTestResult,
    AutomationSourceCreateResult, AutomationSourceInput, AutomationSourceView, OneTimeSecret,
    WebhookSecretReceipt,
};
use super::rss::client::{FeedClient, FeedClientError, FeedHttpClient, FeedResponse};
use super::rss::parser::{FeedFormat, FeedParseError, FeedParser};
use super::source_store::AutomationSourceStore;

#[derive(Clone)]
/// Coordinates source creation, redacted projection, and optimistic concurrency.
pub struct AutomationSourceService {
    store: AutomationSourceStore,
    feed_client: Arc<dyn FeedClient>,
}

impl AutomationSourceService {
    /// Open an automation source service using the current instance key.
    ///
    /// # Errors
    ///
    /// Returns a configuration error when the instance key boundary is unsafe.
    pub fn open(pool: SqlitePool, config_dir: &Path) -> Result<Self, AppError> {
        Ok(Self {
            store: AutomationSourceStore::open(pool, config_dir)?,
            feed_client: Arc::new(
                FeedHttpClient::production()
                    .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?,
            ),
        })
    }

    /// List one page of redacted source projections.
    ///
    /// # Errors
    ///
    /// Returns validation or persistence errors.
    pub async fn list(
        &self,
        page: &PageRequest,
    ) -> Result<CursorPage<AutomationSourceView>, AppError> {
        let page = self.store.list_page(page).await?;
        Ok(CursorPage {
            items: page
                .items
                .into_iter()
                .map(project)
                .collect::<Result<Vec<_>, _>>()?,
            next_cursor: page.next_cursor,
        })
    }

    /// Read one redacted source projection.
    ///
    /// # Errors
    ///
    /// Returns not-found or persistence errors.
    pub async fn get(&self, id: Uuid) -> Result<AutomationSourceView, AppError> {
        self.store
            .get(id)
            .await?
            .ok_or_else(not_found)
            .and_then(project)
    }

    /// Create a source, returning a generated webhook secret exactly in this result.
    ///
    /// # Errors
    ///
    /// Returns validation, reference, random-source, encryption, or persistence errors.
    pub async fn create(
        &self,
        input: AutomationSourceInput,
    ) -> Result<AutomationSourceCreateResult, AppError> {
        let webhook = matches!(&input, AutomationSourceInput::Webhook { .. });
        let secret = webhook.then(random::random_token).transpose()?;
        let record = self
            .store
            .insert(
                Uuid::now_v7(),
                input,
                secret
                    .as_ref()
                    .map(|value| SecretString::new(value.clone())),
                chrono::Utc::now().timestamp_micros(),
            )
            .await?;
        let source = project(record)?;
        Ok(match secret {
            Some(secret) => AutomationSourceCreateResult::Webhook(WebhookSecretReceipt {
                source,
                secret: OneTimeSecret::new(secret),
            }),
            None => AutomationSourceCreateResult::Source(source),
        })
    }

    /// Replace a source while retaining its immutable kind and webhook key.
    ///
    /// # Errors
    ///
    /// Returns not-found, validation, version-conflict, encryption, or persistence errors.
    pub async fn replace(
        &self,
        id: Uuid,
        expected_version: i64,
        input: AutomationSourceInput,
    ) -> Result<AutomationSourceView, AppError> {
        project(
            self.store
                .replace(
                    id,
                    expected_version,
                    input,
                    chrono::Utc::now().timestamp_micros(),
                )
                .await?,
        )
    }

    /// Delete a disabled source under optimistic concurrency.
    ///
    /// # Errors
    ///
    /// Returns not-found, in-use, version-conflict, or persistence errors.
    pub async fn delete(&self, id: Uuid, expected_version: i64) -> Result<(), AppError> {
        self.store.delete(id, expected_version).await
    }

    /// Rotate a webhook secret with an idempotent one-time response receipt.
    ///
    /// # Errors
    ///
    /// Returns validation, not-found, version, idempotency, encryption, or persistence errors.
    pub async fn rotate_webhook_secret(
        &self,
        id: Uuid,
        expected_version: i64,
        idempotency_key: &str,
    ) -> Result<WebhookSecretReceipt, AppError> {
        let secret = random::random_token()?;
        let rotated = self
            .store
            .rotate_webhook_secret(
                id,
                expected_version,
                idempotency_key,
                SecretString::new(secret),
                chrono::Utc::now().timestamp_micros(),
            )
            .await?;
        let value = String::from_utf8(rotated.secret.expose().to_vec())
            .map_err(|_| AppError::new(ErrorCode::Internal, "invalid webhook rotation secret"))?;
        Ok(WebhookSecretReceipt {
            source: project(rotated.source)?,
            secret: OneTimeSecret::new(value),
        })
    }

    /// Validate a candidate without persisting it or disclosing endpoint material.
    ///
    /// RSS reachability is intentionally unavailable until the bounded adapter is installed.
    ///
    /// # Errors
    ///
    /// Returns validation or random-source errors.
    pub async fn test_candidate(
        &self,
        input: AutomationSourceInput,
    ) -> Result<AutomationSourceConnectionTestResult, AppError> {
        let webhook = matches!(&input, AutomationSourceInput::Webhook { .. });
        let secret = webhook.then(random::random_token).transpose()?;
        let input = input.validate(secret.map(SecretString::new))?;
        self.store.validate_references(&input).await?;
        let checked_at = format_time(chrono::Utc::now().timestamp_micros())?;
        if !input.enabled {
            return Ok(AutomationSourceConnectionTestResult {
                reachable: false,
                health: IntegrationHealth::Degraded,
                detected_format: None,
                item_count: 0,
                ignored_item_count: 0,
                failure_code: Some(AutomationFailureCode::SourceDisabled),
                checked_at,
            });
        }
        let Some(feed_url) = input
            .secret
            .as_ref()
            .filter(|_| matches!(input.kind, super::model::AutomationSourceKind::Rss))
        else {
            return Ok(AutomationSourceConnectionTestResult {
                reachable: true,
                health: IntegrationHealth::Healthy,
                detected_format: None,
                item_count: 0,
                ignored_item_count: 0,
                failure_code: None,
                checked_at,
            });
        };
        let response = match self.feed_client.fetch(feed_url, None).await {
            Ok(response) => response,
            Err(error) => return Ok(failed_test(error, checked_at)),
        };
        match response {
            FeedResponse::NotModified { .. } => Ok(AutomationSourceConnectionTestResult {
                reachable: true,
                health: IntegrationHealth::Healthy,
                detected_format: None,
                item_count: 0,
                ignored_item_count: 0,
                failure_code: None,
                checked_at,
            }),
            FeedResponse::Modified { body, .. } => match FeedParser::default().parse(&body) {
                Ok(feed) => Ok(AutomationSourceConnectionTestResult {
                    reachable: true,
                    health: IntegrationHealth::Healthy,
                    detected_format: Some(match feed.format {
                        FeedFormat::Rss20 => "rss-2.0".to_owned(),
                        FeedFormat::Atom => "atom".to_owned(),
                    }),
                    item_count: u16::try_from(feed.items.len())
                        .map_err(|_| AppError::new(ErrorCode::Internal, "feed item overflow"))?,
                    ignored_item_count: feed.ignored_item_count,
                    failure_code: None,
                    checked_at,
                }),
                Err(error) => Ok(failed_parse_test(error, checked_at)),
            },
        }
    }
}

fn failed_test(error: FeedClientError, checked_at: String) -> AutomationSourceConnectionTestResult {
    let (health, failure_code) = match error {
        FeedClientError::Unauthorized => (
            IntegrationHealth::Unauthorized,
            AutomationFailureCode::IntegrationUnauthorized,
        ),
        FeedClientError::RateLimited => (
            IntegrationHealth::RateLimited,
            AutomationFailureCode::IntegrationRateLimited,
        ),
        FeedClientError::Unavailable => (
            IntegrationHealth::Unavailable,
            AutomationFailureCode::IntegrationUnavailable,
        ),
        FeedClientError::Timeout => (
            IntegrationHealth::Unavailable,
            AutomationFailureCode::ProviderTimeout,
        ),
        FeedClientError::ResponseTooLarge => (
            IntegrationHealth::Degraded,
            AutomationFailureCode::ResponseTooLarge,
        ),
        FeedClientError::InvalidResponse => (
            IntegrationHealth::Degraded,
            AutomationFailureCode::InvalidResponse,
        ),
    };
    AutomationSourceConnectionTestResult {
        reachable: false,
        health,
        detected_format: None,
        item_count: 0,
        ignored_item_count: 0,
        failure_code: Some(failure_code),
        checked_at,
    }
}

fn failed_parse_test(
    error: FeedParseError,
    checked_at: String,
) -> AutomationSourceConnectionTestResult {
    let failure_code = if error == FeedParseError::TooLarge {
        AutomationFailureCode::ResponseTooLarge
    } else {
        AutomationFailureCode::InvalidResponse
    };
    AutomationSourceConnectionTestResult {
        reachable: false,
        health: IntegrationHealth::Degraded,
        detected_format: None,
        item_count: 0,
        ignored_item_count: 0,
        failure_code: Some(failure_code),
        checked_at,
    }
}

fn project(source: AutomationSource) -> Result<AutomationSourceView, AppError> {
    Ok(AutomationSourceView {
        id: source.id,
        kind: source.kind,
        display_name: source.display_name,
        enabled: source.enabled,
        downloader_connection_id: source.downloader_connection_id,
        inbox_directory_id: source.inbox_directory_id,
        endpoint_summary: source.endpoint_summary,
        poll_interval_seconds: source.poll_interval_seconds,
        allowed_actions: source.allowed_actions,
        secret_fingerprint: source.secret_fingerprint,
        config_version: source.config_version,
        health: source.health,
        checked_at: source.checked_at_us.map(format_time).transpose()?,
        failure_code: source.failure_code,
        projection_version: source.projection_version,
        updated_at: format_time(source.updated_at_us)?,
    })
}

fn format_time(value: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "invalid automation timestamp"))
}

fn not_found() -> AppError {
    AppError::new(ErrorCode::NotFound, "automation source not found")
}
