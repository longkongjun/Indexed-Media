use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use sqlx::Row;
use uuid::Uuid;

use crate::bootstrap::config::AppConfig;
use crate::discovery::capability::{DeploymentRootSet, DeploymentRootsFingerprint};
use crate::discovery::model::RelativePath;
use crate::discovery::policy::ensure_default_policy;
use crate::discovery::watcher::{
    FileWatch, NotifyFileWatch, WATCH_CHANNEL_CAPACITY, WatchCoalescer, WatchIngress,
    WatchReceiver, WatchedInbox,
};
use crate::platform::db::Db;
use crate::platform::outbox::{OutboxNotifier, OutboxWriter};
use crate::shared::error::{AppError, ErrorCode};
use crate::shared::id::new_id;
use crate::tasks::events::{
    INBOX_DISCOVERY_HEALTH_CHANGED, InboxDiscoveryHealth, InboxDiscoveryHealthChangedPayload,
    InboxDiscoveryHealthReason,
};

/// 失败对账后再次尝试完整扫描的固定最小间隔。
pub const RECONCILE_RETRY_US: i64 = 60_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 自动完整扫描的持久来源。
pub enum ScanReason {
    /// Core 启动后立即校正最终事实。
    Startup,
    /// 收件箱策略的周期到期。
    Periodic,
    /// watcher 溢出、事件提示或错误后的完整恢复扫描。
    WatchRecovery,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReconcileSchedule {
    Unavailable,
    Existing(Uuid),
    Created(Uuid),
}

impl ReconcileSchedule {
    const fn task_id(self) -> Option<Uuid> {
        match self {
            Self::Unavailable => None,
            Self::Existing(id) | Self::Created(id) => Some(id),
        }
    }
}

impl ScanReason {
    /// 返回数据库和诊断中使用的稳定值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Periodic => "periodic",
            Self::WatchRecovery => "watch_recovery",
        }
    }
}

#[derive(Clone)]
/// 自动对账任务、健康状态和事件的事务性存储边界。
pub struct CoordinatorStore {
    pool: sqlx::SqlitePool,
    notifier: OutboxNotifier,
}

impl CoordinatorStore {
    #[must_use]
    /// 创建共享事务 outbox 通知器的协调器存储。
    pub fn new(pool: sqlx::SqlitePool, notifier: OutboxNotifier) -> Self {
        Self { pool, notifier }
    }

    async fn ensure_inbox_projections(&self, now_us: i64) -> Result<(), AppError> {
        let rows = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT id FROM discovery_inbox_directories WHERE health='available' ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        let mut connection = self.pool.acquire().await.map_err(internal)?;
        for bytes in rows {
            ensure_default_policy(&mut connection, parse_uuid(&bytes)?, now_us).await?;
        }
        Ok(())
    }

    async fn all_inboxes(&self, watcher_only: bool) -> Result<Vec<Uuid>, AppError> {
        let rows = sqlx::query(
            "SELECT p.inbox_directory_id
             FROM discovery_inbox_policies p
             JOIN discovery_inbox_directories i ON i.id=p.inbox_directory_id
             WHERE i.health='available' AND (?=0 OR p.watcher_enabled=1)
             ORDER BY p.inbox_directory_id",
        )
        .bind(i64::from(watcher_only))
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(|row| parse_uuid(row.get::<Vec<u8>, _>("inbox_directory_id").as_slice()))
            .collect()
    }

    async fn due_inboxes(&self, now_us: i64) -> Result<Vec<Uuid>, AppError> {
        let rows = sqlx::query(
            "SELECT p.inbox_directory_id
             FROM discovery_inbox_policies p
             JOIN discovery_inbox_directories i ON i.id=p.inbox_directory_id
             JOIN discovery_watch_states w ON w.inbox_directory_id=p.inbox_directory_id
             WHERE i.health='available' AND w.active_reconcile_task_id IS NULL
               AND w.next_reconcile_at_us<=?
             ORDER BY w.next_reconcile_at_us,p.inbox_directory_id",
        )
        .bind(now_us)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(|row| parse_uuid(row.get::<Vec<u8>, _>("inbox_directory_id").as_slice()))
            .collect()
    }

