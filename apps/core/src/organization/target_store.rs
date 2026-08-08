use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use uuid::Uuid;

use crate::discovery::model::{DeploymentRootView, RelativePath, RootAccess, RootId};
use crate::discovery::organization_projection::has_inbox_overlap_on_connection;
use crate::platform::outbox::OutboxNotifier;
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::page::{CursorPage, PageRequest};

use super::model::{
    OrganizationNamingPattern, OrganizationNfoPolicy, OrganizationOperation, OrganizationRuleInput,
    OrganizationTarget, OrganizationTargetInput, OrganizationTargetKind,
};

type TargetRow = (
    Vec<u8>,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
    i64,
);

type RuleRow = (String, Option<Vec<u8>>, Option<String>, i64);

#[derive(Deserialize, Serialize)]
struct TargetCursor {
    version: u8,
    updated_at_us: i64,
    id: Uuid,
}

#[derive(Deserialize, Serialize)]
struct CursorEnvelope {
    payload: String,
    checksum: String,
}

/// 目标写入前由应用服务通过部署配置和 discovery 投影得到的边界结论。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationTargetBoundary {
    /// 与输入 `root_id` 对应、不含宿主路径的部署根投影。
    pub root: DeploymentRootView,
    /// 候选路径是否与收件箱相等、包含或被包含。
    pub overlaps_inbox: bool,
}

/// 整理目标、profile 和规则聚合的 `SQLite` 边界。
#[derive(Clone)]
pub struct OrganizationTargetStore {
    pool: SqlitePool,
    notifier: OutboxNotifier,
}

