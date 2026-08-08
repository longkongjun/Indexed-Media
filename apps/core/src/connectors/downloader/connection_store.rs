use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::connectors::model::IntegrationHealth;
use crate::platform::secrets::{InstanceKey, SealedSecret, SecretAad, SecretCipher as _};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

use super::model::{
    DownloaderCapabilities, DownloaderConnection, DownloaderConnectionInput,
    DownloaderConnectionProbe, DownloaderFailureCode, DownloaderKind, LoadedDownloaderCredentials,
    SECRET_SCHEMA_VERSION,
};

type ConnectionRow = (
    Vec<u8>,
    String,
    String,
    String,
    i64,
    i64,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<String>,
    Option<String>,
    i64,
);

type SecretRow = (String, i64, i64, Vec<u8>, Vec<u8>);

const CONNECTION_COLUMNS: &str =
    "id,kind,display_name,base_url,enabled,config_version,health,failure_code,
     checked_at_us,manual_add,task_monitoring,product_version,api_version,updated_at_us";

#[derive(Deserialize, Serialize)]
struct ConnectionCursor {
    version: u8,
    updated_at_us: i64,
    id: Uuid,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

#[derive(Clone)]
/// 下载器连接配置、密文和公开探测投影的 `SQLite` 边界。
pub struct DownloaderConnectionStore {
    pool: SqlitePool,
    key: Arc<InstanceKey>,
}

impl DownloaderConnectionStore {
    /// 打开 store 并加载当前实例的认证加密密钥。
    ///
    /// # Errors
    ///
    /// 实例密钥不满足文件边界时返回配置错误。
    pub fn open(pool: SqlitePool, config_dir: &Path) -> Result<Self, AppError> {
        let key = InstanceKey::load_or_create(config_dir)
            .map(Arc::new)
            .map_err(|error| AppError::with_source(ErrorCode::ConfigInvalid, error))?;
        Ok(Self { pool, key })
    }

    /// 按最近更新时间和 UUID 的稳定顺序列出所有脱敏连接。
    ///
    /// # Errors
    ///
    /// 数据库不可用或持久行不符合领域约束时返回内部错误。
    pub async fn list(&self) -> Result<Vec<DownloaderConnection>, AppError> {
        let query = format!(
            "SELECT {CONNECTION_COLUMNS} FROM downloader_connections
             ORDER BY updated_at_us DESC,id DESC"
        );
        sqlx::query_as::<_, ConnectionRow>(&query)
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?
            .into_iter()
            .map(decode_connection)
            .collect()
    }

    /// 按 `(updated_at_us,id)` 倒序返回有界游标页。
    ///
    /// # Errors
    ///
    /// 页大小、游标、数据库或持久行无效时返回稳定应用错误。
    pub async fn list_page(
        &self,
        page: &PageRequest,
    ) -> Result<CursorPage<DownloaderConnection>, AppError> {
        if page.limit == 0 || page.limit > crate::shared::page::MAX_PAGE_LIMIT {
            return Err(validation_error("downloader page limit is outside bounds"));
        }
        let cursor = page.cursor.as_deref().map(decode_cursor).transpose()?;
        let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
        query
            .push(CONNECTION_COLUMNS)
            .push(" FROM downloader_connections");
        if let Some(cursor) = &cursor {
            query
                .push(" WHERE (updated_at_us<")
                .push_bind(cursor.updated_at_us)
                .push(" OR (updated_at_us=")
                .push_bind(cursor.updated_at_us)
                .push(" AND id<")
                .push_bind(cursor.id.as_bytes().to_vec())
                .push("))");
        }
        query
            .push(" ORDER BY updated_at_us DESC,id DESC LIMIT ")
            .push_bind(i64::from(page.limit) + 1);
        let rows = query
            .build_query_as::<ConnectionRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?;
        let has_more = rows.len() > page.limit as usize;
        let mut items = rows
            .into_iter()
            .take(page.limit as usize)
            .map(decode_connection)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            items
                .last()
                .map(|item| {
                    encode_cursor(&ConnectionCursor {
                        version: 1,
                        updated_at_us: item.updated_at_us,
                        id: item.id,
                    })
                })
                .transpose()?
        } else {
            None
        };
        items.shrink_to_fit();
        Ok(CursorPage { items, next_cursor })
    }

