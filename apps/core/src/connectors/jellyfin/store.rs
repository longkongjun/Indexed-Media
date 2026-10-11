//! Jellyfin 连接配置的 `SQLite` 存储：加密凭据 + 乐观并发 + 健康投影。

use sqlx::Row as _;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::connectors::jellyfin::model::JellyfinIntegrationView;
use crate::connectors::model::{IntegrationFailureCode, IntegrationHealth};
use crate::platform::secrets::SealedSecret;
use crate::shared::error::{AppError, ErrorCode};

pub(crate) struct JellyfinRecord {
    pub id: Uuid,
    pub base_url: String,
    pub display_name: Option<String>,
    pub sealed: Option<SealedSecret>,
    pub config_version: i64,
    pub health: IntegrationHealth,
    pub failure_code: Option<IntegrationFailureCode>,
    pub server_version: Option<String>,
    pub checked_at_us: Option<i64>,
}

#[derive(Clone)]
pub(crate) struct JellyfinStore {
    pool: SqlitePool,
}

impl JellyfinStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub(crate) async fn load(&self) -> Result<Option<JellyfinRecord>, AppError> {
        let row = sqlx::query(
            "SELECT id,base_url,display_name,secret_schema_version,secret_nonce,secret_ciphertext,
                    config_version,health,failure_code,server_version,checked_at_us
             FROM connectors_jellyfin LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(database)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let health: String = row.try_get("health").map_err(database)?;
        let health = parse_health(&health)?;
        let failure_code: Option<String> = row.try_get("failure_code").map_err(database)?;
        let failure_code = failure_code
            .map(|code| parse_failure_code(&code))
            .transpose()?;
        let sealed = decode_sealed(&row)?;
        Ok(Some(JellyfinRecord {
            id: Uuid::from_slice(
                row.try_get::<Vec<u8>, _>("id")
                    .map_err(database)?
                    .as_slice(),
            )
            .map_err(bad_uuid)?,
            base_url: row.try_get("base_url").map_err(database)?,
            display_name: row.try_get("display_name").map_err(database)?,
            sealed,
            config_version: row.try_get("config_version").map_err(database)?,
            health,
            failure_code,
            server_version: row.try_get("server_version").map_err(database)?,
            checked_at_us: row.try_get("checked_at_us").map_err(database)?,
        }))
    }

    pub(crate) async fn save(
        &self,
        id: Uuid,
        expected_version: i64,
        base_url: &str,
        display_name: Option<&str>,
        sealed: &SealedSecret,
        now_us: i64,
    ) -> Result<i64, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        let rows = if expected_version == 0 {
            sqlx::query(
                "INSERT INTO connectors_jellyfin
                 (id,base_url,display_name,secret_schema_version,secret_nonce,secret_ciphertext,
                  config_version,health,failure_code,server_version,checked_at_us,
                  created_at_us,updated_at_us)
                 VALUES (?,?,?,?,?,?,1,'degraded',NULL,NULL,NULL,?,?)",
            )
            .bind(id.as_bytes().to_vec())
            .bind(base_url)
            .bind(display_name)
            .bind(i64::from(sealed.schema_version()))
            .bind(sealed.nonce().to_vec())
            .bind(sealed.ciphertext().to_vec())
            .bind(now_us)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(database)?
        } else {
            sqlx::query(
                "UPDATE connectors_jellyfin
                 SET base_url=?,display_name=?,secret_schema_version=?,secret_nonce=?,
                     secret_ciphertext=?,config_version=config_version+1,
                     health='degraded',failure_code=NULL,server_version=NULL,checked_at_us=NULL,
                     updated_at_us=?
                 WHERE config_version=?",
            )
            .bind(base_url)
            .bind(display_name)
            .bind(i64::from(sealed.schema_version()))
            .bind(sealed.nonce().to_vec())
            .bind(sealed.ciphertext().to_vec())
            .bind(now_us)
            .bind(expected_version)
            .execute(&mut *tx)
            .await
            .map_err(database)?
        };
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "jellyfin config version changed",
            ));
        }
        let version =
            sqlx::query_scalar::<_, i64>("SELECT config_version FROM connectors_jellyfin LIMIT 1")
                .fetch_one(&mut *tx)
                .await
                .map_err(database)?;
        tx.commit().await.map_err(database)?;
        Ok(version)
    }

    pub(crate) async fn delete(&self, expected_version: i64, now_us: i64) -> Result<(), AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database)?;
        let rows = sqlx::query(
            "UPDATE connectors_jellyfin
             SET base_url='',display_name=NULL,secret_schema_version=NULL,secret_nonce=NULL,
                 secret_ciphertext=NULL,health='unconfigured',failure_code=NULL,
                 server_version=NULL,checked_at_us=NULL,config_version=config_version+1,
                 updated_at_us=?
             WHERE config_version=? AND secret_ciphertext IS NOT NULL",
        )
        .bind(now_us)
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(database)?;
        if rows.rows_affected() != 1 {
            return Err(AppError::new(
                ErrorCode::ConfigVersionConflict,
                "jellyfin config version changed",
            ));
        }
        tx.commit().await.map_err(database)?;
        Ok(())
    }

    pub(crate) async fn record_health(
        &self,
        health: IntegrationHealth,
        failure_code: Option<IntegrationFailureCode>,
        server_version: Option<&str>,
        now_us: i64,
    ) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE connectors_jellyfin
             SET health=?,failure_code=?,server_version=?,checked_at_us=?,updated_at_us=?
             WHERE secret_ciphertext IS NOT NULL",
        )
        .bind(
            serde_json::to_value(health)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "degraded".to_owned()),
        )
        .bind(failure_code.map(|code| code.as_str().to_owned()))
        .bind(server_version)
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(database)?;
        Ok(())
    }
}