impl OrganizationTargetStore {
    /// 使用已有连接池创建 store。
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self::new_with_notifier(pool, OutboxNotifier::new())
    }

    /// 使用共享通知器创建 store，使目标事件提交后立即唤醒 SSE。
    #[must_use]
    pub fn new_with_notifier(pool: SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    /// 按最近更新时间稳定列出账户拥有的安全目标投影。
    ///
    /// # Errors
    ///
    /// 数据库不可用或持久行不符合领域约束时返回内部错误。
    pub async fn list(&self, account_id: Uuid) -> Result<Vec<OrganizationTarget>, AppError> {
        let ids = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM organization_targets
             WHERE account_id=? ORDER BY updated_at_us DESC,id DESC",
        )
        .bind(account_id.as_bytes().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        let mut targets = Vec::with_capacity(ids.len());
        for id in ids {
            let id = Uuid::from_slice(&id).map_err(invalid_database)?;
            targets.push(
                self.get(account_id, id)
                    .await?
                    .ok_or_else(|| invalid_database("listed organization target is missing"))?,
            );
        }
        Ok(targets)
    }

    /// 按 `(updated_at_us,id)` 倒序返回一个有界稳定游标页。
    ///
    /// # Errors
    ///
    /// 页大小、游标、数据库或持久行无效时返回稳定应用错误。
    pub async fn list_page(
        &self,
        account_id: Uuid,
        page: &PageRequest,
    ) -> Result<CursorPage<OrganizationTarget>, AppError> {
        if page.limit == 0 || page.limit > crate::shared::page::MAX_PAGE_LIMIT {
            return Err(validation_error(
                "organization target page is outside bounds",
            ));
        }
        let cursor = page.cursor.as_deref().map(decode_cursor).transpose()?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT id,updated_at_us FROM organization_targets WHERE account_id=",
        );
        query.push_bind(account_id.as_bytes().to_vec());
        if let Some(cursor) = &cursor {
            query
                .push(" AND (updated_at_us<")
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
            .build_query_as::<(Vec<u8>, i64)>()
            .fetch_all(&self.pool)
            .await
            .map_err(database_error)?;
        let has_more = rows.len() > page.limit as usize;
        let rows = rows.into_iter().take(page.limit as usize);
        let mut items = Vec::with_capacity(page.limit as usize);
        for (id, _) in rows {
            let id = Uuid::from_slice(&id).map_err(invalid_database)?;
            items.push(
                self.get(account_id, id)
                    .await?
                    .ok_or_else(|| invalid_database("paged organization target is missing"))?,
            );
        }
        let next_cursor = if has_more {
            items
                .last()
                .map(|item| {
                    encode_cursor(&TargetCursor {
                        version: 1,
                        updated_at_us: item.updated_at_us,
                        id: item.id,
                    })
                })
                .transpose()?
        } else {
            None
        };
        Ok(CursorPage { items, next_cursor })
    }

    /// 判断同账户同逻辑根内是否存在重叠目标，可排除当前被替换的目标。
    ///
    /// # Errors
    ///
    /// 数据库不可用或持久路径无效时返回内部错误。
    pub async fn has_overlap(
        &self,
        account_id: Uuid,
        root_id: &RootId,
        path: &RelativePath,
        excluded_target_id: Option<Uuid>,
    ) -> Result<bool, AppError> {
        let rows = sqlx::query_as::<_, (Vec<u8>, String)>(
            "SELECT id,relative_path_display FROM organization_targets
             WHERE account_id=? AND root_id=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(root_id.as_str())
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        paths_overlap(rows, path, excluded_target_id)
    }

    /// 读取账户拥有的单个安全目标投影。
    ///
    /// # Errors
    ///
    /// 数据库不可用或持久行不符合领域约束时返回内部错误。
    pub async fn get(
        &self,
        account_id: Uuid,
        target_id: Uuid,
    ) -> Result<Option<OrganizationTarget>, AppError> {
        let row = sqlx::query_as::<_, TargetRow>(
            "SELECT t.id,t.kind,t.display_name,t.root_id,t.relative_path_display,
                    p.operation,p.naming_pattern,p.nfo_policy,p.automatic,p.enabled,
                    t.config_version,t.updated_at_us
             FROM organization_targets t
             JOIN organization_profiles p ON p.target_id=t.id
             WHERE t.account_id=? AND t.id=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(target_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let rules = load_rules(&self.pool, target_id).await?;
        decode_target(row, rules).map(Some)
    }

    /// 插入配置版本为 1 的完整目标聚合。
    ///
    /// # Errors
    ///
    /// 输入、能力、overlap、账户或数据库约束失败时返回稳定应用错误。
    pub async fn insert(
        &self,
        account_id: Uuid,
        target_id: Uuid,
        input: OrganizationTargetInput,
        boundary: &OrganizationTargetBoundary,
        now_us: i64,
    ) -> Result<OrganizationTarget, AppError> {
        let input = validate_boundary(input.validate()?, boundary)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        if has_inbox_overlap_on_connection(&mut tx, &input.root_id, &input.relative_path).await? {
            return Err(overlap_error());
        }
        reject_target_overlap(&mut tx, account_id, None, &input).await?;
        let result = sqlx::query(
            "INSERT INTO organization_targets
             (id,account_id,kind,display_name,root_id,relative_path_bytes,
              relative_path_display,config_version,created_at_us,updated_at_us)
             VALUES (?,?,?,?,?,?,?,1,?,?)",
        )
        .bind(target_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(input.kind.as_str())
        .bind(&input.display_name)
        .bind(input.root_id.as_str())
        .bind(input.relative_path.bytes())
        .bind(input.relative_path.as_str())
        .bind(now_us)
        .bind(now_us)
        .execute(&mut *tx)
        .await;
        match result {
            Ok(_) => {}
            Err(error) if is_unique_constraint(&error) => return Err(overlap_error()),
            Err(error) => return Err(database_error(error)),
        }
        insert_profile(&mut tx, target_id, &input, 1, now_us, now_us).await?;
        insert_rules(&mut tx, target_id, &input.rules, 1, now_us, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.get(account_id, target_id)
            .await?
            .ok_or_else(|| invalid_database("inserted organization target is missing"))
    }

    /// 以当前聚合版本原子替换目标、profile 和完整规则集合。
    ///
    /// # Errors
    ///
    /// 输入、能力、overlap、数据库或乐观并发失败时返回稳定应用错误。
    pub async fn replace(
        &self,
        account_id: Uuid,
        target_id: Uuid,
        expected_version: i64,
        input: OrganizationTargetInput,
        boundary: &OrganizationTargetBoundary,
        now_us: i64,
    ) -> Result<OrganizationTarget, AppError> {
        if expected_version < 1 {
            return Err(version_conflict());
        }
        let next_version = expected_version
            .checked_add(1)
            .ok_or_else(version_conflict)?;
        let input = validate_boundary(input.validate()?, boundary)?;
        if let Some(current) = self.get(account_id, target_id).await?
            && current.config_version == expected_version
            && target_matches_input(&current, &input)
        {
            return Ok(current);
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        if has_inbox_overlap_on_connection(&mut tx, &input.root_id, &input.relative_path).await? {
            return Err(overlap_error());
        }
        reject_target_overlap(&mut tx, account_id, Some(target_id), &input).await?;
        let result = sqlx::query(
            "UPDATE organization_targets
             SET kind=?,display_name=?,root_id=?,relative_path_bytes=?,relative_path_display=?,
                 config_version=?,updated_at_us=?
             WHERE id=? AND account_id=? AND config_version=?",
        )
        .bind(input.kind.as_str())
        .bind(&input.display_name)
        .bind(input.root_id.as_str())
        .bind(input.relative_path.bytes())
        .bind(input.relative_path.as_str())
        .bind(next_version)
        .bind(now_us)
        .bind(target_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&mut *tx)
        .await;
        let rows = match result {
            Ok(result) => result.rows_affected(),
            Err(error) if is_unique_constraint(&error) => return Err(overlap_error()),
            Err(error) => return Err(database_error(error)),
        };
        if rows != 1 {
            return Err(version_conflict());
        }
        let profile_rows = sqlx::query(
            "UPDATE organization_profiles
             SET operation=?,naming_pattern=?,nfo_policy=?,automatic=?,enabled=?,
                 config_version=?,updated_at_us=? WHERE target_id=? AND config_version=?",
        )
        .bind(input.operation.as_str())
        .bind(input.naming_pattern.as_str())
        .bind(input.nfo_policy.as_str())
        .bind(i64::from(input.automatic))
        .bind(i64::from(input.enabled))
        .bind(next_version)
        .bind(now_us)
        .bind(target_id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if profile_rows.rows_affected() != 1 {
            return Err(invalid_database("organization profile version diverged"));
        }
        sqlx::query("DELETE FROM organization_rules WHERE target_id=?")
            .bind(target_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        insert_rules(
            &mut tx,
            target_id,
            &input.rules,
            next_version,
            now_us,
            now_us,
        )
        .await?;
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        self.get(account_id, target_id)
            .await?
            .ok_or_else(|| invalid_database("replaced organization target is missing"))
    }

    /// 以当前聚合版本删除目标；profile 与规则由外键级联删除。
    ///
    /// 仍无 completed/compensated `LocalResult` 的活动计划会阻止删除；历史快照不永久绑定目标。
    ///
    /// # Errors
    ///
    /// 数据库或乐观并发失败时返回稳定应用错误。
    pub async fn delete(
        &self,
        account_id: Uuid,
        target_id: Uuid,
        expected_version: i64,
    ) -> Result<(), AppError> {
        if expected_version < 1 {
            return Err(version_conflict());
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let active_plans = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM organization_plans p
             WHERE p.target_id=? AND NOT EXISTS (
               SELECT 1 FROM organization_local_results r
               WHERE r.plan_id=p.id AND r.status IN ('completed','compensated')
             )",
        )
        .bind(target_id.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if active_plans != 0 {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "organization target is referenced by an active plan",
            ));
        }
        let rows = sqlx::query(
            "DELETE FROM organization_targets
             WHERE id=? AND account_id=? AND config_version=?",
        )
        .bind(target_id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(expected_version)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        if rows.rows_affected() != 1 {
            return Err(version_conflict());
        }
        tx.commit().await.map_err(database_error)?;
        self.notifier.notify_after_commit();
        Ok(())
    }
}

fn target_matches_input(target: &OrganizationTarget, input: &OrganizationTargetInput) -> bool {
    target.kind == input.kind
        && target.display_name == input.display_name
        && target.root_id == input.root_id
        && target.relative_path == input.relative_path
        && target.operation == input.operation
        && target.naming_pattern == input.naming_pattern
        && target.nfo_policy == input.nfo_policy
        && target.automatic == input.automatic
        && target.enabled == input.enabled
        && target.rules == input.rules
}

async fn reject_target_overlap(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: Uuid,
    excluded_target_id: Option<Uuid>,
    input: &OrganizationTargetInput,
) -> Result<(), AppError> {
    let rows = sqlx::query_as::<_, (Vec<u8>, String)>(
        "SELECT id,relative_path_display FROM organization_targets
         WHERE account_id=? AND root_id=?",
    )
    .bind(account_id.as_bytes().as_slice())
    .bind(input.root_id.as_str())
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?;
    for (id, path) in rows {
        let id = Uuid::from_slice(&id).map_err(invalid_database)?;
        if Some(id) == excluded_target_id {
            continue;
        }
        let path = RelativePath::parse(&path).map_err(invalid_database)?;
        if input.relative_path.overlaps(&path) {
            return Err(overlap_error());
        }
    }
    Ok(())
}

fn validate_boundary(
    input: OrganizationTargetInput,
    boundary: &OrganizationTargetBoundary,
) -> Result<OrganizationTargetInput, AppError> {
    if boundary.root.id != input.root_id || boundary.root.access != RootAccess::ReadWrite {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "organization target requires its configured read-write root",
        ));
    }
    if boundary.overlaps_inbox {
        return Err(overlap_error());
    }
    Ok(input)
}

async fn insert_profile(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target_id: Uuid,
    input: &OrganizationTargetInput,
    config_version: i64,
    created_at_us: i64,
    updated_at_us: i64,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO organization_profiles
         (target_id,operation,naming_pattern,nfo_policy,automatic,enabled,config_version,
          created_at_us,updated_at_us) VALUES (?,?,?,?,?,?,?,?,?)",
    )
    .bind(target_id.as_bytes().as_slice())
    .bind(input.operation.as_str())
    .bind(input.naming_pattern.as_str())
    .bind(input.nfo_policy.as_str())
    .bind(i64::from(input.automatic))
    .bind(i64::from(input.enabled))
    .bind(config_version)
    .bind(created_at_us)
    .bind(updated_at_us)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_rules(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    target_id: Uuid,
    rules: &[OrganizationRuleInput],
    config_version: i64,
    created_at_us: i64,
    updated_at_us: i64,
) -> Result<(), AppError> {
    for (ordinal, rule) in rules.iter().enumerate() {
        sqlx::query(
            "INSERT INTO organization_rules
             (id,target_id,ordinal,media_kind,inbox_directory_id,explicit_tag,enabled,
              config_version,created_at_us,updated_at_us) VALUES (?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(target_id.as_bytes().as_slice())
        .bind(i64::try_from(ordinal).map_err(invalid_database)?)
        .bind(rule.media_kind.as_str())
        .bind(
            rule.inbox_directory_id
                .map(|id| id.as_bytes().as_slice().to_vec()),
        )
        .bind(rule.explicit_tag.as_deref())
        .bind(i64::from(rule.enabled))
        .bind(config_version)
        .bind(created_at_us)
        .bind(updated_at_us)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

async fn load_rules(
    pool: &SqlitePool,
    target_id: Uuid,
) -> Result<Vec<OrganizationRuleInput>, AppError> {
    sqlx::query_as::<_, RuleRow>(
        "SELECT media_kind,inbox_directory_id,explicit_tag,enabled
         FROM organization_rules WHERE target_id=? ORDER BY ordinal",
    )
    .bind(target_id.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|(kind, inbox_id, explicit_tag, enabled)| {
        Ok(OrganizationRuleInput {
            media_kind: OrganizationTargetKind::parse(&kind)
                .ok_or_else(|| invalid_database("unknown organization rule media kind"))?,
            inbox_directory_id: inbox_id
                .map(|id| Uuid::from_slice(&id).map_err(invalid_database))
                .transpose()?,
            explicit_tag,
            enabled: decode_bool(enabled)?,
        })
    })
    .collect()
}

fn decode_target(
    row: TargetRow,
    rules: Vec<OrganizationRuleInput>,
) -> Result<OrganizationTarget, AppError> {
    let (
        id,
        kind,
        display_name,
        root_id,
        relative_path,
        operation,
        naming_pattern,
        nfo_policy,
        automatic,
        enabled,
        config_version,
        updated_at_us,
    ) = row;
    Ok(OrganizationTarget {
        id: Uuid::from_slice(&id).map_err(invalid_database)?,
        kind: OrganizationTargetKind::parse(&kind)
            .ok_or_else(|| invalid_database("unknown organization target kind"))?,
        display_name,
        root_id: RootId::parse(&root_id).map_err(invalid_database)?,
        relative_path: RelativePath::parse(&relative_path).map_err(invalid_database)?,
        operation: OrganizationOperation::parse(&operation)
            .ok_or_else(|| invalid_database("unknown organization operation"))?,
        naming_pattern: OrganizationNamingPattern::parse(&naming_pattern)
            .ok_or_else(|| invalid_database("unknown organization naming pattern"))?,
        nfo_policy: OrganizationNfoPolicy::parse(&nfo_policy)
            .ok_or_else(|| invalid_database("unknown organization NFO policy"))?,
        automatic: decode_bool(automatic)?,
        enabled: decode_bool(enabled)?,
        rules,
        config_version,
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

fn is_unique_constraint(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.is_unique_violation())
}

fn database_error(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid_database(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}

fn overlap_error() -> AppError {
    AppError::new(
        ErrorCode::OrganizationTargetOverlap,
        "organization target overlaps an inbox or another target",
    )
}

fn version_conflict() -> AppError {
    AppError::new(
        ErrorCode::ConfigVersionConflict,
        "organization target version changed",
    )
}

fn paths_overlap(
    rows: Vec<(Vec<u8>, String)>,
    candidate: &RelativePath,
    excluded_target_id: Option<Uuid>,
) -> Result<bool, AppError> {
    for (id, path) in rows {
        let id = Uuid::from_slice(&id).map_err(invalid_database)?;
        if Some(id) == excluded_target_id {
            continue;
        }
        let path = RelativePath::parse(&path).map_err(invalid_database)?;
        if candidate.overlaps(&path) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn encode_cursor(cursor: &TargetCursor) -> Result<String, AppError> {
    let payload = serde_json::to_vec(cursor).map_err(invalid_database)?;
    let envelope = CursorEnvelope {
        payload: URL_SAFE_NO_PAD.encode(&payload),
        checksum: cursor_checksum(&payload),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope).map_err(invalid_database)?);
    if encoded.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(invalid_database(
            "organization target cursor exceeds bounds",
        ));
    }
    Ok(encoded)
}

fn decode_cursor(value: &str) -> Result<TargetCursor, AppError> {
    if value.is_empty() || value.len() > crate::shared::page::MAX_CURSOR_BYTES {
        return Err(validation_error("invalid organization target cursor"));
    }
    let envelope = URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CursorEnvelope>(&bytes).ok())
        .ok_or_else(|| validation_error("invalid organization target cursor"))?;
    let payload = URL_SAFE_NO_PAD
        .decode(envelope.payload)
        .map_err(|_| validation_error("invalid organization target cursor"))?;
    if envelope.checksum != cursor_checksum(&payload) {
        return Err(validation_error("invalid organization target cursor"));
    }
    serde_json::from_slice::<TargetCursor>(&payload)
        .ok()
        .filter(|cursor| cursor.version == 1)
        .ok_or_else(|| validation_error("invalid organization target cursor"))
}

fn cursor_checksum(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mediaflow.organization-target.cursor.v1\0");
    hasher.update(payload);
    hex::encode(&hasher.finalize()[..16])
}

fn validation_error(message: &'static str) -> AppError {
    AppError::new(ErrorCode::ValidationFailed, message)
}