    /// 按本地稳定 ID 读取一项脱敏连接。
    ///
    /// # Errors
    ///
    /// 数据库不可用或持久行不符合领域约束时返回内部错误。
    pub async fn get(&self, id: Uuid) -> Result<Option<DownloaderConnection>, AppError> {
        let query = format!("SELECT {CONNECTION_COLUMNS} FROM downloader_connections WHERE id=?");
        sqlx::query_as::<_, ConnectionRow>(&query)
            .bind(id.as_bytes().as_slice())
            .fetch_optional(&self.pool)
            .await
            .map_err(database_error)?
            .map(decode_connection)
            .transpose()
    }

    /// 校验、加密并插入配置版本为 1 的连接。
    ///
    /// # Errors
    ///
    /// 输入、加密、数据库或 ID 冲突失败时返回稳定应用错误。
    pub async fn insert(
        &self,
        id: Uuid,
        input: DownloaderConnectionInput,
        now_us: i64,
    ) -> Result<DownloaderConnection, AppError> {
        let input = input.validate()?;
        let sealed = self
            .key
            .seal(
                &SecretAad::new(id, input.kind.integration_kind(), 1, SECRET_SCHEMA_VERSION),
                input.credentials.expose(),
            )
            .map_err(secret_error)?;
        let result = sqlx::query(
            "INSERT INTO downloader_connections
             (id,kind,display_name,base_url,enabled,secret_schema_version,secret_nonce,
              secret_ciphertext,config_version,health,failure_code,checked_at_us,
              manual_add,task_monitoring,product_version,api_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,?,?,1,'degraded','integration.unavailable',NULL,
                     NULL,NULL,NULL,NULL,?,?)",
        )
        .bind(id.as_bytes().as_slice())
        .bind(input.kind.as_str())
        .bind(input.display_name)
        .bind(input.base_url)
        .bind(i64::from(input.enabled))
        .bind(i64::from(sealed.schema_version()))
        .bind(sealed.nonce().as_slice())
        .bind(sealed.ciphertext())
        .bind(now_us)
        .bind(now_us)
        .execute(&self.pool)
        .await;
        match result {
            Ok(_) => self
                .get(id)
                .await?
                .ok_or_else(|| invalid_database("inserted downloader connection is missing")),
            Err(error) if is_constraint(&error) => Err(version_conflict()),
            Err(error) => Err(database_error(error)),
        }
    }

