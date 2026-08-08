use sha2::{Digest as _, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use crate::discovery::model::{RelativePath, RootId};
use crate::shared::error::{AppError, ErrorCode};

use super::model::{
    ConfirmedNfoInput, OrganizationNamingPattern, OrganizationNfoPolicy, OrganizationOperation,
    OrganizationRuleInput, OrganizationTarget, OrganizationTargetKind,
};
use super::nfo::{NfoDecision, NfoGenerator};
use super::planner::{
    ConfigProvenance, OrganizationLocation, PlanAuthorization, PlanDraft, PlannedOperationDraft,
    PlannedOperationKind, PlanningRiskCode, ProvenanceSourceKind,
};

/// 已分配稳定 ID 的不可变计划步骤。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationPlannedOperation {
    /// operation 稳定 ID。
    pub id: Uuid,
    /// 操作类别。
    pub kind: PlannedOperationKind,
    /// 能力安全目标位置。
    pub destination: OrganizationLocation,
}

/// 从不可变持久事实重建的计划与配置快照。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrganizationPlanRecord {
    /// 计划稳定 ID。
    pub id: Uuid,
    /// 配置快照稳定 ID。
    pub snapshot_id: Uuid,
    /// `ProcessingTask` 稳定 ID。
    pub task_id: Uuid,
    /// 任务内单调计划版本。
    pub version: i64,
    /// 当前有效授权；可由计划专属授权回执提升为 one-time。
    pub authorization: PlanAuthorization,
    /// 已分配稳定 ID 的全部步骤。
    pub operations: Vec<OrganizationPlannedOperation>,
    /// 完整不可变草稿，用于重算、审计和后续 executor 输入。
    pub draft: PlanDraft,
    /// 创建时间 Unix epoch 微秒。
    pub created_at_us: i64,
}

#[derive(Clone)]
/// 配置快照、计划、operation 和幂等回执的事务化 `SQLite` 边界。
pub struct OrganizationPlanStore {
    pool: SqlitePool,
}

struct PersistedIds {
    plan_id: Uuid,
}

impl OrganizationPlanStore {
    /// 使用已有连接池创建 store。
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 在一个即时事务中创建任务的下一个不可变计划版本。
    ///
    /// # Errors
    ///
    /// 草稿越界、外键、数据库或持久化校验失败时返回稳定应用错误。
    pub async fn persist_next(
        &self,
        account_id: Uuid,
        draft: PlanDraft,
        now_us: i64,
    ) -> Result<OrganizationPlanRecord, AppError> {
        validate_draft(&draft)?;
        let fingerprint = draft_fingerprint(&draft)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        let ids = persist_in_transaction(&mut tx, account_id, &draft, &fingerprint, now_us).await?;
        tx.commit().await.map_err(database_error)?;
        self.get(account_id, ids.plan_id)
            .await?
            .ok_or_else(|| invalid_database("persisted organization plan is missing"))
    }

