use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use uuid::Uuid;

use crate::discovery::model::{InboxHealth, RelativePath, RootId};
use crate::discovery::policy::ensure_default_policy;
use crate::organization::target_projection::has_target_overlap_on_connection;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, MAX_CURSOR_BYTES, PageRequest};

#[derive(Clone)]
/// 收件箱定义、身份快照与健康状态的 `SQLite` 持久化边界。
pub struct DiscoveryStore {
    pool: sqlx::SqlitePool,
}

/// 在一个即时事务中插入的已校验收件箱值。
pub struct NewInbox<'a> {
    /// 新的稳定收件箱 UUID。
    pub id: Uuid,
    /// 包含该目录的已配置部署根目录。
    pub root_id: &'a RootId,
    /// 根目录下的规范化目录路径。
    pub relative_path: &'a RelativePath,
    /// 能力预检捕获的序列化根目录身份。
    pub root_identity: &'a [u8],
    /// 能力预检捕获的序列化目录身份。
    pub directory_identity: &'a [u8],
    /// 用于创建、更新和检查时间戳的 UTC 微秒。
    pub now_us: i64,
}

#[derive(Clone)]
/// 用于能力重新验证的已解码持久化收件箱。
pub struct StoredInbox {
    /// 稳定的收件箱 UUID。
    pub id: Uuid,
    /// 已校验部署根目录标识符。
    pub root_id: RootId,
    /// 已校验的根目录下规范化路径。
    pub relative_path: RelativePath,
    /// 持久化的根目录身份快照。
    pub root_identity: Vec<u8>,
    /// 持久化的目录身份快照。
    pub directory_identity: Vec<u8>,
    /// 最近一次持久化的重新验证结果。
    pub health: InboxHealth,
    /// 最近一次重新验证的 UTC 微秒时间。
    pub last_checked_at_us: i64,
    /// 作为主分页键的 UTC 微秒。
    pub created_at_us: i64,
}

#[derive(Serialize, Deserialize)]
struct InboxCursor {
    created_at_us: i64,
    id: Uuid,
}

impl DiscoveryStore {
    #[must_use]
    /// 将收件箱持久化绑定到 `pool`，但不打开事务。
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// 返回 `path` 是否按完整路径组件与 `root_id` 上任意持久化收件箱重叠。
    ///
    /// # Errors
    ///
    /// 无法查询路径或某个持久化路径校验失败时返回 [`AppError`]。
    pub async fn has_overlap(
        &self,
        root_id: &RootId,
        path: &RelativePath,
    ) -> Result<bool, AppError> {
        Ok(self
            .paths_for_root(&self.pool, root_id)
            .await?
            .iter()
            .any(|existing| existing.overlaps(path)))
    }

