use crate::discovery::model::{RelativePath, RootId};
use crate::shared::error::{AppError, ErrorCode};

/// 在 discovery 持有的写事务上检查候选收件箱是否与目标重叠。
///
/// 这是 organization 表的只读窄投影，不允许 discovery 写入目标聚合。
///
/// # Errors
///
/// 数据库不可用或持久路径已损坏时返回内部错误。
pub async fn has_target_overlap_on_connection(
    connection: &mut sqlx::SqliteConnection,
    root_id: &RootId,
    path: &RelativePath,
) -> Result<bool, AppError> {
    let paths = sqlx::query_scalar::<_, String>(
        "SELECT relative_path_display FROM organization_targets WHERE root_id=?",
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