    /// 以 `(account,idempotency-key)` 保证重算只创建一次新版本。
    ///
    /// 相同 key 与草稿精确重放返回同一计划；不同草稿返回请求冲突。
    ///
    /// # Errors
    ///
    /// key/草稿无效、幂等绑定冲突或数据库失败时返回稳定应用错误。
    pub async fn recalculate(
        &self,
        account_id: Uuid,
        idempotency_key: &str,
        draft: PlanDraft,
        now_us: i64,
    ) -> Result<OrganizationPlanRecord, AppError> {
        validate_key(idempotency_key)?;
        validate_draft(&draft)?;
        let key_hash = digest(&[
            b"mediaflow.organization.recalculate.v1\0",
            idempotency_key.as_bytes(),
        ]);
        let request_digest = draft_fingerprint(&draft)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(database_error)?;
        if let Some((stored_digest, plan_id)) = sqlx::query_as::<_, (Vec<u8>, Vec<u8>)>(
            "SELECT request_digest,result_plan_id
             FROM organization_plan_recalculation_receipts
             WHERE account_id=? AND idempotency_key_sha256=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        {
            if stored_digest != request_digest {
                return Err(request_conflict());
            }
            let plan_id = Uuid::from_slice(&plan_id).map_err(invalid_database)?;
            tx.commit().await.map_err(database_error)?;
            return self
                .get(account_id, plan_id)
                .await?
                .ok_or_else(|| invalid_database("recalculation receipt plan is missing"));
        }
        let ids =
            persist_in_transaction(&mut tx, account_id, &draft, &request_digest, now_us).await?;
        sqlx::query(
            "INSERT INTO organization_plan_recalculation_receipts
             (account_id,task_id,idempotency_key_sha256,request_digest,result_plan_id,created_at_us)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(draft.task_id.as_bytes().as_slice())
        .bind(key_hash.as_slice())
        .bind(request_digest.as_slice())
        .bind(ids.plan_id.as_bytes().as_slice())
        .bind(now_us)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        self.get(account_id, ids.plan_id)
            .await?
            .ok_or_else(|| invalid_database("recalculated organization plan is missing"))
    }

    /// 保存只绑定当前计划版本的一次性授权回执。
    ///
    /// # Errors
    ///
    /// 计划不是当前版本、包含不可覆盖风险、key 冲突或数据库失败时返回稳定应用错误。
    pub async fn authorize_once(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        plan_version: i64,
        idempotency_key: &str,
        now_us: i64,
    ) -> Result<OrganizationPlanRecord, AppError> {
        validate_key(idempotency_key)?;
        self.assert_current(account_id, task_id, plan_version)
            .await?;
        let current = self
            .current(account_id, task_id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::NotFound, "organization plan not found"))?;
        if current.draft.risk_codes.iter().any(|risk| {
            !matches!(
                risk,
                PlanningRiskCode::RuleNotMatched | PlanningRiskCode::AutomaticDisabled
            )
        }) {
            return Err(AppError::new(
                ErrorCode::ResourceConflict,
                "organization plan has non-overridable risks",
            ));
        }
        let key_hash = digest(&[
            b"mediaflow.organization.authorize.v1\0",
            idempotency_key.as_bytes(),
        ]);
        let result = sqlx::query(
            "INSERT INTO organization_plan_authorization_receipts
             (plan_id,account_id,task_id,plan_version,idempotency_key_sha256,created_at_us)
             VALUES (?,?,?,?,?,?) ON CONFLICT(plan_id) DO NOTHING",
        )
        .bind(current.id.as_bytes().as_slice())
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .bind(plan_version)
        .bind(key_hash.as_slice())
        .bind(now_us)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 0 {
            let stored = sqlx::query_scalar::<_, Vec<u8>>(
                "SELECT idempotency_key_sha256 FROM organization_plan_authorization_receipts
                 WHERE plan_id=?",
            )
            .bind(current.id.as_bytes().as_slice())
            .fetch_one(&self.pool)
            .await
            .map_err(database_error)?;
            if stored != key_hash {
                return Err(request_conflict());
            }
        }
        self.get(account_id, current.id)
            .await?
            .ok_or_else(|| invalid_database("authorized organization plan is missing"))
    }

    /// 断言期望版本仍是任务的当前计划版本。
    ///
    /// # Errors
    ///
    /// 计划不存在、版本过期或数据库失败时返回稳定应用错误。
    pub async fn assert_current(
        &self,
        account_id: Uuid,
        task_id: Uuid,
        expected_version: i64,
    ) -> Result<(), AppError> {
        let current = sqlx::query_scalar::<_, i64>(
            "SELECT version FROM organization_plans
             WHERE account_id=? AND task_id=? ORDER BY version DESC LIMIT 1",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        if current != Some(expected_version) {
            return Err(version_conflict());
        }
        Ok(())
    }

    /// 返回任务的当前最高计划版本。
    ///
    /// # Errors
    ///
    /// 数据库或持久投影无效时返回稳定应用错误。
    pub async fn current(
        &self,
        account_id: Uuid,
        task_id: Uuid,
    ) -> Result<Option<OrganizationPlanRecord>, AppError> {
        let id = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM organization_plans
             WHERE account_id=? AND task_id=? ORDER BY version DESC LIMIT 1",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        let Some(id) = id else {
            return Ok(None);
        };
        self.get(account_id, Uuid::from_slice(&id).map_err(invalid_database)?)
            .await
    }