    async fn watcher_enabled(&self, inbox_id: Uuid) -> Result<bool, AppError> {
        sqlx::query_scalar::<_, i64>(
            "SELECT watcher_enabled FROM discovery_inbox_policies WHERE inbox_directory_id=?",
        )
        .bind(inbox_id.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await
        .map(|value| value == Some(1))
        .map_err(internal)
    }

    async fn record_event(&self, inbox_id: Uuid, observed_at_us: i64) -> Result<(), AppError> {
        sqlx::query(
            "UPDATE discovery_watch_states
             SET last_event_at_us=MAX(COALESCE(last_event_at_us,?),?),watcher_active=1,
                 health=CASE WHEN health='pending' THEN 'healthy' ELSE health END,
                 version=version+1,updated_at_us=? WHERE inbox_directory_id=?",
        )
        .bind(observed_at_us)
        .bind(observed_at_us)
        .bind(observed_at_us)
        .bind(inbox_id.as_bytes().as_slice())
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(internal)
    }

    /// 幂等确保一个收件箱最多存在一个排队/运行的自动完整扫描。
    async fn schedule_reconcile(
        &self,
        inbox_id: Uuid,
        reason: ScanReason,
        now_us: i64,
    ) -> Result<Option<Uuid>, AppError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        let scheduled = schedule_reconcile_in_tx(&mut tx, inbox_id, reason, now_us).await?;
        tx.commit().await.map_err(internal)?;
        if matches!(scheduled, ReconcileSchedule::Created(_)) {
            self.notifier.notify_after_commit();
        }
        Ok(scheduled.task_id())
    }

    async fn refresh_completed(&self, now_us: i64) -> Result<(), AppError> {
        let rows = sqlx::query(
            "SELECT w.inbox_directory_id,w.active_reconcile_task_id,w.watcher_active,
                    t.status,p.reconcile_interval_seconds,
                    EXISTS(SELECT 1 FROM tasks_scan_errors e
                           WHERE e.task_id=t.id AND e.scope='root'
                             AND e.code IN ('root.unavailable','root.not_found','path.escape',
                                            'path.symlink_forbidden')) AS root_failed
             FROM discovery_watch_states w
             JOIN discovery_inbox_policies p ON p.inbox_directory_id=w.inbox_directory_id
             JOIN tasks_scan_tasks t ON t.id=w.active_reconcile_task_id
             WHERE t.status NOT IN ('queued','running')",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        for row in rows {
            self.finish_projection(&row, now_us).await?;
        }
        Ok(())
    }

    async fn finish_projection(
        &self,
        row: &sqlx::sqlite::SqliteRow,
        now_us: i64,
    ) -> Result<(), AppError> {
        let inbox_id = parse_uuid(row.get::<Vec<u8>, _>("inbox_directory_id").as_slice())?;
        let task_id = parse_uuid(row.get::<Vec<u8>, _>("active_reconcile_task_id").as_slice())?;
        let status: String = row.get("status");
        let root_failed = row.get::<i64, _>("root_failed") != 0;
        let (health, reason, next_at_us) = completion_projection(
            &status,
            root_failed,
            now_us,
            row.get("reconcile_interval_seconds"),
        );
        let watcher_active = row.get::<i64, _>("watcher_active") != 0;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(internal)?;
        sqlx::query(
            "UPDATE discovery_watch_states
             SET health=?,last_error_code=?,last_reconcile_finished_at_us=?,
                 next_reconcile_at_us=?,active_reconcile_task_id=NULL,last_reconcile_task_id=?,
                 requested_reason=NULL,version=version+1,updated_at_us=?
             WHERE inbox_directory_id=? AND active_reconcile_task_id=?",
        )
        .bind(health_value(health))
        .bind(reason.map(reason_value))
        .bind(now_us)
        .bind(next_at_us)
        .bind(task_id.as_bytes().as_slice())
        .bind(now_us)
        .bind(inbox_id.as_bytes().as_slice())
        .bind(task_id.as_bytes().as_slice())
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
        OutboxWriter::write(
            &mut tx,
            INBOX_DISCOVERY_HEALTH_CHANGED,
            inbox_id,
            &InboxDiscoveryHealthChangedPayload {
                inbox_directory_id: inbox_id,
                health,
                watcher_active,
                reason,
            },
            now_us,
        )
        .await?;
        tx.commit().await.map_err(internal)?;
        self.notifier.notify_after_commit();
        Ok(())
    }

