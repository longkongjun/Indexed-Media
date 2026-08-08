use sqlx::SqlitePool;

use crate::automation::model::AutomationFailureCode;
use crate::connectors::model::IntegrationHealth;
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::events::{IDENTIFICATION_ENHANCER_CHANGED, IdentificationEnhancerChangedPayload};

use super::model::{EnhancerConfigInput, EnhancerConfigRecord, EnhancerHealthProjection};

type EnhancerRow = (
    Vec<u8>,
    i64,
    String,
    String,
    String,
    i64,
    i64,
    String,
    Option<i64>,
    Option<String>,
    i64,
    i64,
);

#[derive(Clone)]
/// Persists the singleton local enhancer configuration and safe health projection.
pub struct EnhancerStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl EnhancerStore {
    #[must_use]
    pub const fn new(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// Read the mandatory migration-seeded singleton row.
    ///
    /// # Errors
    ///
    /// Returns an internal error for missing, corrupt, or unreadable state.
    pub async fn get(&self) -> Result<EnhancerConfigRecord, AppError> {
        let row = sqlx::query_as::<_, EnhancerRow>(
            "SELECT id,enabled,base_url,endpoint_summary,model,timeout_ms,config_version,health,
                    checked_at_us,fallback_code,projection_version,updated_at_us
             FROM identification_enhancer WHERE singleton_key=1",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?;
        decode(row)
    }

    /// Replace the whole configuration under optimistic concurrency.
    ///
    /// # Errors
    ///
    /// Returns validation, version-conflict, outbox, or persistence errors.
    pub async fn replace(
        &self,
        expected_version: i64,
        input: EnhancerConfigInput,
        now_us: i64,
    ) -> Result<EnhancerConfigRecord, AppError> {
        if expected_version < 1 {
            return Err(version_conflict());
        }
        let input = input.validate()?;
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(version_conflict)?;
        let (health, fallback) = initial_health(input.enabled);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let rows = sqlx::query(
            "UPDATE identification_enhancer SET enabled=?,base_url=?,endpoint_summary=?,model=?,
             timeout_ms=?,config_version=?,health=?,checked_at_us=NULL,fallback_code=?,
             projection_version=projection_version+1,updated_at_us=?
             WHERE singleton_key=1 AND config_version=?",
        )
        .bind(i64::from(input.enabled))
        .bind(input.base_url)
        .bind(input.endpoint_summary)
        .bind(input.model)
        .bind(i64::from(input.timeout_ms))
        .bind(next_version)
        .bind(health_value(health))
        .bind(fallback.map(AutomationFailureCode::as_str))
        .bind(now_us)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        let record = fetch_in_tx(&mut tx).await?;
        write_event(&mut tx, &record, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(record)
    }

    /// Commit one safe probe result when the configuration version is still current.
    ///
    /// # Errors
    ///
    /// Returns invalid projection, version-conflict, outbox, or persistence errors.
    pub async fn commit_probe(
        &self,
        expected_version: i64,
        projection: EnhancerHealthProjection,
        now_us: i64,
    ) -> Result<EnhancerConfigRecord, AppError> {
        validate_projection(projection)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let rows = sqlx::query(
            "UPDATE identification_enhancer SET health=?,fallback_code=?,checked_at_us=?,
             projection_version=projection_version+1,updated_at_us=?
             WHERE singleton_key=1 AND enabled=1 AND config_version=?",
        )
        .bind(health_value(projection.health))
        .bind(projection.fallback_code.map(AutomationFailureCode::as_str))
        .bind(now_us)
        .bind(now_us)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        let record = fetch_in_tx(&mut tx).await?;
        write_event(&mut tx, &record, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(record)
    }
}

async fn fetch_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<EnhancerConfigRecord, AppError> {
    sqlx::query_as::<_, EnhancerRow>(
        "SELECT id,enabled,base_url,endpoint_summary,model,timeout_ms,config_version,health,
                checked_at_us,fallback_code,projection_version,updated_at_us
         FROM identification_enhancer WHERE singleton_key=1",
    )
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)
    .and_then(decode)
}

async fn write_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    record: &EnhancerConfigRecord,
    now_us: i64,
) -> Result<(), AppError> {
    OutboxWriter::write(
        tx,
        IDENTIFICATION_ENHANCER_CHANGED,
        record.id,
        &IdentificationEnhancerChangedPayload {
            projection_version: record.projection_version,
            enabled: record.enabled,
            health: record.health,
            fallback_code: record.fallback_code,
        },
        now_us,
    )
    .await
    .map(|_| ())
}

fn decode(row: EnhancerRow) -> Result<EnhancerConfigRecord, AppError> {
    Ok(EnhancerConfigRecord {
        id: uuid::Uuid::from_slice(&row.0).map_err(invalid)?,
        enabled: bool_value(row.1)?,
        base_url: row.2,
        endpoint_summary: row.3,
        model: row.4,
        timeout_ms: u32::try_from(row.5).map_err(invalid)?,
        config_version: row.6,
        health: parse_health(&row.7).ok_or_else(|| invalid("invalid enhancer health"))?,
        checked_at_us: row.8,
        fallback_code: row
            .9
            .as_deref()
            .map(|value| {
                AutomationFailureCode::parse(value)
                    .ok_or_else(|| invalid("invalid enhancer fallback code"))
            })
            .transpose()?,
        projection_version: row.10,
        updated_at_us: row.11,
    })
}

fn initial_health(enabled: bool) -> (IntegrationHealth, Option<AutomationFailureCode>) {
    if enabled {
        (
            IntegrationHealth::Degraded,
            Some(AutomationFailureCode::IntegrationUnavailable),
        )
    } else {
        (
            IntegrationHealth::Degraded,
            Some(AutomationFailureCode::SourceDisabled),
        )
    }
}

fn validate_projection(projection: EnhancerHealthProjection) -> Result<(), AppError> {
    let valid = match projection.health {
        IntegrationHealth::Healthy => projection.fallback_code.is_none(),
        IntegrationHealth::Degraded
        | IntegrationHealth::Unavailable
        | IntegrationHealth::RateLimited => projection.fallback_code.is_some(),
        IntegrationHealth::Unconfigured | IntegrationHealth::Unauthorized => false,
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorCode::Internal,
            "invalid enhancer health projection",
        ))
    }
}

fn bool_value(value: i64) -> Result<bool, AppError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid("invalid enhancer boolean")),
    }
}

fn parse_health(value: &str) -> Option<IntegrationHealth> {
    match value {
        "healthy" => Some(IntegrationHealth::Healthy),
        "degraded" => Some(IntegrationHealth::Degraded),
        "unavailable" => Some(IntegrationHealth::Unavailable),
        "rate-limited" => Some(IntegrationHealth::RateLimited),
        _ => None,
    }
}

const fn health_value(value: IntegrationHealth) -> &'static str {
    match value {
        IntegrationHealth::Healthy => "healthy",
        IntegrationHealth::Degraded => "degraded",
        IntegrationHealth::Unavailable => "unavailable",
        IntegrationHealth::RateLimited => "rate-limited",
        IntegrationHealth::Unconfigured => "unconfigured",
        IntegrationHealth::Unauthorized => "unauthorized",
    }
}

fn version_conflict() -> AppError {
    AppError::new(
        ErrorCode::ConfigVersionConflict,
        "identification enhancer version changed",
    )
}

fn database_error(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}
