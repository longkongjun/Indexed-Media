use sqlx::{Row as _, SqlitePool};
use uuid::Uuid;

use crate::discovery::model::{RelativePath, RootId};
use crate::shared::error::{AppError, ErrorCode};

/// organization 用于判断候选目标是否与收件箱重叠的只读窄投影。
#[derive(Clone)]
pub struct OrganizationInboxProjection {
    pool: SqlitePool,
}

/// planner 使用的当前来源 revision 与能力安全位置投影。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationSourceProjection {
    /// `ProcessingTask` 稳定 ID。
    pub task_id: Uuid,
    /// 当前不可变文件 revision ID。
    pub file_revision_id: Uuid,
    /// 来源收件箱 ID。
    pub inbox_directory_id: Uuid,
    /// 来源部署根逻辑 ID。
    pub root_id: RootId,
    /// 从部署根开始的规范相对文件路径。
    pub relative_path: RelativePath,
    /// revision 捕获的持久文件身份字节。
    pub identity_snapshot: Vec<u8>,
    /// revision 捕获的文件长度。
    pub size_bytes: u64,
    /// revision 捕获的纳秒修改时间。
    pub modified_at_ns: i64,
}

impl OrganizationInboxProjection {
    /// 使用已有数据库连接池创建投影。
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 返回同一逻辑根内是否存在相等、包含或被包含的收件箱路径。
    ///
    /// # Errors
    ///
    /// 数据库不可用或持久路径已损坏时返回内部错误。
    pub async fn has_overlap(
        &self,
        root_id: &RootId,
        path: &RelativePath,
    ) -> Result<bool, AppError> {
        let mut connection = self.pool.acquire().await.map_err(internal)?;
        has_inbox_overlap_on_connection(&mut connection, root_id, path).await
    }

    /// 读取账户任务当前绑定的 revision 与根内来源位置。
    ///
    /// # Errors
    ///
    /// 任务不存在、账户不匹配、路径损坏或数据库不可用时返回稳定应用错误。
    pub async fn source_for_task(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<OrganizationSourceProjection, AppError> {
        let row = sqlx::query(
            "SELECT t.id,t.file_revision_id,t.inbox_directory_id,i.root_id,
                    i.relative_path_display AS inbox_path,
                    f.relative_path_display AS file_path,
                    r.identity_snapshot,r.size_bytes,r.modified_at_ns
             FROM tasks_processing_tasks t
             JOIN discovery_inbox_directories i ON i.id=t.inbox_directory_id
             JOIN discovery_tracked_files f ON f.id=t.discovered_file_id
             JOIN discovery_file_revisions r ON r.id=t.file_revision_id
             WHERE t.account_id=? AND t.id=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::TaskNotFound, "processing task not found"))?;
        let stored_task_id: Vec<u8> = row.get("id");
        let revision_id: Vec<u8> = row.get("file_revision_id");
        let inbox_id: Vec<u8> = row.get("inbox_directory_id");
        let root_id: String = row.get("root_id");
        let inbox_path: String = row.get("inbox_path");
        let file_path: String = row.get("file_path");
        let joined = if inbox_path == "." {
            file_path
        } else {
            format!("{inbox_path}/{file_path}")
        };
        Ok(OrganizationSourceProjection {
            task_id: Uuid::from_slice(&stored_task_id)
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
            file_revision_id: Uuid::from_slice(&revision_id)
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
            inbox_directory_id: Uuid::from_slice(&inbox_id)
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
            root_id: RootId::parse(&root_id)
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
            relative_path: RelativePath::parse(&joined)
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
            identity_snapshot: row.get("identity_snapshot"),
            size_bytes: u64::try_from(row.get::<i64, _>("size_bytes"))
                .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?,
            modified_at_ns: row.get("modified_at_ns"),
        })
    }
}

/// 在调用方持有的连接/事务上权威检查收件箱 overlap。
///
/// # Errors
///
/// 数据库不可用或持久路径已损坏时返回内部错误。
pub async fn has_inbox_overlap_on_connection(
    connection: &mut sqlx::SqliteConnection,
    root_id: &RootId,
    path: &RelativePath,
) -> Result<bool, AppError> {
    let paths = sqlx::query_scalar::<_, String>(
        "SELECT relative_path_display FROM discovery_inbox_directories WHERE root_id=?",
    )
    .bind(root_id.as_str())
    .fetch_all(connection)
    .await
    .map_err(internal)?;
    for existing in paths {
        let existing = RelativePath::parse(&existing)
            .map_err(|error| AppError::new(ErrorCode::Internal, error.to_string()))?;
        if existing.overlaps(path) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn internal(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