    async fn watch_targets(
        &self,
        root_paths: &BTreeMap<String, PathBuf>,
    ) -> Result<Vec<WatchedInbox>, AppError> {
        let rows = sqlx::query(
            "SELECT i.id,i.root_id,i.relative_path_display
             FROM discovery_inbox_directories i
             JOIN discovery_inbox_policies p ON p.inbox_directory_id=i.id
             WHERE i.health='available' AND p.watcher_enabled=1 ORDER BY i.id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(|row| {
                let inbox_directory_id = parse_uuid(row.get::<Vec<u8>, _>("id").as_slice())?;
                let root_id: String = row.get("root_id");
                let root = root_paths.get(&root_id).ok_or_else(|| {
                    AppError::new(ErrorCode::RootUnavailable, "watch root is unavailable")
                })?;
                let relative =
                    RelativePath::parse(row.get::<String, _>("relative_path_display").as_str())
                        .map_err(|_| {
                            AppError::new(ErrorCode::Internal, "stored watch path is invalid")
                        })?;
                Ok(WatchedInbox {
                    inbox_directory_id,
                    host_path: root.join(relative.as_str()),
                })
            })
            .collect()
    }

    async fn set_watcher_targets(
        &self,
        active: &[WatchedInbox],
        now_us: i64,
    ) -> Result<(), AppError> {
        let active = active
            .iter()
            .map(|target| target.inbox_directory_id)
            .collect::<BTreeSet<_>>();
        let rows = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT inbox_directory_id FROM discovery_watch_states ORDER BY inbox_directory_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        for bytes in rows {
            let inbox_id = parse_uuid(&bytes)?;
            sqlx::query(
                "UPDATE discovery_watch_states SET watcher_active=?,version=version+1,updated_at_us=?
                 WHERE inbox_directory_id=? AND watcher_active!=?",
            )
            .bind(i64::from(active.contains(&inbox_id)))
            .bind(now_us)
            .bind(inbox_id.as_bytes().as_slice())
            .bind(i64::from(active.contains(&inbox_id)))
            .execute(&self.pool)
            .await
            .map_err(internal)?;
        }
        Ok(())
    }

    async fn watcher_unavailable(&self, now_us: i64) -> Result<(), AppError> {
        let inboxes = self.all_inboxes(true).await?;
        for inbox_id in inboxes {
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(internal)?;
            sqlx::query(
                "UPDATE discovery_watch_states
                 SET health='unavailable',watcher_active=0,last_error_code='watcher.unavailable',
                     next_reconcile_at_us=MIN(next_reconcile_at_us,?),version=version+1,
                     updated_at_us=? WHERE inbox_directory_id=?
                       AND (health!='unavailable' OR watcher_active!=0
                            OR COALESCE(last_error_code,'')!='watcher.unavailable')",
            )
            .bind(now_us)
            .bind(now_us)
            .bind(inbox_id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await
            .map_err(internal)?;
            let changed = sqlx::query_scalar::<_, i64>("SELECT changes()")
                .fetch_one(&mut *tx)
                .await
                .map_err(internal)?;
            if changed == 0 {
                tx.rollback().await.map_err(internal)?;
                continue;
            }
            OutboxWriter::write(
                &mut tx,
                INBOX_DISCOVERY_HEALTH_CHANGED,
                inbox_id,
                &InboxDiscoveryHealthChangedPayload {
                    inbox_directory_id: inbox_id,
                    health: InboxDiscoveryHealth::Unavailable,
                    watcher_active: false,
                    reason: Some(InboxDiscoveryHealthReason::WatcherUnavailable),
                },
                now_us,
            )
            .await?;
            tx.commit().await.map_err(internal)?;
            self.notifier.notify_after_commit();
        }
        Ok(())
    }
}

pub(crate) async fn schedule_reconcile_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    inbox_id: Uuid,
    reason: ScanReason,
    now_us: i64,
) -> Result<ReconcileSchedule, AppError> {
    ensure_default_policy(tx, inbox_id, now_us).await?;
    if let Some(bytes) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT t.id FROM tasks_scan_tasks t
         WHERE t.inbox_directory_id=? AND t.reason!='manual'
           AND t.status IN ('queued','running')
         ORDER BY t.created_at_us,t.id LIMIT 1",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?
    {
        return parse_uuid(&bytes).map(ReconcileSchedule::Existing);
    }
    let account_id =
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM identity_accounts WHERE singleton_key=1")
            .fetch_optional(&mut **tx)
            .await
            .map_err(internal)?;
    let Some(account_id) = account_id else {
        return Ok(ReconcileSchedule::Unavailable);
    };
    let account_id = parse_uuid(&account_id)?;
    let inbox_version = sqlx::query_scalar::<_, i64>(
        "SELECT version FROM discovery_inbox_directories WHERE id=? AND health='available'",
    )
    .bind(inbox_id.as_bytes().as_slice())
    .fetch_optional(&mut **tx)
    .await
    .map_err(internal)?;
    let Some(inbox_version) = inbox_version else {
        return Ok(ReconcileSchedule::Unavailable);
    };
    let task_id = new_id();
    insert_reconcile_task(
        tx,
        ReconcileTaskInsert {
            task_id,
            batch_id: new_id(),
            attempt_id: new_id(),
            account_id,
            inbox_id,
            inbox_version,
            reason,
            now_us,
        },
    )
    .await?;
    sqlx::query(
        "UPDATE discovery_watch_states
         SET active_reconcile_task_id=?,requested_reason=?,last_reconcile_started_at_us=?,
             version=version+1,updated_at_us=? WHERE inbox_directory_id=?",
    )
    .bind(task_id.as_bytes().as_slice())
    .bind(reason.as_str())
    .bind(now_us)
    .bind(now_us)
    .bind(inbox_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    OutboxWriter::write(
        tx,
        "task.state-changed",
        task_id,
        &serde_json::json!({"status":"queued","recovering":false}),
        now_us,
    )
    .await?;
    Ok(ReconcileSchedule::Created(task_id))
}

struct ReconcileTaskInsert {
    task_id: Uuid,
    batch_id: Uuid,
    attempt_id: Uuid,
    account_id: Uuid,
    inbox_id: Uuid,
    inbox_version: i64,
    reason: ScanReason,
    now_us: i64,
}

async fn insert_reconcile_task(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    value: ReconcileTaskInsert,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO discovery_scan_batches
         (id,inbox_directory_id,inbox_version,created_at_us,updated_at_us) VALUES (?,?,?,?,?)",
    )
    .bind(value.batch_id.as_bytes().as_slice())
    .bind(value.inbox_id.as_bytes().as_slice())
    .bind(value.inbox_version)
    .bind(value.now_us)
    .bind(value.now_us)
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "INSERT INTO tasks_scan_tasks
         (id,account_id,scan_batch_id,inbox_directory_id,status,stage,recovering,
          current_attempt_id,created_at_us,updated_at_us,reason)
         VALUES (?,?,?,?,'queued','queued',0,?,?,?,?)",
    )
    .bind(value.task_id.as_bytes().as_slice())
    .bind(value.account_id.as_bytes().as_slice())
    .bind(value.batch_id.as_bytes().as_slice())
    .bind(value.inbox_id.as_bytes().as_slice())
    .bind(value.attempt_id.as_bytes().as_slice())
    .bind(value.now_us)
    .bind(value.now_us)
    .bind(value.reason.as_str())
    .execute(&mut **tx)
    .await
    .map_err(internal)?;
    sqlx::query(
        "INSERT INTO tasks_scan_attempts
         (id,task_id,reason,ordinal,status,created_at_us) VALUES (?,?,'initial',1,'queued',?)",
    )
    .bind(value.attempt_id.as_bytes().as_slice())
    .bind(value.task_id.as_bytes().as_slice())
    .bind(value.now_us)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(internal)
}

