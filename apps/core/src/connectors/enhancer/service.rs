use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sqlx::SqlitePool;

use crate::automation::model::AutomationFailureCode;
use crate::connectors::model::IntegrationHealth;
use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::{AppError, ErrorCode};

use super::model::{
    EnhancementInput, EnhancerConfigInput, EnhancerConfigRecord, EnhancerConfigView,
    EnhancerConnectionTestResult, EnhancerHealthProjection,
};
use super::port::{EnhancementHints, EnhancerError, IdentificationEnhancer};
use super::store::EnhancerStore;

const ADAPTER_VERSION: &str = "ollama-v1";

#[derive(Clone)]
/// Coordinates candidate probes, versioned configuration, health, and bounded enhancement.
pub struct EnhancerService {
    store: EnhancerStore,
    adapter: Arc<dyn IdentificationEnhancer>,
    breaker_open_until: Arc<Mutex<Option<Instant>>>,
}

impl EnhancerService {
    #[must_use]
    pub fn new(
        pool: SqlitePool,
        adapter: Arc<dyn IdentificationEnhancer>,
        notifier: OutboxNotifier,
    ) -> Self {
        Self {
            store: EnhancerStore::new(pool, notifier),
            adapter,
            breaker_open_until: Arc::new(Mutex::new(None)),
        }
    }

    /// Read the redacted singleton configuration.
    ///
    /// # Errors
    ///
    /// Returns a persistence or timestamp error.
    pub async fn get(&self) -> Result<EnhancerConfigView, AppError> {
        project(self.store.get().await?)
    }

    /// Probe a validated candidate without changing persistent state.
    ///
    /// # Errors
    ///
    /// Invalid local-only configuration returns validation failure; protocol failures are data.
    pub async fn test(
        &self,
        input: EnhancerConfigInput,
    ) -> Result<EnhancerConnectionTestResult, AppError> {
        let input = input.validate()?;
        let now_us = chrono::Utc::now().timestamp_micros();
        if !input.enabled {
            return connection_result(Err(EnhancerError::Disabled), now_us);
        }
        let result = self.adapter.probe(input.endpoint()).await;
        connection_result(result, now_us)
    }

    /// Replace the configuration and persist an immediate safe probe projection when enabled.
    ///
    /// # Errors
    ///
    /// Returns validation, version-conflict, outbox, persistence, or timestamp errors.
    pub async fn replace(
        &self,
        expected_version: i64,
        input: EnhancerConfigInput,
    ) -> Result<EnhancerConfigView, AppError> {
        let record = self
            .store
            .replace(
                expected_version,
                input,
                chrono::Utc::now().timestamp_micros(),
            )
            .await?;
        if !record.enabled {
            return project(record);
        }
        let result = self.adapter.probe(record.endpoint()).await;
        let projection = projection_from_probe(&result);
        let record = self
            .store
            .commit_probe(
                record.config_version,
                projection,
                chrono::Utc::now().timestamp_micros(),
            )
            .await?;
        project(record)
    }

    /// Run the configured local enhancer and persist only its safe health category.
    ///
    /// # Errors
    ///
    /// Returns disabled/protocol errors, or a stable unavailable error for unreadable state.
    pub async fn enhance(
        &self,
        input: &EnhancementInput,
    ) -> Result<EnhancementHints, EnhancerError> {
        if self.breaker_is_open()? {
            return Err(EnhancerError::Overloaded);
        }
        let record = self
            .store
            .get()
            .await
            .map_err(|_| EnhancerError::Unavailable)?;
        if !record.enabled {
            return Err(EnhancerError::Disabled);
        }
        let result = self.adapter.enhance(record.endpoint(), input).await;
        self.update_breaker(&result)?;
        let projection = match &result {
            Ok(_) => EnhancerHealthProjection {
                health: IntegrationHealth::Healthy,
                fallback_code: None,
            },
            Err(error) => projection_from_error(*error),
        };
        self.store
            .commit_probe(
                record.config_version,
                projection,
                chrono::Utc::now().timestamp_micros(),
            )
            .await
            .map_err(|_| EnhancerError::Unavailable)?;
        result
    }

    fn breaker_is_open(&self) -> Result<bool, EnhancerError> {
        let mut state = self
            .breaker_open_until
            .lock()
            .map_err(|_| EnhancerError::Unavailable)?;
        if state.is_some_and(|until| until > Instant::now()) {
            return Ok(true);
        }
        *state = None;
        Ok(false)
    }