    /// 以当前配置版本校验、重新加密并完整替换连接。
    ///
    /// # Errors
    ///
    /// 输入、加密、数据库或乐观并发失败时返回稳定应用错误。
    pub async fn replace(
        &self,
        id: Uuid,
        expected_version: i64,
        input: DownloaderConnectionInput,
        now_us: i64,
    ) -> Result<DownloaderConnection, AppError> {
        if expected_version < 1 {
            return Err(version_conflict());
        }
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(version_conflict)?;
        let input = input.validate()?;
        let sealed = self
            .key
            .seal(
                &SecretAad::new(
                    id,
                    input.kind.integration_kind(),
                    next_version,
                    SECRET_SCHEMA_VERSION,
                ),
                input.credentials.expose(),
            )
            .map_err(secret_error)?;
        let rows = sqlx::query(
            "UPDATE downloader_connections
             SET kind=?,display_name=?,base_url=?,enabled=?,secret_schema_version=?,
                 secret_nonce=?,secret_ciphertext=?,config_version=?,health='degraded',
                 failure_code='integration.unavailable',checked_at_us=NULL,
                 manual_add=NULL,task_monitoring=NULL,product_version=NULL,api_version=NULL,
                 updated_at_us=?
             WHERE id=? AND config_version=?",
        )
        .bind(input.kind.as_str())
        .bind(input.display_name)
        .bind(input.base_url)
        .bind(i64::from(input.enabled))
        .bind(i64::from(sealed.schema_version()))
        .bind(sealed.nonce().as_slice())
        .bind(sealed.ciphertext())
        .bind(next_version)
        .bind(now_us)
        .bind(id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        self.get(id)
            .await?
            .ok_or_else(|| invalid_database("replaced downloader connection is missing"))
    }

    /// 以当前配置版本物理删除本地连接和密文，不触发远端副作用。
    ///
    /// # Errors
    ///
    /// 数据库或乐观并发失败时返回稳定应用错误。
    pub async fn delete(&self, id: Uuid, expected_version: i64) -> Result<(), AppError> {
        if expected_version < 1 {
            return Err(version_conflict());
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let active = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM download_tasks
             WHERE connection_id=? AND status NOT IN ('completed','failed')",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if active != 0 {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "downloader connection has active download tasks",
            ));
        }
        let rows =
            sqlx::query("DELETE FROM downloader_connections WHERE id=? AND config_version=?")
                .bind(id.as_bytes().as_slice())
                .bind(expected_version)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        tx.commit().await.map_err(database_error)
    }

    /// 解密指定连接的凭据，并保持返回值 Debug 脱敏与析构清零。
    ///
    /// # Errors
    ///
    /// 连接不存在、持久密文无效或认证失败时返回稳定应用错误。
    pub async fn load_secret(&self, id: Uuid) -> Result<LoadedDownloaderCredentials, AppError> {
        let row = sqlx::query_as::<_, SecretRow>(
            "SELECT kind,config_version,secret_schema_version,secret_nonce,secret_ciphertext
             FROM downloader_connections WHERE id=?",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "downloader connection not found"))?;
        let (kind, config_version, schema_version, nonce, ciphertext) = row;
        let kind = DownloaderKind::parse(&kind)
            .ok_or_else(|| invalid_database("unknown downloader kind"))?;
        let schema_version = u16::try_from(schema_version)
            .map_err(|_| invalid_database("invalid secret schema version"))?;
        let nonce: [u8; 24] = nonce
            .try_into()
            .map_err(|_| invalid_database("invalid secret nonce"))?;
        let sealed = SealedSecret::from_parts(schema_version, nonce, ciphertext)
            .map_err(|error| invalid_database(error.to_string()))?;
        let plaintext = self
            .key
            .open(
                &SecretAad::new(id, kind.integration_kind(), config_version, schema_version),
                &sealed,
            )
            .map_err(secret_error)?;
        LoadedDownloaderCredentials::decode(plaintext.expose())
    }

    /// 以当前配置版本提交一次健康与能力探测，不改变配置版本。
    ///
    /// # Errors
    ///
    /// 投影、数据库或乐观并发失败时返回稳定应用错误。
    pub async fn commit_probe(
        &self,
        id: Uuid,
        expected_version: i64,
        probe: DownloaderConnectionProbe,
        checked_at_us: i64,
    ) -> Result<(), AppError> {
        validate_probe(&probe)?;
        let (manual_add, task_monitoring, product_version, api_version) = probe
            .capabilities
            .as_ref()
            .map_or((None, None, None, None), |capabilities| {
                (
                    Some(i64::from(capabilities.manual_add)),
                    Some(i64::from(capabilities.task_monitoring)),
                    Some(capabilities.product_version.as_str()),
                    Some(capabilities.api_version.as_str()),
                )
            });
        let rows = sqlx::query(
            "UPDATE downloader_connections
             SET health=?,failure_code=?,checked_at_us=?,manual_add=?,task_monitoring=?,
                 product_version=?,api_version=?,updated_at_us=?
             WHERE id=? AND config_version=?",
        )
        .bind(health_value(probe.health))
        .bind(probe.failure_code.map(DownloaderFailureCode::as_str))
        .bind(checked_at_us)
        .bind(manual_add)
        .bind(task_monitoring)
        .bind(product_version)
        .bind(api_version)
        .bind(checked_at_us)
        .bind(id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        Ok(())
    }
}

fn validate_probe(probe: &DownloaderConnectionProbe) -> Result<(), AppError> {
    let healthy = probe.health == IntegrationHealth::Healthy;
    if matches!(probe.health, IntegrationHealth::Unconfigured)
        || healthy != probe.failure_code.is_none()
        || healthy != probe.capabilities.is_some()
        || probe.failure_code.is_some_and(|code| {
            matches!(
                code,
                DownloaderFailureCode::IntegrationNotConfigured
                    | DownloaderFailureCode::CorrelationAmbiguous
                    | DownloaderFailureCode::RemoteMissing
            )
        })
        || probe.capabilities.as_ref().is_some_and(|capabilities| {
            !(1..=64).contains(&capabilities.product_version.chars().count())
                || !(1..=64).contains(&capabilities.api_version.chars().count())
        })
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid downloader probe projection",
        ));
    }
    Ok(())
}