    /// 按账户和稳定 ID 读取不可变计划。
    ///
    /// # Errors
    ///
    /// 数据库或持久投影无效时返回稳定应用错误。
    #[allow(clippy::too_many_lines)]
    pub async fn get(
        &self,
        account_id: Uuid,
        plan_id: Uuid,
    ) -> Result<Option<OrganizationPlanRecord>, AppError> {
        let row = sqlx::query(
            "SELECT p.id,p.snapshot_id,p.task_id,p.file_revision_id,p.selected_identity_id,
                    p.version,p.source_root_id,p.source_relative_path,p.destination_root_id,
                    p.destination_relative_path,p.operation AS plan_operation,p.naming,p.authorization,
                    p.risk_codes_json,p.created_at_us,
                    s.target_id,s.target_config_version,s.kind,s.display_name,s.root_id,
                    s.relative_path_display,s.operation AS target_operation,s.naming_pattern,s.nfo_policy,
                    s.automatic,s.enabled
             FROM organization_plans p
             JOIN organization_config_snapshots s ON s.id=p.snapshot_id
             WHERE p.account_id=? AND p.id=?",
        )
        .bind(account_id.as_bytes().as_slice())
        .bind(plan_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let snapshot_id = row_uuid(&row, "snapshot_id")?;
        let target_id = row_uuid(&row, "target_id")?;
        let rules = load_rules(&self.pool, snapshot_id).await?;
        let provenance = load_provenance(&self.pool, snapshot_id).await?;
        let operations = load_operations(&self.pool, plan_id).await?;
        let nfo_input = load_nfo_input(&self.pool, plan_id).await?;
        let stored_authorization = PlanAuthorization::parse(&row.get::<String, _>("authorization"))
            .ok_or_else(|| invalid_database("invalid plan authorization"))?;
        let authorized = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM organization_plan_authorization_receipts WHERE plan_id=?",
        )
        .bind(plan_id.as_bytes().as_slice())
        .fetch_one(&self.pool)
        .await
        .map_err(database_error)?
            == 1;
        let risk_strings =
            serde_json::from_str::<Vec<String>>(&row.get::<String, _>("risk_codes_json"))
                .map_err(invalid_database)?;
        let risk_codes = risk_strings
            .into_iter()
            .map(|risk| {
                PlanningRiskCode::parse(&risk)
                    .ok_or_else(|| invalid_database("invalid planning risk code"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let source = OrganizationLocation {
            root_id: RootId::parse(&row.get::<String, _>("source_root_id"))
                .map_err(invalid_database)?,
            relative_path: RelativePath::parse(&row.get::<String, _>("source_relative_path"))
                .map_err(invalid_database)?,
        };
        let destination = OrganizationLocation {
            root_id: RootId::parse(&row.get::<String, _>("destination_root_id"))
                .map_err(invalid_database)?,
            relative_path: RelativePath::parse(&row.get::<String, _>("destination_relative_path"))
                .map_err(invalid_database)?,
        };
        let target = OrganizationTarget {
            id: target_id,
            kind: OrganizationTargetKind::parse(&row.get::<String, _>("kind"))
                .ok_or_else(|| invalid_database("invalid snapshot kind"))?,
            display_name: row.get("display_name"),
            root_id: RootId::parse(&row.get::<String, _>("root_id")).map_err(invalid_database)?,
            relative_path: RelativePath::parse(&row.get::<String, _>("relative_path_display"))
                .map_err(invalid_database)?,
            operation: OrganizationOperation::parse(&row.get::<String, _>("target_operation"))
                .ok_or_else(|| invalid_database("invalid snapshot operation"))?,
            naming_pattern: OrganizationNamingPattern::parse(
                &row.get::<String, _>("naming_pattern"),
            )
            .ok_or_else(|| invalid_database("invalid snapshot naming pattern"))?,
            nfo_policy: OrganizationNfoPolicy::parse(&row.get::<String, _>("nfo_policy"))
                .ok_or_else(|| invalid_database("invalid snapshot NFO policy"))?,
            automatic: decode_bool(row.get("automatic"))?,
            enabled: decode_bool(row.get("enabled"))?,
            rules,
            config_version: row.get("target_config_version"),
            updated_at_us: row.get("created_at_us"),
        };
        let operation = OrganizationOperation::parse(&row.get::<String, _>("plan_operation"))
            .ok_or_else(|| invalid_database("invalid plan operation"))?;
        let draft = PlanDraft {
            task_id: row_uuid(&row, "task_id")?,
            file_revision_id: row_uuid(&row, "file_revision_id")?,
            selected_identity_id: optional_row_uuid(&row, "selected_identity_id")?,
            source,
            destination,
            operation,
            naming: row.get("naming"),
            authorization: stored_authorization,
            risk_codes,
            target,
            nfo_input,
            provenance,
            operations: operations
                .iter()
                .map(|operation| PlannedOperationDraft {
                    kind: operation.kind,
                    destination: operation.destination.clone(),
                })
                .collect(),
        };
        Ok(Some(OrganizationPlanRecord {
            id: plan_id,
            snapshot_id,
            task_id: draft.task_id,
            version: row.get("version"),
            authorization: if authorized {
                PlanAuthorization::OneTime
            } else {
                stored_authorization
            },
            operations,
            draft,
            created_at_us: row.get("created_at_us"),
        }))
    }
}

#[allow(clippy::too_many_lines)]
async fn persist_in_transaction(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: Uuid,
    draft: &PlanDraft,
    fingerprint: &[u8; 32],
    now_us: i64,
) -> Result<PersistedIds, AppError> {
    let version = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(MAX(version),0)+1 FROM organization_plans WHERE task_id=?",
    )
    .bind(draft.task_id.as_bytes().as_slice())
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    let plan_id = Uuid::now_v7();
    let snapshot_id = Uuid::now_v7();
    let target = &draft.target;
    sqlx::query(
        "INSERT INTO organization_config_snapshots
         (id,target_id,target_config_version,kind,display_name,root_id,relative_path_display,
          operation,naming_pattern,nfo_policy,automatic,enabled,created_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(snapshot_id.as_bytes().as_slice())
    .bind(target.id.as_bytes().as_slice())
    .bind(target.config_version)
    .bind(target.kind.as_str())
    .bind(&target.display_name)
    .bind(target.root_id.as_str())
    .bind(target.relative_path.as_str())
    .bind(target.operation.as_str())
    .bind(target.naming_pattern.as_str())
    .bind(target.nfo_policy.as_str())
    .bind(i64::from(target.automatic))
    .bind(i64::from(target.enabled))
    .bind(now_us)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    for (ordinal, rule) in target.rules.iter().enumerate() {
        sqlx::query(
            "INSERT INTO organization_config_snapshot_rules
             (snapshot_id,ordinal,media_kind,inbox_directory_id,explicit_tag,enabled)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(snapshot_id.as_bytes().as_slice())
        .bind(i64::try_from(ordinal).map_err(invalid_database)?)
        .bind(rule.media_kind.as_str())
        .bind(rule.inbox_directory_id.map(|id| id.as_bytes().to_vec()))
        .bind(rule.explicit_tag.as_deref())
        .bind(i64::from(rule.enabled))
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    }
    for (ordinal, value) in draft.provenance.iter().enumerate() {
        sqlx::query(
            "INSERT INTO organization_config_provenance
             (snapshot_id,ordinal,field_name,source_kind,source_id,source_version)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(snapshot_id.as_bytes().as_slice())
        .bind(i64::try_from(ordinal).map_err(invalid_database)?)
        .bind(&value.field)
        .bind(value.source_kind.as_str())
        .bind(value.source_id.map(|id| id.as_bytes().to_vec()))
        .bind(value.source_version)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    }
    let risk_codes = draft
        .risk_codes
        .iter()
        .map(|risk| risk.as_str())
        .collect::<Vec<_>>();
    let risk_codes_json = serde_json::to_string(&risk_codes).map_err(invalid_database)?;
    sqlx::query(
        "INSERT INTO organization_plans
         (id,account_id,task_id,file_revision_id,selected_identity_id,version,target_id,
          snapshot_id,source_root_id,source_relative_path,destination_root_id,
          destination_relative_path,operation,naming,authorization,risk_codes_json,
          config_fingerprint,created_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(plan_id.as_bytes().as_slice())
    .bind(account_id.as_bytes().as_slice())
    .bind(draft.task_id.as_bytes().as_slice())
    .bind(draft.file_revision_id.as_bytes().as_slice())
    .bind(draft.selected_identity_id.map(|id| id.as_bytes().to_vec()))
    .bind(version)
    .bind(target.id.as_bytes().as_slice())
    .bind(snapshot_id.as_bytes().as_slice())
    .bind(draft.source.root_id.as_str())
    .bind(draft.source.relative_path.as_str())
    .bind(draft.destination.root_id.as_str())
    .bind(draft.destination.relative_path.as_str())
    .bind(draft.operation.as_str())
    .bind(&draft.naming)
    .bind(draft.authorization.as_str())
    .bind(risk_codes_json)
    .bind(fingerprint.as_slice())
    .bind(now_us)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    for (ordinal, operation) in draft.operations.iter().enumerate() {
        sqlx::query(
            "INSERT INTO organization_plan_operations
             (id,plan_id,ordinal,kind,destination_root_id,destination_relative_path)
             VALUES (?,?,?,?,?,?)",
        )
        .bind(Uuid::now_v7().as_bytes().as_slice())
        .bind(plan_id.as_bytes().as_slice())
        .bind(i64::try_from(ordinal).map_err(invalid_database)?)
        .bind(operation.kind.as_str())
        .bind(operation.destination.root_id.as_str())
        .bind(operation.destination.relative_path.as_str())
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    }
    if let Some(input) = &draft.nfo_input {
        let input_json = serde_json::to_string(input).map_err(invalid_database)?;
        sqlx::query("INSERT INTO organization_plan_nfo_inputs (plan_id,input_json) VALUES (?,?)")
            .bind(plan_id.as_bytes().as_slice())
            .bind(input_json)
            .execute(&mut **tx)
            .await
            .map_err(database_error)?;
    }
    Ok(PersistedIds { plan_id })
}

async fn load_nfo_input(
    pool: &SqlitePool,
    plan_id: Uuid,
) -> Result<Option<ConfirmedNfoInput>, AppError> {
    sqlx::query_scalar::<_, String>(
        "SELECT input_json FROM organization_plan_nfo_inputs WHERE plan_id=?",
    )
    .bind(plan_id.as_bytes().as_slice())
    .fetch_optional(pool)
    .await
    .map_err(database_error)?
    .map(|value| serde_json::from_str(&value).map_err(invalid_database))
    .transpose()
}

async fn load_rules(
    pool: &SqlitePool,
    snapshot_id: Uuid,
) -> Result<Vec<OrganizationRuleInput>, AppError> {
    sqlx::query(
        "SELECT media_kind,inbox_directory_id,explicit_tag,enabled
         FROM organization_config_snapshot_rules WHERE snapshot_id=? ORDER BY ordinal",
    )
    .bind(snapshot_id.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|row| {
        Ok(OrganizationRuleInput {
            media_kind: OrganizationTargetKind::parse(&row.get::<String, _>("media_kind"))
                .ok_or_else(|| invalid_database("invalid snapshot rule kind"))?,
            inbox_directory_id: optional_row_uuid(&row, "inbox_directory_id")?,
            explicit_tag: row.get("explicit_tag"),
            enabled: decode_bool(row.get("enabled"))?,
        })
    })
    .collect()
}

async fn load_provenance(
    pool: &SqlitePool,
    snapshot_id: Uuid,
) -> Result<Vec<ConfigProvenance>, AppError> {
    sqlx::query(
        "SELECT field_name,source_kind,source_id,source_version
         FROM organization_config_provenance WHERE snapshot_id=? ORDER BY ordinal",
    )
    .bind(snapshot_id.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|row| {
        Ok(ConfigProvenance {
            field: row.get("field_name"),
            source_kind: ProvenanceSourceKind::parse(&row.get::<String, _>("source_kind"))
                .ok_or_else(|| invalid_database("invalid provenance source kind"))?,
            source_id: optional_row_uuid(&row, "source_id")?,
            source_version: row.get("source_version"),
        })
    })
    .collect()
}

async fn load_operations(
    pool: &SqlitePool,
    plan_id: Uuid,
) -> Result<Vec<OrganizationPlannedOperation>, AppError> {
    sqlx::query(
        "SELECT id,kind,destination_root_id,destination_relative_path
         FROM organization_plan_operations WHERE plan_id=? ORDER BY ordinal",
    )
    .bind(plan_id.as_bytes().as_slice())
    .fetch_all(pool)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|row| {
        Ok(OrganizationPlannedOperation {
            id: row_uuid(&row, "id")?,
            kind: PlannedOperationKind::parse(&row.get::<String, _>("kind"))
                .ok_or_else(|| invalid_database("invalid planned operation kind"))?,
            destination: OrganizationLocation {
                root_id: RootId::parse(&row.get::<String, _>("destination_root_id"))
                    .map_err(invalid_database)?,
                relative_path: RelativePath::parse(
                    &row.get::<String, _>("destination_relative_path"),
                )
                .map_err(invalid_database)?,
            },
        })
    })
    .collect()
}

fn validate_draft(draft: &PlanDraft) -> Result<(), AppError> {
    let nfo_operations = draft
        .operations
        .iter()
        .filter(|operation| operation.kind == PlannedOperationKind::EnsureMissingNfo)
        .collect::<Vec<_>>();
    let nfo_valid = match (&draft.nfo_input, nfo_operations.as_slice()) {
        (None, []) => true,
        (Some(input), [operation]) => {
            operation
                .destination
                .relative_path
                .as_str()
                .rsplit('/')
                .next()
                == Some(input.file_name.as_str())
                && matches!(
                    NfoGenerator.decide(input, None),
                    Ok(NfoDecision::Generate { .. })
                )
        }
        _ => false,
    };
    if draft.operations.is_empty()
        || draft.operations.len() > 64
        || draft.provenance.is_empty()
        || draft.provenance.len() > 64
        || draft.risk_codes.len() > 32
        || !nfo_valid
        || !(1..=512).contains(&draft.naming.chars().count())
        || draft
            .provenance
            .iter()
            .any(|value| !(1..=64).contains(&value.field.len()) || value.source_version < 1)
    {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid organization plan draft",
        ));
    }
    Ok(())
}

fn validate_key(value: &str) -> Result<(), AppError> {
    if value.is_empty() || value.len() > 255 || value.chars().any(char::is_control) {
        return Err(AppError::new(
            ErrorCode::ValidationFailed,
            "invalid organization idempotency key",
        ));
    }
    Ok(())
}

fn draft_fingerprint(draft: &PlanDraft) -> Result<[u8; 32], AppError> {
    let bytes = serde_json::to_vec(draft).map_err(invalid_database)?;
    Ok(digest(&[b"mediaflow.organization-plan-draft.v1\0", &bytes]))
}

fn digest(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn row_uuid(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<Uuid, AppError> {
    Uuid::from_slice(&row.get::<Vec<u8>, _>(column)).map_err(invalid_database)
}

fn optional_row_uuid(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<Option<Uuid>, AppError> {
    row.get::<Option<Vec<u8>>, _>(column)
        .map(|value| Uuid::from_slice(&value).map_err(invalid_database))
        .transpose()
}

fn decode_bool(value: i64) -> Result<bool, AppError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid_database("invalid SQLite boolean")),
    }
}

fn database_error(error: sqlx::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}

fn invalid_database(error: impl std::fmt::Display) -> AppError {
    AppError::new(ErrorCode::Internal, error.to_string())
}

fn request_conflict() -> AppError {
    AppError::new(
        ErrorCode::RequestConflict,
        "organization idempotency key is already bound",
    )
}

fn version_conflict() -> AppError {
    AppError::new(
        ErrorCode::ConfigVersionConflict,
        "organization plan version changed",
    )
}