    fn update_breaker(
        &self,
        result: &Result<EnhancementHints, EnhancerError>,
    ) -> Result<(), EnhancerError> {
        let mut state = self
            .breaker_open_until
            .lock()
            .map_err(|_| EnhancerError::Unavailable)?;
        match result {
            Err(EnhancerError::Overloaded) => {
                *state = Some(Instant::now() + Duration::from_secs(30));
            }
            Ok(_) => *state = None,
            Err(_) => {}
        }
        Ok(())
    }
}

fn connection_result(
    result: Result<super::port::EnhancerProbe, EnhancerError>,
    now_us: i64,
) -> Result<EnhancerConnectionTestResult, AppError> {
    let checked_at = format_time(now_us)?;
    Ok(match result {
        Ok(probe) if probe.model_available => EnhancerConnectionTestResult {
            reachable: true,
            health: IntegrationHealth::Healthy,
            adapter_version: probe.adapter_version,
            model_available: true,
            fallback_code: None,
            checked_at,
        },
        Ok(probe) => EnhancerConnectionTestResult {
            reachable: true,
            health: IntegrationHealth::Degraded,
            adapter_version: probe.adapter_version,
            model_available: false,
            fallback_code: Some(AutomationFailureCode::IntegrationNotConfigured),
            checked_at,
        },
        Err(error) => {
            let projection = projection_from_error(error);
            EnhancerConnectionTestResult {
                reachable: false,
                health: projection.health,
                adapter_version: ADAPTER_VERSION.to_owned(),
                model_available: false,
                fallback_code: projection.fallback_code,
                checked_at,
            }
        }
    })
}

fn projection_from_probe(
    result: &Result<super::port::EnhancerProbe, EnhancerError>,
) -> EnhancerHealthProjection {
    match result {
        Ok(probe) if probe.model_available => EnhancerHealthProjection {
            health: IntegrationHealth::Healthy,
            fallback_code: None,
        },
        Ok(_) => projection_from_error(EnhancerError::ModelUnavailable),
        Err(error) => projection_from_error(*error),
    }
}

const fn projection_from_error(error: EnhancerError) -> EnhancerHealthProjection {
    match error {
        EnhancerError::Disabled => EnhancerHealthProjection {
            health: IntegrationHealth::Degraded,
            fallback_code: Some(AutomationFailureCode::SourceDisabled),
        },
        EnhancerError::ModelUnavailable => EnhancerHealthProjection {
            health: IntegrationHealth::Degraded,
            fallback_code: Some(AutomationFailureCode::IntegrationNotConfigured),
        },
        EnhancerError::Overloaded => EnhancerHealthProjection {
            health: IntegrationHealth::RateLimited,
            fallback_code: Some(AutomationFailureCode::IntegrationRateLimited),
        },
        EnhancerError::Unavailable => EnhancerHealthProjection {
            health: IntegrationHealth::Unavailable,
            fallback_code: Some(AutomationFailureCode::IntegrationUnavailable),
        },
        EnhancerError::Timeout => EnhancerHealthProjection {
            health: IntegrationHealth::Unavailable,
            fallback_code: Some(AutomationFailureCode::ProviderTimeout),
        },
        EnhancerError::ResponseTooLarge => EnhancerHealthProjection {
            health: IntegrationHealth::Unavailable,
            fallback_code: Some(AutomationFailureCode::ResponseTooLarge),
        },
        EnhancerError::InvalidResponse => EnhancerHealthProjection {
            health: IntegrationHealth::Unavailable,
            fallback_code: Some(AutomationFailureCode::InvalidResponse),
        },
    }
}

fn project(record: EnhancerConfigRecord) -> Result<EnhancerConfigView, AppError> {
    Ok(EnhancerConfigView {
        kind: "ollama",
        enabled: record.enabled,
        endpoint_summary: record.endpoint_summary,
        model: record.model,
        timeout_ms: record.timeout_ms,
        config_version: record.config_version,
        health: record.health,
        checked_at: record.checked_at_us.map(format_time).transpose()?,
        fallback_code: record.fallback_code,
        projection_version: record.projection_version,
        updated_at: format_time(record.updated_at_us)?,
    })
}

fn format_time(value: i64) -> Result<String, AppError> {
    chrono::DateTime::from_timestamp_micros(value)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true))
        .ok_or_else(|| AppError::new(ErrorCode::Internal, "invalid enhancer timestamp"))
}