    /// 重新检查重叠，并在 `SQLite` 即时事务中插入 `inbox`。
    ///
    /// 事务会串行化竞争创建，将收件箱和身份快照一同提交，并在任一提交前错误时尝试回滚。
    ///
    /// # Errors
    ///
    /// 组件或唯一性重叠时返回 [`ErrorCode::InboxOverlap`]。连接、事务、查询、插入或提交失败时
    /// 返回内部 [`AppError`]。
    pub async fn create_immediate(&self, inbox: NewInbox<'_>) -> Result<StoredInbox, AppError> {
        let mut connection = self.pool.acquire().await.map_err(internal)?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(internal)?;
        let result = async {
            if has_target_overlap_on_connection(&mut connection, inbox.root_id, inbox.relative_path)
                .await?
            {
                return Err(AppError::new(
                    ErrorCode::InboxOverlap,
                    "inbox directory overlaps an organization target",
                ));
            }
            let existing = self.paths_for_root(&mut *connection, inbox.root_id).await?;
            if existing
                .iter()
                .any(|path| path.overlaps(inbox.relative_path))
            {
                return Err(AppError::new(
                    ErrorCode::InboxOverlap,
                    "inbox directory overlaps an existing path",
                ));
            }
            sqlx::query(
                "INSERT INTO discovery_inbox_directories
                 (id,root_id,relative_path_bytes,relative_path_display,root_identity,
                  directory_identity,health,last_checked_at_us,version,created_at_us,updated_at_us)
                 VALUES (?,?,?,?,?,?,'available',?,1,?,?)",
            )
            .bind(inbox.id.as_bytes().as_slice())
            .bind(inbox.root_id.as_str())
            .bind(inbox.relative_path.bytes())
            .bind(inbox.relative_path.as_str())
            .bind(inbox.root_identity)
            .bind(inbox.directory_identity)
            .bind(inbox.now_us)
            .bind(inbox.now_us)
            .bind(inbox.now_us)
            .execute(&mut *connection)
            .await
            .map_err(|error| {
                if error
                    .as_database_error()
                    .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
                {
                    AppError::new(ErrorCode::InboxOverlap, "inbox directory already exists")
                } else {
                    internal(error)
                }
            })?;
            ensure_default_policy(&mut connection, inbox.id, inbox.now_us).await?;
            Ok(StoredInbox {
                id: inbox.id,
                root_id: inbox.root_id.clone(),
                relative_path: inbox.relative_path.clone(),
                root_identity: inbox.root_identity.to_vec(),
                directory_identity: inbox.directory_identity.to_vec(),
                health: InboxHealth::Available,
                last_checked_at_us: inbox.now_us,
                created_at_us: inbox.now_us,
            })
        }
        .await;
        match result {
            Ok(value) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(internal)?;
                Ok(value)
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error)
            }
        }
    }

    /// 按 UUID 加载并校验一个持久化收件箱。
    ///
    /// # Errors
    ///
    /// 不存在时返回 [`ErrorCode::InboxNotFound`]；查询失败或持久化 UUID、根目录、路径、健康值
    /// 格式错误时返回内部 [`AppError`]。
    pub async fn get(&self, id: Uuid) -> Result<StoredInbox, AppError> {
        let row = sqlx::query(
            "SELECT id,root_id,relative_path_display,root_identity,directory_identity,
                    health,last_checked_at_us,created_at_us
             FROM discovery_inbox_directories WHERE id=?",
        )
        .bind(id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::InboxNotFound, "inbox directory not found"))?;
        decode_row(&row)
    }

    /// 返回不透明游标之后按 `(created_at_us, id)` 排序的收件箱。
    ///
    /// 最多返回 `page.limit` 行；仅存在另一行时，`next_cursor` 才编码最后返回的排序键。
    ///
    /// # Errors
    ///
    /// 游标无效、查询/序列化失败或存储的收件箱数据格式错误时返回 [`AppError`]。
    pub async fn list(&self, page: &PageRequest) -> Result<CursorPage<StoredInbox>, AppError> {
        let cursor = page.cursor.as_deref().map(decode_cursor).transpose()?;
        let rows = if let Some(cursor) = cursor {
            sqlx::query(
                "SELECT id,root_id,relative_path_display,root_identity,directory_identity,
                        health,last_checked_at_us,created_at_us
                 FROM discovery_inbox_directories
                 WHERE created_at_us>? OR (created_at_us=? AND id>?)
                 ORDER BY created_at_us,id LIMIT ?",
            )
            .bind(cursor.created_at_us)
            .bind(cursor.created_at_us)
            .bind(cursor.id.as_bytes().as_slice())
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT id,root_id,relative_path_display,root_identity,directory_identity,
                        health,last_checked_at_us,created_at_us
                 FROM discovery_inbox_directories ORDER BY created_at_us,id LIMIT ?",
            )
            .bind(i64::from(page.limit) + 1)
            .fetch_all(&self.pool)
            .await
        }
        .map_err(internal)?;
        let has_more = rows.len() > page.limit as usize;
        let mut items = rows
            .iter()
            .take(page.limit as usize)
            .map(decode_row)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more && let Some(last) = items.last() {
            let bytes = serde_json::to_vec(&InboxCursor {
                created_at_us: last.created_at_us,
                id: last.id,
            })
            .map_err(internal)?;
            Some(URL_SAFE_NO_PAD.encode(bytes))
        } else {
            None
        };
        Ok(CursorPage {
            items: std::mem::take(&mut items),
            next_cursor,
        })
    }

    /// 持久化收件箱健康结果及其检查/更新时间戳。
    ///
    /// 在此存储边界，缺失的收件箱是成功的空操作。
    ///
    /// # Errors
    ///
    /// `SQLite` 无法执行更新时返回 [`AppError`]。
    pub async fn update_health(
        &self,
        id: Uuid,
        health: InboxHealth,
        checked_at_us: i64,
    ) -> Result<(), AppError> {
        let value = match health {
            InboxHealth::Available => "available",
            InboxHealth::Unavailable => "unavailable",
        };
        sqlx::query(
            "UPDATE discovery_inbox_directories
             SET health=?,last_checked_at_us=?,updated_at_us=? WHERE id=?",
        )
        .bind(value)
        .bind(checked_at_us)
        .bind(checked_at_us)
        .bind(id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map_err(internal)?;
        Ok(())
    }

    async fn paths_for_root<'e, E>(
        &self,
        executor: E,
        root_id: &RootId,
    ) -> Result<Vec<RelativePath>, AppError>
    where
        E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
    {
        sqlx::query_scalar::<_, String>(
            "SELECT relative_path_display FROM discovery_inbox_directories WHERE root_id=?",
        )
        .bind(root_id.as_str())
        .fetch_all(executor)
        .await
        .map_err(internal)?
        .into_iter()
        .map(|value| {
            RelativePath::parse(&value)
                .map_err(|_| AppError::new(ErrorCode::Internal, "stored relative path invalid"))
        })
        .collect()
    }
}

fn decode_row(row: &sqlx::sqlite::SqliteRow) -> Result<StoredInbox, AppError> {
    let id_bytes: Vec<u8> = row.get("id");
    let id = Uuid::from_slice(&id_bytes)
        .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
    let root_id = RootId::parse(row.get::<String, _>("root_id").as_str())
        .map_err(|_| AppError::new(ErrorCode::Internal, "stored root ID invalid"))?;
    let relative_path = RelativePath::parse(row.get::<String, _>("relative_path_display").as_str())
        .map_err(|_| AppError::new(ErrorCode::Internal, "stored relative path invalid"))?;
    let health = match row.get::<String, _>("health").as_str() {
        "available" => InboxHealth::Available,
        "unavailable" => InboxHealth::Unavailable,
        _ => return Err(AppError::new(ErrorCode::Internal, "stored health invalid")),
    };
    Ok(StoredInbox {
        id,
        root_id,
        relative_path,
        root_identity: row.get("root_identity"),
        directory_identity: row.get("directory_identity"),
        health,
        last_checked_at_us: row.get("last_checked_at_us"),
        created_at_us: row.get("created_at_us"),
    })
}

fn decode_cursor(value: &str) -> Result<InboxCursor, AppError> {
    if value.len() > MAX_CURSOR_BYTES {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid inbox cursor",
        ));
    }
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| AppError::new(ErrorCode::ValidationFailed, "invalid inbox cursor"))
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