fn completion_projection(
    status: &str,
    root_failed: bool,
    now_us: i64,
    reconcile_interval_seconds: i64,
) -> (
    InboxDiscoveryHealth,
    Option<InboxDiscoveryHealthReason>,
    i64,
) {
    if status == "completed" {
        (
            InboxDiscoveryHealth::Healthy,
            None,
            now_us.saturating_add(reconcile_interval_seconds.saturating_mul(1_000_000)),
        )
    } else if root_failed {
        (
            InboxDiscoveryHealth::Unavailable,
            Some(InboxDiscoveryHealthReason::WatcherUnavailable),
            now_us.saturating_add(RECONCILE_RETRY_US),
        )
    } else {
        (
            InboxDiscoveryHealth::Degraded,
            Some(InboxDiscoveryHealthReason::ReconcileFailed),
            now_us.saturating_add(RECONCILE_RETRY_US),
        )
    }
}

const fn health_value(health: InboxDiscoveryHealth) -> &'static str {
    match health {
        InboxDiscoveryHealth::Healthy => "healthy",
        InboxDiscoveryHealth::Degraded => "degraded",
        InboxDiscoveryHealth::Unavailable => "unavailable",
    }
}

const fn reason_value(reason: InboxDiscoveryHealthReason) -> &'static str {
    match reason {
        InboxDiscoveryHealthReason::WatcherUnavailable => "watcher.unavailable",
        InboxDiscoveryHealthReason::ReconcileFailed => "reconcile.failed",
    }
}

