use sqlx::SqlitePool;
use uuid::Uuid;

use crate::connectors::model::{IntegrationFailureCode, IntegrationHealth};
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::platform::secrets::SealedSecret;
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::events::{INTEGRATION_HEALTH_CHANGED, IntegrationHealthChangedPayload};

type TmdbRow = (
    Vec<u8>,
    String,
    Option<String>,
    Option<i64>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    i64,
    String,
    Option<String>,
    Option<i64>,
);

pub(crate) struct TmdbIntegrationRecord {
    pub id: Uuid,
    pub locale: String,
    pub region: Option<String>,
    pub sealed: Option<SealedSecret>,
    pub config_version: i64,
    pub health: IntegrationHealth,
    pub failure_code: Option<IntegrationFailureCode>,
    pub checked_at_us: Option<i64>,
}

pub(crate) struct TmdbCacheRecord {
    pub response_json: Option<Vec<u8>>,
    pub fresh_until_us: i64,
}

#[derive(Clone)]
pub(crate) struct ConnectorStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl ConnectorStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    pub const fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    pub async fn load_tmdb(&self) -> Result<Option<TmdbIntegrationRecord>, AppError> {
        let row = sqlx::query_as::<_, TmdbRow>(
            "SELECT id,locale,region,secret_schema_version,secret_nonce,secret_ciphertext,
                    config_version,health,failure_code,checked_at_us
             FROM connectors_integrations WHERE kind='tmdb'",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        row.map(decode_record).transpose()
    }

