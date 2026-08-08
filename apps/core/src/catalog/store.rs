#![allow(clippy::too_many_lines)]

use std::collections::{HashMap, HashSet};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{SecondsFormat, TimeZone as _, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};
use unicode_normalization::UnicodeNormalization as _;
use uuid::Uuid;

use crate::catalog::CatalogLocalResultPort;
use crate::catalog::model::{
    ArtworkKind, ArtworkRefView, ArtworkState, LocalStatus, MediaChild, MediaFileAsset,
    MediaItemDetail, MediaItemFilter, MediaItemKind, MediaItemSummary, MediaMetadataValue,
    MediaNodeKind, MediaVersion, MetadataFieldState, MetadataSourceType, NfoStatus,
    VerifiedLocalResult,
};
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};
use crate::tasks::events::{CATALOG_MEDIA_CHANGED, CatalogMediaChange, CatalogMediaChangedPayload};

const MAX_NODES: usize = 200;
const MAX_METADATA: usize = 64;
const MAX_VERSIONS: usize = 256;
const MAX_FILE_ASSETS: usize = 256;
const MAX_FILE_LINKS: usize = 1024;

#[derive(Deserialize, Serialize)]
struct CatalogCursor {
    version: u8,
    account_id: Uuid,
    filter_digest: String,
    updated_at_us: i64,
    id: Uuid,
    snapshot_revision: i64,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

#[derive(Clone)]
/// 将已核对本地结果原子投影为账户隔离目录读模型的存储入口。
pub struct CatalogStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl CatalogStore {
    #[must_use]
    /// 使用独立通知器创建存储；事务事件仍会持久化，但不会唤醒其他共享监听者。
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    #[must_use]
    /// 使用共享通知器创建存储，使提交后的目录事件可立即唤醒 SSE 交付循环。
    pub fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// 在不重新读取识别候选的前提下，原子应用一个完整的已核对本地投影。
    ///
    /// # Errors
    ///
    /// 媒体树不合法时返回校验错误，任务或 revision 不属于账户时返回未找到；同一
    /// `result_id` 被不同内容复用时返回冲突，数据库事务失败时返回内部错误。相同账户、
    /// 标识和内容的重放是幂等的，不会重复生成目录变更事件。
    pub async fn apply_verified_local_result(
        &self,
        account_id: Uuid,
        result: &VerifiedLocalResult,
        now_us: i64,
    ) -> Result<Uuid, AppError> {
        validate_result(result)?;
        timestamp(now_us)?;
        let digest = result_digest(result)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;

        if let Some(row) = sqlx::query(
            "SELECT account_id,media_item_id,request_sha256
             FROM catalog_applied_local_results WHERE result_id=?",
        )
        .bind(result.result_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        {
            let same = uuid(&row, "account_id")? == account_id
                && row.get::<Vec<u8>, _>("request_sha256").as_slice() == digest.as_slice();
            let media_id = uuid(&row, "media_item_id")?;
            tx.rollback().await.map_err(internal)?;
            return if same {
                Ok(media_id)
            } else {
                Err(AppError::new(
                    ErrorCode::RequestConflict,
                    "catalog result ID is already bound to different content",
                ))
            };
        }

        let task = sqlx::query(
            "SELECT account_id,inbox_directory_id FROM tasks_processing_tasks WHERE id=?",
        )
        .bind(result.task_id.as_bytes().as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "processing task not found"))?;
        if uuid(&task, "account_id")? != account_id {
            return Err(AppError::new(
                ErrorCode::NotFound,
                "processing task is outside account",
            ));
        }
        let inbox_id = uuid(&task, "inbox_directory_id")?;
        for asset in &result.file_assets {
            let owned = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM discovery_file_revisions revision
                 JOIN discovery_tracked_files file ON file.id=revision.tracked_file_id
                 WHERE revision.id=? AND file.inbox_directory_id=?",
            )
            .bind(asset.file_revision_id.as_bytes().as_slice())
            .bind(inbox_id.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(internal)?;
            if owned != 1 {
                return Err(AppError::new(
                    ErrorCode::NotFound,
                    "file revision is outside task inbox",
                ));
            }
        }

        let existing =
            sqlx::query("SELECT account_id,projection_version FROM catalog_media_items WHERE id=?")
                .bind(result.media.id.as_bytes().as_slice())
                .fetch_optional(&mut *tx)
                .await
                .map_err(internal)?;
        let (created, projection_version) = if let Some(row) = existing {
            if uuid(&row, "account_id")? != account_id {
                return Err(AppError::new(
                    ErrorCode::RequestConflict,
                    "catalog media ID belongs to another account",
                ));
            }
            let previous: i64 = row.get("projection_version");
            clear_projection(&mut tx, result.media.id).await?;
            (false, previous.saturating_add(1))
        } else {
            sqlx::query(
                "INSERT INTO catalog_media_items
                 (id,account_id,library_id,kind,title,title_normalized,year,local_status,nfo_status,
                  projection_version,created_at_us,updated_at_us)
                 VALUES (?,?,?,?,?,?,?,?,?,1,?,?)",
            )
            .bind(result.media.id.as_bytes().as_slice())
            .bind(account_id.as_bytes().as_slice())
            .bind(result.library_id.as_bytes().as_slice())
            .bind(result.media.kind.as_str())
            .bind(&result.media.title)
            .bind(normalize_title(&result.media.title))
            .bind(result.media.year.map(i64::from))
            .bind(result.media.local_status.as_str())
            .bind(result.nfo_status.as_str())
            .bind(now_us)
            .bind(now_us)
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            (true, 1)
        };

        insert_projection(&mut tx, account_id, result).await?;
        sqlx::query(
            "UPDATE catalog_media_items
             SET library_id=?,kind=?,title=?,title_normalized=?,year=?,local_status=?,nfo_status=?,
                 projection_version=?,updated_at_us=? WHERE id=? AND account_id=?",
        )
        .bind(result.library_id.as_bytes().as_slice())
        .bind(result.media.kind.as_str())
        .bind(&result.media.title)
        .bind(normalize_title(&result.media.title))
        .bind(result.media.year.map(i64::from))
        .bind(result.media.local_status.as_str())
        .bind(result.nfo_status.as_str())
        .bind(projection_version)
        .bind(now_us)
        .bind(result.media.id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        sqlx::query(
            "INSERT INTO catalog_applied_local_results
             (result_id,account_id,task_id,media_item_id,request_sha256,applied_at_us)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(result.result_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(result.task_id.as_bytes().as_slice())
        .bind(result.media.id.as_bytes().as_slice())
        .bind(digest.as_slice())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        OutboxWriter::write(
            &mut tx,
            CATALOG_MEDIA_CHANGED,
            result.media.id,
            &CatalogMediaChangedPayload {
                media_item_id: result.media.id,
                projection_version,
                change: if created {
                    CatalogMediaChange::Created
                } else {
                    CatalogMediaChange::Updated
                },
            },
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(result.media.id)
    }

    /// 返回一页快照稳定、有界的正式媒体摘要。
    ///
    /// # Errors
    ///
    /// 过滤条件或游标无效时返回校验错误；持久化失败时返回内部错误。
    pub async fn list(
        &self,
        account_id: Uuid,
        filter: &MediaItemFilter,
        page: &PageRequest,
    ) -> Result<CursorPage<MediaItemSummary>, AppError> {
        if page.limit == 0 || page.limit > crate::shared::page::MAX_PAGE_LIMIT {
            return Err(validation("catalog page limit is outside bounds"));
        }
        let filter = normalize_filter(filter)?;
        let digest = filter_digest(&filter);
        let cursor = page.cursor.as_deref().map(decode_cursor).transpose()?;
        if cursor
            .as_ref()
            .is_some_and(|cursor| cursor.account_id != account_id || cursor.filter_digest != digest)
        {
            return Err(validation("catalog cursor query does not match"));
        }
        let snapshot_revision = if let Some(cursor) = &cursor {
            Some(cursor.snapshot_revision)
        } else {
            sqlx::query_scalar::<_, Option<i64>>(
                "SELECT MAX(revision) FROM catalog_media_order_history WHERE account_id=?",
            )
            .bind(account_id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(internal)?
        };
        let Some(snapshot_revision) = snapshot_revision else {
            return Ok(CursorPage {
                items: Vec::new(),
                next_cursor: None,
            });
        };

        let mut query = QueryBuilder::<Sqlite>::new(
            "WITH snapshot_media AS (
                 SELECT history.* FROM catalog_media_order_history history
                 WHERE history.account_id=",
        );
        query
            .push_bind(account_id.as_bytes().to_vec())
            .push(" AND history.revision<=")
            .push_bind(snapshot_revision)
            .push(
                " AND history.revision=(
                     SELECT MAX(candidate.revision) FROM catalog_media_order_history candidate
                     WHERE candidate.media_item_id=history.media_item_id
                       AND candidate.revision<=",
            )
            .push_bind(snapshot_revision)
            .push(
                ")
             ) SELECT media_item_id AS id,library_id,kind,title,year,local_status,
                      artwork_id,artwork_kind,artwork_state,updated_at_us,
                      updated_at_us AS snapshot_updated_at_us
               FROM snapshot_media WHERE 1=1",
            );
        push_filters(&mut query, &filter);
        if let Some(cursor) = &cursor {
            query
                .push(" AND (updated_at_us<")
                .push_bind(cursor.updated_at_us)
                .push(" OR (updated_at_us=")
                .push_bind(cursor.updated_at_us)
                .push(" AND media_item_id<")
                .push_bind(cursor.id.as_bytes().to_vec())
                .push("))");
        }
        query
            .push(" ORDER BY updated_at_us DESC,media_item_id DESC LIMIT ")
            .push_bind(i64::from(page.limit) + 1);
        let rows = query
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(internal)?;
        let has_more = rows.len() > page.limit as usize;
        let selected = rows.iter().take(page.limit as usize).collect::<Vec<_>>();
        let items = selected
            .iter()
            .map(|row| decode_summary(row))
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if has_more {
            selected
                .last()
                .map(|row| {
                    encode_cursor(&CatalogCursor {
                        version: 1,
                        account_id,
                        filter_digest: digest.clone(),
                        updated_at_us: row.get("snapshot_updated_at_us"),
                        id: uuid(row, "id")?,
                        snapshot_revision,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// 读取一个有界的正式媒体层级。
    ///
    /// # Errors
    ///
    /// 跨账户读取时返回未找到；已存图超过边界、持久化或解码失败时返回内部错误。
    pub async fn detail(
        &self,
        account_id: Uuid,
        media_item_id: Uuid,
    ) -> Result<MediaItemDetail, AppError> {
        let row = sqlx::query(
            "SELECT item.id,item.library_id,item.kind,item.title,item.year,item.local_status,
                    item.updated_at_us,art.id AS artwork_id,art.kind AS artwork_kind,
                    art.state AS artwork_state,item.nfo_status
             FROM catalog_media_items item
             LEFT JOIN catalog_artwork_refs art
               ON art.media_item_id=item.id AND art.kind='poster'
             WHERE item.id=? AND item.account_id=?",
        )
        .bind(media_item_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound, "catalog item not found"))?;
        let item = decode_summary(&row)?;
        let nfo_status = parse_nfo_status(row.get::<String, _>("nfo_status").as_str())?;

        let metadata_rows = sqlx::query(
            "SELECT field,value,state,source_type,source_id,source_version
             FROM catalog_metadata_values WHERE media_item_id=? ORDER BY field LIMIT 65",
        )
        .bind(media_item_id.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        ensure_cap(metadata_rows.len(), MAX_METADATA, "catalog metadata")?;
        let metadata = metadata_rows
            .iter()
            .map(decode_metadata)
            .collect::<Result<Vec<_>, _>>()?;

        let node_rows = sqlx::query(
            "SELECT id,parent_id,kind,title,ordinal FROM catalog_media_nodes
             WHERE media_item_id=?
             ORDER BY CASE kind WHEN 'season' THEN 0 WHEN 'episode' THEN 1 ELSE 2 END,
                      ordinal,id LIMIT 201",
        )
        .bind(media_item_id.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        ensure_cap(node_rows.len(), MAX_NODES, "catalog children")?;

        let version_rows = sqlx::query(
            "SELECT id,owner_node_id,label FROM catalog_media_versions
             WHERE media_item_id=? ORDER BY owner_node_id,id LIMIT 257",
        )
        .bind(media_item_id.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        ensure_cap(version_rows.len(), MAX_VERSIONS, "catalog versions")?;

        let file_rows = sqlx::query(
            "SELECT link.version_id,asset.id,asset.source_relative_path,
                    asset.current_relative_path,asset.size_bytes
             FROM catalog_version_files link
             JOIN catalog_file_assets asset ON asset.id=link.file_asset_id
             WHERE link.media_item_id=? ORDER BY link.version_id,link.ordinal LIMIT 1025",
        )
        .bind(media_item_id.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        ensure_cap(file_rows.len(), MAX_FILE_LINKS, "catalog file links")?;
        let mut files_by_version: HashMap<Uuid, Vec<MediaFileAsset>> = HashMap::new();
        for row in &file_rows {
            let size_bytes = u64::try_from(row.get::<i64, _>("size_bytes")).map_err(internal)?;
            files_by_version
                .entry(uuid(row, "version_id")?)
                .or_default()
                .push(MediaFileAsset {
                    id: uuid(row, "id")?,
                    source_relative_path: row.get("source_relative_path"),
                    current_relative_path: row.get("current_relative_path"),
                    size_bytes,
                });
        }
        let mut root_versions = Vec::new();
        let mut versions_by_node: HashMap<Uuid, Vec<MediaVersion>> = HashMap::new();
        for row in &version_rows {
            let version_id = uuid(row, "id")?;
            let version = MediaVersion {
                id: version_id,
                label: row.get("label"),
                files: files_by_version.remove(&version_id).unwrap_or_default(),
            };
            if let Some(owner) = optional_uuid(row, "owner_node_id")? {
                versions_by_node.entry(owner).or_default().push(version);
            } else {
                root_versions.push(version);
            }
        }
        let children = node_rows
            .iter()
            .map(|row| {
                let id = uuid(row, "id")?;
                Ok(MediaChild {
                    id,
                    parent_id: optional_uuid(row, "parent_id")?,
                    kind: parse_node_kind(row.get::<String, _>("kind").as_str())?,
                    title: row.get("title"),
                    ordinal: u32::try_from(row.get::<i64, _>("ordinal")).map_err(internal)?,
                    versions: versions_by_node.remove(&id).unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, AppError>>()?;

        let task_rows = sqlx::query(
            "SELECT DISTINCT task_id FROM catalog_applied_local_results
             WHERE account_id=? AND media_item_id=? ORDER BY applied_at_us,task_id LIMIT 33",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(media_item_id.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        ensure_cap(task_rows.len(), 32, "catalog related tasks")?;
        let related_task_ids = task_rows
            .iter()
            .map(|row| uuid(row, "task_id"))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(MediaItemDetail {
            item,
            metadata,
            versions: root_versions,
            children,
            nfo_status,
            related_task_ids,
        })
    }
}

#[async_trait::async_trait]
impl CatalogLocalResultPort for CatalogStore {
    async fn apply_local_result(
        &self,
        account_id: Uuid,
        result: &VerifiedLocalResult,
        now_us: i64,
    ) -> Result<Uuid, AppError> {
        self.apply_verified_local_result(account_id, result, now_us)
            .await
    }
}

async fn clear_projection(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    media_item_id: Uuid,
) -> Result<(), AppError> {
    for table in [
        "catalog_version_files",
        "catalog_media_versions",
        "catalog_media_nodes",
        "catalog_file_assets",
        "catalog_metadata_values",
        "catalog_artwork_refs",
    ] {
        sqlx::query(&format!("DELETE FROM {table} WHERE media_item_id=?"))
            .bind(media_item_id.as_bytes().as_slice())
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
    }
    Ok(())
}

async fn insert_projection(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    account_id: Uuid,
    result: &VerifiedLocalResult,
) -> Result<(), AppError> {
    for node in &result.media.nodes {
        sqlx::query(
            "INSERT INTO catalog_media_nodes(id,media_item_id,parent_id,kind,title,ordinal)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(node.id.as_bytes().as_slice())
        .bind(result.media.id.as_bytes().as_slice())
        .bind(node.parent_id.map(|id| id.as_bytes().to_vec()))
        .bind(node.kind.as_str())
        .bind(&node.title)
        .bind(i64::from(node.ordinal))
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    for asset in &result.file_assets {
        sqlx::query(
            "INSERT INTO catalog_file_assets
             (id,account_id,media_item_id,file_revision_id,source_relative_path,
              current_relative_path,size_bytes) VALUES (?,?,?,?,?,?,?)",
        )
        .bind(asset.id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(result.media.id.as_bytes().as_slice())
        .bind(asset.file_revision_id.as_bytes().as_slice())
        .bind(&asset.source_relative_path)
        .bind(&asset.current_relative_path)
        .bind(i64::try_from(asset.size_bytes).map_err(internal)?)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    for version in &result.media.versions {
        sqlx::query(
            "INSERT INTO catalog_media_versions(id,media_item_id,owner_node_id,label)
             VALUES (?,?,?,?)",
        )
        .bind(version.id.as_bytes().as_slice())
        .bind(result.media.id.as_bytes().as_slice())
        .bind(version.owner_node_id.map(|id| id.as_bytes().to_vec()))
        .bind(&version.label)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
        for (ordinal, file_id) in version.file_asset_ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO catalog_version_files
                 (media_item_id,version_id,file_asset_id,ordinal) VALUES (?,?,?,?)",
            )
            .bind(result.media.id.as_bytes().as_slice())
            .bind(version.id.as_bytes().as_slice())
            .bind(file_id.as_bytes().as_slice())
            .bind(i64::try_from(ordinal).map_err(internal)?)
            .execute(&mut **tx)
            .await
            .map_err(internal)?;
        }
    }
    for value in &result.media.metadata {
        sqlx::query(
            "INSERT INTO catalog_metadata_values
             (media_item_id,field,value,state,source_type,source_id,source_version)
             VALUES (?,?,?,?,?,?,?)",
        )
        .bind(result.media.id.as_bytes().as_slice())
        .bind(&value.field)
        .bind(&value.value)
        .bind(value.state.as_str())
        .bind(value.source.as_ref().map(|source| source.kind.as_str()))
        .bind(
            value
                .source
                .as_ref()
                .and_then(|source| source.id.as_deref()),
        )
        .bind(
            value
                .source
                .as_ref()
                .and_then(|source| source.version.as_deref()),
        )
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    for artwork in &result.media.artwork_refs {
        sqlx::query(
            "INSERT INTO catalog_artwork_refs
             (id,media_item_id,kind,state,local_relative_path) VALUES (?,?,?,?,?)",
        )
        .bind(artwork.id.as_bytes().as_slice())
        .bind(result.media.id.as_bytes().as_slice())
        .bind(artwork.kind.as_str())
        .bind(artwork.state.as_str())
        .bind(&artwork.local_relative_path)
        .execute(&mut **tx)
        .await
        .map_err(internal)?;
    }
    Ok(())
}

fn validate_result(result: &VerifiedLocalResult) -> Result<(), AppError> {
    validate_text(&result.media.title, 512, "catalog title")?;
    if result
        .media
        .year
        .is_some_and(|year| !(1870..=9999).contains(&year))
    {
        return Err(validation("catalog year is outside bounds"));
    }
    if result.media.nodes.len() > MAX_NODES
        || result.media.metadata.len() > MAX_METADATA
        || result.media.versions.len() > MAX_VERSIONS
        || result.file_assets.is_empty()
        || result.file_assets.len() > MAX_FILE_ASSETS
    {
        return Err(validation("catalog graph is outside bounds"));
    }
    let mut node_ids = HashSet::new();
    for node in &result.media.nodes {
        validate_text(&node.title, 512, "catalog child title")?;
        if node.ordinal > 99_999 || !node_ids.insert(node.id) {
            return Err(validation("catalog child is invalid"));
        }
        match result.media.kind {
            MediaItemKind::Movie => return Err(validation("movie cannot contain child nodes")),
            MediaItemKind::Series
                if !matches!(node.kind, MediaNodeKind::Season | MediaNodeKind::Episode) =>
            {
                return Err(validation("series child kind is invalid"));
            }
            MediaItemKind::GenericVideo if node.kind != MediaNodeKind::GenericVideoItem => {
                return Err(validation("generic-video child kind is invalid"));
            }
            MediaItemKind::Series | MediaItemKind::GenericVideo => {}
        }
    }
    for node in &result.media.nodes {
        match node.kind {
            MediaNodeKind::Episode => {
                let Some(parent) = node.parent_id else {
                    return Err(validation("episode parent is missing"));
                };
                if !result.media.nodes.iter().any(|candidate| {
                    candidate.id == parent && candidate.kind == MediaNodeKind::Season
                }) {
                    return Err(validation("episode parent is invalid"));
                }
            }
            MediaNodeKind::Season | MediaNodeKind::GenericVideoItem if node.parent_id.is_some() => {
                return Err(validation("catalog child parent is invalid"));
            }
            MediaNodeKind::Season | MediaNodeKind::GenericVideoItem => {}
        }
    }

    let mut file_ids = HashSet::new();
    let mut revision_ids = HashSet::new();
    for asset in &result.file_assets {
        if !file_ids.insert(asset.id) || !revision_ids.insert(asset.file_revision_id) {
            return Err(validation("catalog file identity is duplicated"));
        }
        validate_relative_path(&asset.source_relative_path)?;
        validate_relative_path(&asset.current_relative_path)?;
        if asset.size_bytes > 9_007_199_254_740_991 {
            return Err(validation("catalog file size is outside bounds"));
        }
    }
    let mut version_ids = HashSet::new();
    let mut files_referenced = HashSet::new();
    let mut owner_counts: HashMap<Option<Uuid>, usize> = HashMap::new();
    let mut link_count = 0_usize;
    for version in &result.media.versions {
        if !version_ids.insert(version.id)
            || version.file_asset_ids.is_empty()
            || version.file_asset_ids.len() > 32
        {
            return Err(validation("catalog version is invalid"));
        }
        if version
            .label
            .as_ref()
            .is_some_and(|label| label.chars().count() > 128 || label.chars().any(char::is_control))
        {
            return Err(validation("catalog version label is invalid"));
        }
        if let Some(owner) = version.owner_node_id {
            if !node_ids.contains(&owner) {
                return Err(validation("catalog version owner is invalid"));
            }
        } else if result.media.kind != MediaItemKind::Movie {
            return Err(validation("non-movie root version is invalid"));
        }
        *owner_counts.entry(version.owner_node_id).or_default() += 1;
        if owner_counts[&version.owner_node_id] > 32 {
            return Err(validation("catalog owner version count exceeds bounds"));
        }
        let mut version_files = HashSet::new();
        for file_id in &version.file_asset_ids {
            if !file_ids.contains(file_id) || !version_files.insert(*file_id) {
                return Err(validation("catalog version file is invalid"));
            }
            files_referenced.insert(*file_id);
            link_count += 1;
        }
    }
    if files_referenced != file_ids || link_count > MAX_FILE_LINKS {
        return Err(validation("catalog file reference set is invalid"));
    }
    if result.media.kind == MediaItemKind::Movie && result.media.versions.is_empty() {
        return Err(validation("movie version is missing"));
    }

    let mut fields = HashSet::new();
    for value in &result.media.metadata {
        validate_text(&value.field, 64, "catalog metadata field")?;
        if !fields.insert(value.field.as_str()) {
            return Err(validation("catalog metadata field is duplicated"));
        }
        match value.state {
            MetadataFieldState::Missing if value.value.is_none() && value.source.is_none() => {}
            MetadataFieldState::Present
                if value.value.as_ref().is_some_and(|value| {
                    value.chars().count() <= 4096 && !value.chars().any(char::is_control)
                }) && value.source.is_some() => {}
            MetadataFieldState::Missing | MetadataFieldState::Present => {
                return Err(validation("catalog metadata state is inconsistent"));
            }
        }
        if let Some(source) = &value.source {
            validate_optional_text(source.id.as_deref(), 128, "catalog source ID")?;
            validate_optional_text(source.version.as_deref(), 64, "catalog source version")?;
        }
    }
    let mut artwork_kinds = HashSet::new();
    let mut artwork_ids = HashSet::new();
    if result.media.artwork_refs.len() > 2 {
        return Err(validation("catalog artwork count exceeds bounds"));
    }
    for artwork in &result.media.artwork_refs {
        if !artwork_ids.insert(artwork.id) || !artwork_kinds.insert(artwork.kind) {
            return Err(validation("catalog artwork identity is duplicated"));
        }
        match artwork.state {
            ArtworkState::Available => validate_relative_path(
                artwork
                    .local_relative_path
                    .as_deref()
                    .ok_or_else(|| validation("available artwork path is missing"))?,
            )?,
            ArtworkState::Missing if artwork.local_relative_path.is_none() => {}
            ArtworkState::Missing => return Err(validation("missing artwork has a path")),
        }
    }
    Ok(())
}

fn validate_text(value: &str, max: usize, kind: &str) -> Result<(), AppError> {
    if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        Err(validation(format!("{kind} is invalid")))
    } else {
        Ok(())
    }
}

fn validate_optional_text(value: Option<&str>, max: usize, kind: &str) -> Result<(), AppError> {
    if let Some(value) = value {
        validate_text(value, max, kind)?;
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), AppError> {
    validate_text(value, 4096, "catalog relative path")?;
    if value.starts_with('/')
        || value.starts_with('\\')
        || value.contains("://")
        || value
            .split(['/', '\\'])
            .any(|part| part == "." || part == ".." || part.is_empty())
    {
        Err(validation("catalog relative path is unsafe"))
    } else {
        Ok(())
    }
}

fn result_digest(result: &VerifiedLocalResult) -> Result<[u8; 32], AppError> {
    let bytes = serde_json::to_vec(result).map_err(internal)?;
    Ok(Sha256::digest(bytes).into())
}

fn normalize_title(value: &str) -> String {
    value.nfkc().collect::<String>().to_lowercase()
}

fn normalize_filter(filter: &MediaItemFilter) -> Result<MediaItemFilter, AppError> {
    let mut filter = filter.clone();
    filter.query = filter
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_title);
    if filter
        .query
        .as_ref()
        .is_some_and(|query| query.chars().count() > 200 || query.chars().any(char::is_control))
    {
        return Err(validation("catalog query is outside bounds"));
    }
    Ok(filter)
}

fn filter_digest(filter: &MediaItemFilter) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.catalog.filter.v1\0");
    if let Some(kind) = filter.kind {
        hasher.update(kind.as_str().as_bytes());
    }
    hasher.update([0]);
    if let Some(library_id) = filter.library_id {
        hasher.update(library_id.as_bytes());
    }
    hasher.update([0]);
    if let Some(status) = filter.local_status {
        hasher.update(status.as_str().as_bytes());
    }
    hasher.update([0]);
    if let Some(query) = &filter.query {
        hasher.update(query.as_bytes());
    }
    hex::encode(&hasher.finalize()[..16])
}

fn push_filters(query: &mut QueryBuilder<'_, Sqlite>, filter: &MediaItemFilter) {
    if let Some(kind) = filter.kind {
        query.push(" AND kind=").push_bind(kind.as_str());
    }
    if let Some(library_id) = filter.library_id {
        query
            .push(" AND library_id=")
            .push_bind(library_id.as_bytes().to_vec());
    }
    if let Some(status) = filter.local_status {
        query.push(" AND local_status=").push_bind(status.as_str());
    }
    if let Some(search) = &filter.query {
        let escaped = search
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        query
            .push(" AND title_normalized LIKE ")
            .push_bind(format!("%{escaped}%"))
            .push(" ESCAPE '\\'");
    }
}

fn encode_cursor(cursor: &CatalogCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(internal)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(internal)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(AppError::new(
            ErrorCode::Internal,
            "catalog cursor exceeds bounds",
        ));
    }
    Ok(encoded)
}

fn decode_cursor(value: &str) -> Result<CatalogCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_cursor());
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(invalid_cursor)?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| invalid_cursor())?;
    if envelope.checksum != cursor_checksum(&payload) {
        return Err(invalid_cursor());
    }
    serde_json::from_slice::<CatalogCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(invalid_cursor)
}

fn cursor_checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.catalog.cursor.v1\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn invalid_cursor() -> AppError {
    validation("invalid catalog cursor")
}

fn decode_summary(row: &sqlx::sqlite::SqliteRow) -> Result<MediaItemSummary, AppError> {
    let year = row
        .get::<Option<i64>, _>("year")
        .map(|year| u16::try_from(year).map_err(internal))
        .transpose()?;
    let artwork_ref = optional_uuid(row, "artwork_id")?
        .map(|id| {
            Ok(ArtworkRefView {
                id,
                kind: parse_artwork_kind(
                    row.get::<Option<String>, _>("artwork_kind")
                        .as_deref()
                        .ok_or_else(|| invalid("artwork kind"))?,
                )?,
                state: parse_artwork_state(
                    row.get::<Option<String>, _>("artwork_state")
                        .as_deref()
                        .ok_or_else(|| invalid("artwork state"))?,
                )?,
            })
        })
        .transpose()?;
    Ok(MediaItemSummary {
        id: uuid(row, "id")?,
        kind: parse_item_kind(row.get::<String, _>("kind").as_str())?,
        library_id: uuid(row, "library_id")?,
        title: row.get("title"),
        year,
        local_status: parse_local_status(row.get::<String, _>("local_status").as_str())?,
        artwork_ref,
        updated_at: timestamp(row.get("updated_at_us"))?,
    })
}

fn decode_metadata(row: &sqlx::sqlite::SqliteRow) -> Result<MediaMetadataValue, AppError> {
    Ok(MediaMetadataValue {
        field: row.get("field"),
        value: row.get("value"),
        state: parse_metadata_state(row.get::<String, _>("state").as_str())?,
        source_type: row
            .get::<Option<String>, _>("source_type")
            .as_deref()
            .map(parse_source_type)
            .transpose()?,
        source_id: row.get("source_id"),
        source_version: row.get("source_version"),
    })
}

fn parse_item_kind(value: &str) -> Result<MediaItemKind, AppError> {
    match value {
        "movie" => Ok(MediaItemKind::Movie),
        "series" => Ok(MediaItemKind::Series),
        "generic-video" => Ok(MediaItemKind::GenericVideo),
        _ => Err(invalid("media item kind")),
    }
}

fn parse_node_kind(value: &str) -> Result<MediaNodeKind, AppError> {
    match value {
        "season" => Ok(MediaNodeKind::Season),
        "episode" => Ok(MediaNodeKind::Episode),
        "generic-video-item" => Ok(MediaNodeKind::GenericVideoItem),
        _ => Err(invalid("media node kind")),
    }
}

fn parse_local_status(value: &str) -> Result<LocalStatus, AppError> {
    match value {
        "complete" => Ok(LocalStatus::Complete),
        "partial" => Ok(LocalStatus::Partial),
        _ => Err(invalid("local status")),
    }
}

fn parse_nfo_status(value: &str) -> Result<NfoStatus, AppError> {
    match value {
        "not-requested" => Ok(NfoStatus::NotRequested),
        "complete" => Ok(NfoStatus::Complete),
        "partial" => Ok(NfoStatus::Partial),
        "failed" => Ok(NfoStatus::Failed),
        _ => Err(invalid("NFO status")),
    }
}

fn parse_artwork_kind(value: &str) -> Result<ArtworkKind, AppError> {
    match value {
        "poster" => Ok(ArtworkKind::Poster),
        "backdrop" => Ok(ArtworkKind::Backdrop),
        _ => Err(invalid("artwork kind")),
    }
}

fn parse_artwork_state(value: &str) -> Result<ArtworkState, AppError> {
    match value {
        "available" => Ok(ArtworkState::Available),
        "missing" => Ok(ArtworkState::Missing),
        _ => Err(invalid("artwork state")),
    }
}

fn parse_metadata_state(value: &str) -> Result<MetadataFieldState, AppError> {
    match value {
        "present" => Ok(MetadataFieldState::Present),
        "missing" => Ok(MetadataFieldState::Missing),
        _ => Err(invalid("metadata state")),
    }
}

fn parse_source_type(value: &str) -> Result<MetadataSourceType, AppError> {
    match value {
        "tmdb" => Ok(MetadataSourceType::Tmdb),
        "nfo" => Ok(MetadataSourceType::Nfo),
        "manual" => Ok(MetadataSourceType::Manual),
        "system" => Ok(MetadataSourceType::System),
        _ => Err(invalid("metadata source")),
    }
}

fn ensure_cap(actual: usize, maximum: usize, kind: &str) -> Result<(), AppError> {
    if actual > maximum {
        Err(AppError::new(
            ErrorCode::Internal,
            format!("{kind} exceeds response bounds"),
        ))
    } else {
        Ok(())
    }
}

fn timestamp(value: i64) -> Result<String, AppError> {
    Utc.timestamp_micros(value)
        .single()
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Micros, true))
        .ok_or_else(|| invalid("catalog timestamp"))
}

fn uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid, AppError> {
    Uuid::from_slice(&row.get::<Vec<u8>, _>(column)).map_err(internal)
}

fn optional_uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Option<Uuid>, AppError> {
    row.get::<Option<Vec<u8>>, _>(column)
        .as_deref()
        .map(Uuid::from_slice)
        .transpose()
        .map_err(internal)
}

fn validation(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}

fn invalid(kind: &str) -> AppError {
    AppError::new(ErrorCode::Internal, format!("stored {kind} is invalid"))
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