/// 排空 watcher 提示、安排启动/周期/恢复扫描并投影完成健康的持久协调器。
pub struct DiscoveryCoordinator {
    store: CoordinatorStore,
    receiver: WatchReceiver,
    coalescer: WatchCoalescer,
}

struct UnavailableFileWatch;

impl FileWatch for UnavailableFileWatch {
    fn replace(&mut self, targets: Vec<WatchedInbox>) -> Result<(), AppError> {
        if targets.is_empty() {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorCode::RootUnavailable,
                "operating system watcher is unavailable",
            ))
        }
    }
}

/// 生产 watcher 与持久协调器的 Core 生命周期包装。
pub struct DiscoveryRuntime {
    coordinator: DiscoveryCoordinator,
    store: CoordinatorStore,
    ingress: WatchIngress,
    watcher: Box<dyn FileWatch>,
    root_paths: BTreeMap<String, PathBuf>,
    config_path: PathBuf,
    fingerprint: DeploymentRootsFingerprint,
    configuration_current: bool,
}

impl DiscoveryRuntime {
    /// 从已验证配置构建生产协调器；OS watcher 初始化失败会降级而不会阻止周期对账。
    ///
    /// # Errors
    ///
    /// 部署根目录配置无效时返回 [`AppError`]。
    pub fn from_app(
        config: &AppConfig,
        db: &Db,
        notifier: OutboxNotifier,
    ) -> Result<Self, AppError> {
        let roots = DeploymentRootSet::load(&config.deployment_roots_file, config.mode)?;
        let root_paths = roots
            .declarations()
            .iter()
            .map(|root| (root.id.as_str().to_owned(), root.container_path.clone()))
            .collect();
        let fingerprint = roots.fingerprint();
        let (ingress, receiver) = WatchIngress::bounded(WATCH_CHANNEL_CAPACITY);
        let watcher: Box<dyn FileWatch> = if let Ok(watcher) = NotifyFileWatch::new(ingress.clone())
        {
            Box::new(watcher)
        } else {
            ingress.mark_overflow();
            Box::new(UnavailableFileWatch)
        };
        let store = CoordinatorStore::new(db.pool().clone(), notifier);
        Ok(Self {
            coordinator: DiscoveryCoordinator::new(store.clone(), receiver),
            store,
            ingress,
            watcher,
            root_paths,
            config_path: config.deployment_roots_file.clone(),
            fingerprint,
            configuration_current: true,
        })
    }

    /// 在后台循环前挂载 watcher 并立即持久化启动对账任务。
    ///
    /// watcher 不可用只投影降级；数据库失败仍返回 [`AppError`]。
    ///
    /// # Errors
    ///
    /// 无法查询/更新 watcher 投影或创建启动对账任务时返回 [`AppError`]。
    pub async fn prepare(&mut self) -> Result<Vec<Uuid>, AppError> {
        let now_us = chrono::Utc::now().timestamp_micros();
        self.refresh_watches(now_us).await?;
        self.coordinator.startup(now_us).await
    }

    /// 使用显式时间推进一次 watcher 刷新与协调周期，供嵌入和确定性测试使用。
    ///
    /// # Errors
    ///
    /// 配置健康投影、数据库查询/事务或任务调度失败时返回 [`AppError`]。
    pub async fn tick_once(&mut self, now_us: i64) -> Result<Vec<Uuid>, AppError> {
        self.refresh_watches(now_us).await?;
        self.coordinator.tick(now_us).await
    }