    pub async fn save_tmdb(
        &self,
        id: Uuid,
        expected_version: i64,
        locale: &str,
        region: Option<&str>,
        sealed: &SealedSecret,
        now_us: i64,
    ) -> Result<i64, AppError> {
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(version_conflict)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let rows = if expected_version == 0 {
            sqlx::query(
                "INSERT INTO connectors_integrations
                 (id,kind,locale,region,secret_schema_version,secret_nonce,secret_ciphertext,
                  config_version,health,failure_code,checked_at_us,created_at_us,updated_at_us)
                 VALUES (?,'tmdb',?,?,?,?,?,?,'degraded','integration.unavailable',NULL,?,?)",
            )
            .bind(id.as_bytes().as_slice())
            .bind(locale)
            .bind(region)
            .bind(i64::from(sealed.schema_version()))
            .bind(sealed.nonce().as_slice())
            .bind(sealed.ciphertext())
            .bind(next_version)
            .bind(now_us)
            .bind(now_us)
            .execute(&mut *tx)
            .await
        } else {
            sqlx::query(
                "UPDATE connectors_integrations
                 SET locale=?,region=?,secret_schema_version=?,secret_nonce=?,secret_ciphertext=?,
                     config_version=?,health='degraded',failure_code='integration.unavailable',
                     checked_at_us=NULL,updated_at_us=?
                 WHERE kind='tmdb' AND config_version=?",
            )
            .bind(locale)
            .bind(region)
            .bind(i64::from(sealed.schema_version()))
            .bind(sealed.nonce().as_slice())
            .bind(sealed.ciphertext())
            .bind(next_version)
            .bind(now_us)
            .bind(expected_version)
            .execute(&mut *tx)
            .await
        };
        let rows = rows.map_err(|error| {
            if is_constraint(&error) {
                version_conflict()
            } else {
                database_error(error)
            }
        })?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        sqlx::query(
            "UPDATE tasks_processing_tasks
             SET next_retry_at_us=?,updated_at_us=?,version=version+1
             WHERE status='paused' AND stage='identification'
               AND reason IN ('identification.provider-unavailable',
                              'identification.provider-unauthorized')",
        )
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        OutboxWriter::write(
            &mut tx,
            INTEGRATION_HEALTH_CHANGED,
            id,
            &IntegrationHealthChangedPayload {
                kind: "tmdb".to_owned(),
                health: IntegrationHealth::Degraded,
                failure_code: Some(IntegrationFailureCode::Unavailable),
            },
            now_us,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(next_version)
    }

    pub async fn delete_tmdb(&self, expected_version: i64, now_us: i64) -> Result<i64, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let current = sqlx::query_as::<_, (Vec<u8>, i64)>(
            "SELECT id,config_version FROM connectors_integrations WHERE kind='tmdb'",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let Some((id, actual_version)) = current else {
            if expected_version == 0 {
                tx.rollback().await.map_err(database_error)?;
                return Ok(0);
            }
            return Err(version_conflict());
        };
        if actual_version != expected_version {
            return Err(version_conflict());
        }
        let id = Uuid::from_slice(&id).map_err(invalid_database)?;
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(version_conflict)?;
        let rows = sqlx::query(
            "UPDATE connectors_integrations
             SET secret_schema_version=NULL,secret_nonce=NULL,secret_ciphertext=NULL,
                 config_version=?,health='unconfigured',failure_code='integration.not-configured',
                 checked_at_us=NULL,updated_at_us=?
             WHERE kind='tmdb' AND config_version=?",
        )
        .bind(next_version)
        .bind(now_us)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        OutboxWriter::write(
            &mut tx,
            INTEGRATION_HEALTH_CHANGED,
            id,
            &IntegrationHealthChangedPayload {
                kind: "tmdb".to_owned(),
                health: IntegrationHealth::Unconfigured,
                failure_code: Some(IntegrationFailureCode::NotConfigured),
            },
            now_us,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(next_version)
    }

    pub async fn update_tmdb_health(
        &self,
        health: IntegrationHealth,
        failure_code: Option<IntegrationFailureCode>,
        checked_at_us: i64,
    ) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let current = sqlx::query_as::<_, (Vec<u8>, Option<Vec<u8>>, String, Option<String>)>(
            "SELECT id,secret_ciphertext,health,failure_code
             FROM connectors_integrations WHERE kind='tmdb'",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let Some((id, secret, current_health, current_failure)) = current else {
            tx.rollback().await.map_err(database_error)?;
            return Ok(());
        };
        let (health, failure_code) = if secret.is_none() {
            (
                IntegrationHealth::Unconfigured,
                Some(IntegrationFailureCode::NotConfigured),
            )
        } else {
            (health, failure_code)
        };
        let next_health = health_value(health);
        let next_failure = failure_code.map(IntegrationFailureCode::as_str);
        sqlx::query(
            "UPDATE connectors_integrations
             SET health=?,failure_code=?,checked_at_us=?,updated_at_us=? WHERE kind='tmdb'",
        )
        .bind(next_health)
        .bind(next_failure)
        .bind(checked_at_us)
        .bind(checked_at_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if current_health != next_health || current_failure.as_deref() != next_failure {
            let id = Uuid::from_slice(&id).map_err(invalid_database)?;
            OutboxWriter::write(
                &mut tx,
                INTEGRATION_HEALTH_CHANGED,
                id,
                &IntegrationHealthChangedPayload {
                    kind: "tmdb".to_owned(),
                    health,
                    failure_code,
                },
                checked_at_us,
            )
            .await?;
        }
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(())
    }

    pub async fn load_tmdb_cache(
        &self,
        query_key: &[u8; 32],
        now_us: i64,
    ) -> Result<Option<TmdbCacheRecord>, AppError> {
        let row = sqlx::query_as::<_, (Option<Vec<u8>>, i64)>(
            "SELECT response_json,fresh_until_us FROM connectors_tmdb_cache
             WHERE query_key=? AND stale_until_us>?",
        )
        .bind(query_key.as_slice())
        .bind(now_us)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(row.map(|(response_json, fresh_until_us)| TmdbCacheRecord {
            response_json,
            fresh_until_us,
        }))
    }

    pub async fn save_tmdb_cache(
        &self,
        query_key: &[u8; 32],
        response_json: Option<&[u8]>,
        fresh_until_us: i64,
        stale_until_us: i64,
        now_us: i64,
    ) -> Result<(), AppError> {
        let outcome = if response_json.is_some() {
            "found"
        } else {
            "not-found"
        };
        sqlx::query(
            "INSERT INTO connectors_tmdb_cache
             (query_key,provider_schema_version,outcome,response_json,fresh_until_us,
              stale_until_us,created_at_us,updated_at_us)
             VALUES (?,1,?,?,?,?,?,?)
             ON CONFLICT(query_key) DO UPDATE SET
               provider_schema_version=excluded.provider_schema_version,
               outcome=excluded.outcome,response_json=excluded.response_json,
               fresh_until_us=excluded.fresh_until_us,stale_until_us=excluded.stale_until_us,
               updated_at_us=excluded.updated_at_us",
        )
        .bind(query_key.as_slice())
        .bind(outcome)
        .bind(response_json)
        .bind(fresh_until_us)
        .bind(stale_until_us)
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(())
    }
}

fn decode_record(row: TmdbRow) -> Result<TmdbIntegrationRecord, AppError> {
    let (id, locale, region, schema, nonce, ciphertext, config_version, health, failure, checked) =
        row;
    let id = Uuid::from_slice(&id).map_err(invalid_database)?;
    let sealed = match (schema, nonce, ciphertext) {
        (None, None, None) => None,
        (Some(schema), Some(nonce), Some(ciphertext)) => {
            let schema = u16::try_from(schema).map_err(invalid_database)?;
            let nonce: [u8; 24] = nonce.try_into().map_err(|_| invalid_database("nonce"))?;
            Some(SealedSecret::from_parts(schema, nonce, ciphertext).map_err(invalid_database)?)
        }
        _ => return Err(invalid_database("partial secret record")),
    };
    let failure_code = match failure {
        None => None,
        Some(value) => Some(
            IntegrationFailureCode::parse(&value)
                .ok_or_else(|| invalid_database("failure code"))?,
        ),
    };
    Ok(TmdbIntegrationRecord {
        id,
        locale,
        region,
        sealed,
        config_version,
        health: parse_health(&health).ok_or_else(|| invalid_database("health"))?,
        failure_code,
        checked_at_us: checked,
    })
}

fn parse_health(value: &str) -> Option<IntegrationHealth> {
    match value {
        "unconfigured" => Some(IntegrationHealth::Unconfigured),
        "healthy" => Some(IntegrationHealth::Healthy),
        "degraded" => Some(IntegrationHealth::Degraded),
        "unavailable" => Some(IntegrationHealth::Unavailable),
        "unauthorized" => Some(IntegrationHealth::Unauthorized),
        "rate-limited" => Some(IntegrationHealth::RateLimited),
        _ => None,
    }
}

const fn health_value(health: IntegrationHealth) -> &'static str {
    match health {
        IntegrationHealth::Unconfigured => "unconfigured",
        IntegrationHealth::Healthy => "healthy",
        IntegrationHealth::Degraded => "degraded",
        IntegrationHealth::Unavailable => "unavailable",
        IntegrationHealth::Unauthorized => "unauthorized",
        IntegrationHealth::RateLimited => "rate-limited",
    }
}

fn is_constraint(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.is_unique_violation())
}

fn database_error(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid_database(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}

fn version_conflict() -> AppError {
    AppError::new(
        ErrorCode::ConfigVersionConflict,
        "connector config version changed",
    )
}