pub(crate) fn project(record: &JellyfinRecord) -> JellyfinIntegrationView {
    JellyfinIntegrationView {
        kind: "jellyfin",
        configured: record.sealed.is_some(),
        base_url: if record.sealed.is_some() {
            Some(record.base_url.clone())
        } else {
            None
        },
        display_name: if record.sealed.is_some() {
            record.display_name.clone()
        } else {
            None
        },
        server_version: record.server_version.clone(),
        config_version: record.config_version,
        health: record.health,
        failure_code: record.failure_code,
        checked_at: record
            .checked_at_us
            .and_then(|us| chrono::TimeZone::timestamp_micros(&chrono::Utc, us).single())
            .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)),
    }
}

fn decode_sealed(row: &sqlx::sqlite::SqliteRow) -> Result<Option<SealedSecret>, AppError> {
    let schema_version: Option<i64> = row.try_get("secret_schema_version").map_err(database)?;
    let nonce: Option<Vec<u8>> = row.try_get("secret_nonce").map_err(database)?;
    let ciphertext: Option<Vec<u8>> = row.try_get("secret_ciphertext").map_err(database)?;
    match (schema_version, nonce, ciphertext) {
        (None, None, None) => Ok(None),
        (Some(schema), Some(nonce), Some(ciphertext)) => SealedSecret::from_parts(
            u16::try_from(schema).map_err(|_| bad_state())?,
            nonce.try_into().map_err(|_| bad_state())?,
            ciphertext,
        )
        .map(Some)
        .map_err(|_| bad_state()),
        _ => Err(bad_state()),
    }
}

fn parse_health(value: &str) -> Result<IntegrationHealth, AppError> {
    match value {
        "unconfigured" => Ok(IntegrationHealth::Unconfigured),
        "healthy" => Ok(IntegrationHealth::Healthy),
        "degraded" => Ok(IntegrationHealth::Degraded),
        "unavailable" => Ok(IntegrationHealth::Unavailable),
        "unauthorized" => Ok(IntegrationHealth::Unauthorized),
        _ => Err(bad_state()),
    }
}

fn parse_failure_code(value: &str) -> Result<IntegrationFailureCode, AppError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(|_| bad_state())
}

fn bad_state() -> AppError {
    AppError::new(
        ErrorCode::DatabaseInvalid,
        "stored jellyfin record is invalid",
    )
}

fn bad_uuid(error: uuid::Error) -> AppError {
    AppError::with_source(ErrorCode::DatabaseInvalid, error)
}

fn database(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
