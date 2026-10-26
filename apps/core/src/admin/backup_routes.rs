//! 备份管理面：管理员手动备份、清单列出与已验证备份的只读信息。
//!
//! 恢复操作保留为运行手册流程（停止 Core → 空目录 → `restore_backup`），不暴露为
//! 运行中 API——恢复语义是进程级重启，运行内恢复会撕裂任务与文件系统事实。

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;

use crate::bootstrap::config::AppConfig;
use crate::identity::service::IdentityService;
use crate::platform::backup::{BackupManifest, create_backup};
use crate::platform::db::Db;
use crate::platform::request_security::{CsrfGuard, SessionGuard, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};

#[derive(Clone)]
struct BackupState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
}

#[derive(Deserialize)]
struct ListQuery {
    limit: Option<i64>,
}

/// 组装备份管理面管理员路由。
pub fn router(config: AppConfig, db: &Db) -> Router {
    Router::new()
        .route(
            "/api/v1/system/backups",
            get(list_backups).post(axum::routing::post(create_manual_backup)),
        )
        .with_state(Arc::new(BackupState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            config,
            db: db.clone(),
        }))
}

impl BackupState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty()
            || !matches!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(250),
                    sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
                )
                .await,
                Ok(Ok(1))
            )
        {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            ));
        }
        Ok(())
    }

    async fn guard_get(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.ensure_ready().await?;
        SessionGuard::authenticate(headers, &self.identity).await?;
        Ok(())
    }

    /// 写通道守卫：认证不滑动会话，再要求同源证明与会话 CSRF 令牌；设备令牌通道按
    /// “令牌即写证明”豁免。拒绝与其他管理写路由一样留下审计记录。
    async fn guard_write(&self, headers: &HeaderMap) -> Result<(), AppError> {
        self.ensure_ready().await?;
        let session = SessionGuard::authenticate_without_sliding(headers, &self.identity).await?;
        if let Err(error) = validate_same_origin(headers, &self.config.public_origin) {
            self.identity.audit_denial("origin").await?;
            return Err(error);
        }
        if let Err(error) = CsrfGuard::validate(headers, &session) {
            self.identity.audit_denial("csrf").await?;
            return Err(error);
        }
        Ok(())
    }

    fn backups_dir(&self) -> std::path::PathBuf {
        self.config.config_dir.join("backups")
    }
}

async fn create_manual_backup(
    State(state): State<Arc<BackupState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    state.guard_write(&headers).await?;
    let artifact = create_backup(
        state.db.pool(),
        &state.config.config_dir.join("mediaflow.db"),
        &state.config.deployment_roots_file,
        &state.backups_dir(),
    )
    .await?;
    let manifest: BackupManifest = serde_json::from_reader(
        std::fs::File::open(&artifact.manifest)
            .map_err(|error| AppError::with_source(ErrorCode::BackupFailed, error))?,
    )
    .map_err(|error| AppError::with_source(ErrorCode::BackupFailed, error))?;
    Ok(Json(serde_json::json!({
        "manifest": manifest,
        "created": true,
    })))
}

async fn list_backups(
    State(state): State<Arc<BackupState>>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    state.guard_get(&headers).await?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let dir = state.backups_dir();
    let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .map(|iterator| {
            iterator
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| b.cmp(a));
    entries.truncate(usize::try_from(limit).unwrap_or(20));
    let mut items = Vec::with_capacity(entries.len());
    for path in entries {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let Ok(manifest) = serde_json::from_reader::<_, BackupManifest>(file) else {
            continue;
        };
        let id = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        items.push(serde_json::json!({
            "id": id,
            "manifest": manifest,
        }));
    }
    Ok(Json(serde_json::json!({ "items": items })))
}