    /// 启动 250 ms 协调循环并返回显式关闭用句柄。
    #[must_use]
    pub fn start(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
            loop {
                interval.tick().await;
                let now_us = chrono::Utc::now().timestamp_micros();
                if let Err(error) = self.tick_once(now_us).await {
                    eprintln!("{}", error.code().as_str());
                }
            }
        })
    }

    async fn refresh_watches(&mut self, now_us: i64) -> Result<(), AppError> {
        self.store.ensure_inbox_projections(now_us).await?;
        if !self.fingerprint.matches_path(&self.config_path) {
            if self.configuration_current {
                let _ = self.watcher.replace(Vec::new());
                self.configuration_current = false;
                self.ingress.mark_overflow();
                self.store.watcher_unavailable(now_us).await?;
            }
            return Ok(());
        }
        let targets = self.store.watch_targets(&self.root_paths).await?;
        if self.watcher.replace(targets.clone()).is_err() {
            self.ingress.mark_overflow();
            self.store.watcher_unavailable(now_us).await?;
        } else {
            self.store.set_watcher_targets(&targets, now_us).await?;
        }
        Ok(())
    }
}

impl DiscoveryCoordinator {
    #[must_use]
    /// 创建协调器；调用方保留对应 [`crate::discovery::watcher::WatchIngress`]。
    pub fn new(store: CoordinatorStore, receiver: WatchReceiver) -> Self {
        Self {
            store,
            receiver,
            coalescer: WatchCoalescer::default(),
        }
    }

    /// Core 启动时为每个可用收件箱立即确保一次完整对账。
    ///
    /// # Errors
    ///
    /// 数据库状态无法恢复/排队时返回 [`AppError`]。
    pub async fn startup(&mut self, now_us: i64) -> Result<Vec<Uuid>, AppError> {
        self.store.ensure_inbox_projections(now_us).await?;
        self.store.refresh_completed(now_us).await?;
        self.schedule_many(
            self.store.all_inboxes(false).await?,
            ScanReason::Startup,
            now_us,
        )
        .await
    }

    /// 推进一次协调周期：完成投影、排空提示/overflow、释放到期合并并安排周期对账。
    ///
    /// # Errors
    ///
    /// 任一持久查询、任务创建、健康事件或事务失败时返回 [`AppError`]。
    pub async fn tick(&mut self, now_us: i64) -> Result<Vec<Uuid>, AppError> {
        self.store.ensure_inbox_projections(now_us).await?;
        self.store.refresh_completed(now_us).await?;
        let mut coalescer_overflow = false;
        while let Ok(hint) = self.receiver.try_recv() {
            if self.store.watcher_enabled(hint.inbox_directory_id).await? {
                self.store
                    .record_event(hint.inbox_directory_id, hint.observed_at_us)
                    .await?;
                if !self.coalescer.push(hint) {
                    coalescer_overflow = true;
                }
            }
        }
        let mut scheduled = Vec::new();
        if self.receiver.take_overflow() || coalescer_overflow {
            self.coalescer.clear();
            scheduled.extend(
                self.schedule_many(
                    self.store.all_inboxes(true).await?,
                    ScanReason::WatchRecovery,
                    now_us,
                )
                .await?,
            );
        }
        let hint_inboxes = self
            .coalescer
            .drain_ready(now_us)
            .into_iter()
            .map(|hint| hint.inbox_directory_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        scheduled.extend(
            self.schedule_many(hint_inboxes, ScanReason::WatchRecovery, now_us)
                .await?,
        );
        scheduled.extend(
            self.schedule_many(
                self.store.due_inboxes(now_us).await?,
                ScanReason::Periodic,
                now_us,
            )
            .await?,
        );
        scheduled.sort_unstable();
        scheduled.dedup();
        Ok(scheduled)
    }

    async fn schedule_many(
        &self,
        inboxes: Vec<Uuid>,
        reason: ScanReason,
        now_us: i64,
    ) -> Result<Vec<Uuid>, AppError> {
        let mut tasks = Vec::new();
        for inbox_id in inboxes {
            if let Some(task_id) = self
                .store
                .schedule_reconcile(inbox_id, reason, now_us)
                .await?
            {
                tasks.push(task_id);
            }
        }
        Ok(tasks)
    }
}

fn parse_uuid(bytes: &[u8]) -> Result<Uuid, AppError> {
    Uuid::from_slice(bytes).map_err(|error| AppError::with_source(ErrorCode::Internal, error))
}

fn internal(error: impl std::error::Error) -> AppError {
    AppError::with_source(ErrorCode::Internal, error)
}