fn decode_connection(row: ConnectionRow) -> Result<DownloaderConnection, AppError> {
    let (
        id,
        kind,
        display_name,
        base_url,
        enabled,
        config_version,
        health,
        failure_code,
        checked_at_us,
        manual_add,
        task_monitoring,
        product_version,
        api_version,
        updated_at_us,
    ) = row;
    let capabilities = match (manual_add, task_monitoring, product_version, api_version) {
        (None, None, None, None) => None,
        (Some(manual_add), Some(task_monitoring), Some(product_version), Some(api_version)) => {
            Some(DownloaderCapabilities {
                manual_add: decode_bool(manual_add)?,
                task_monitoring: decode_bool(task_monitoring)?,
                product_version,
                api_version,
            })
        }
        _ => return Err(invalid_database("partial downloader capabilities")),
    };
    Ok(DownloaderConnection {
        id: Uuid::from_slice(&id).map_err(invalid_database)?,
        kind: DownloaderKind::parse(&kind)
            .ok_or_else(|| invalid_database("unknown downloader kind"))?,
        display_name,
        base_url,
        enabled: decode_bool(enabled)?,
        config_version,
        capabilities,
        health: parse_health(&health).ok_or_else(|| invalid_database("unknown health"))?,
        failure_code: failure_code
            .map(|value| {
                DownloaderFailureCode::parse(&value)
                    .ok_or_else(|| invalid_database("unknown downloader failure code"))
            })
            .transpose()?,
        checked_at_us,
        updated_at_us,
    })
}

fn decode_bool(value: i64) -> Result<bool, AppError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid_database("invalid SQLite boolean")),
    }
}

fn parse_health(value: &str) -> Option<IntegrationHealth> {
    match value {
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

fn secret_error(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid_database(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}

fn version_conflict() -> AppError {
    AppError::new(
        ErrorCode::ConfigVersionConflict,
        "downloader connection version changed",
    )
}

fn encode_cursor(cursor: &ConnectionCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(invalid_database)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(invalid_database)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_database("downloader cursor exceeds bounds"));
    }
    Ok(encoded)
}

fn decode_cursor(value: &str) -> Result<ConnectionCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(validation_error("invalid downloader cursor"));
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(|| validation_error("invalid downloader cursor"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| validation_error("invalid downloader cursor"))?;
    if envelope.checksum != cursor_checksum(&payload) {
        return Err(validation_error("invalid downloader cursor"));
    }
    serde_json::from_slice::<ConnectionCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(|| validation_error("invalid downloader cursor"))
}

fn cursor_checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.downloader-connection.cursor.v1\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn validation_error(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}
