use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::stream::{self, Stream};
use tokio::sync::watch;

use crate::bootstrap::config::AppConfig;
use crate::discovery::capability::{DeploymentRootSet, DeploymentRootsFingerprint};
use crate::identity::service::IdentityService;
use crate::platform::db::Db;
use crate::platform::outbox::{MAX_PUBLIC_EVENT_ID, OutboxNotifier, OutboxReader};
use crate::platform::request_security::{SessionGuard, session_token, validate_same_origin};
use crate::shared::error::{AppError, ErrorCode};
use crate::tasks::events::TaskEventEnvelope;

const DEFAULT_PAGE_SIZE: u32 = 50;

#[derive(Clone, Copy, Debug)]
/// 单个 SSE 路由实例的时序和数据库分页边界。
///
/// 构建路由器要求每个时长均非零，且 `page_size` 位于 `1..=200`。
pub struct SseOptions {
    /// 发送注释心跳前的最大空闲时间。
    pub heartbeat: Duration,
    /// 通知器代际未变化时，两次持久 outbox 读取之间的回退延迟。
    pub poll_interval: Duration,
    /// 两次数据库重新验证会话令牌之间的间隔。
    pub session_revalidate: Duration,
    /// 每次重放读取加载的最大 outbox 事件数。
    pub page_size: u32,
}

impl SseOptions {
    #[must_use]
    /// 返回生产默认值：15 s 心跳、1 s 轮询、5 s 会话检查、每页 50 个事件。
    pub const fn production() -> Self {
        Self {
            heartbeat: Duration::from_secs(15),
            poll_interval: Duration::from_secs(1),
            session_revalidate: Duration::from_secs(5),
            page_size: DEFAULT_PAGE_SIZE,
        }
    }

    #[must_use]
    /// 使用生产分页大小返回加速的确定性测试时序。
    pub const fn for_tests() -> Self {
        Self {
            heartbeat: Duration::from_millis(25),
            poll_interval: Duration::from_millis(20),
            session_revalidate: Duration::from_millis(10),
            page_size: DEFAULT_PAGE_SIZE,
        }
    }

    fn validate(self) -> Result<Self, AppError> {
        if self.heartbeat.is_zero()
            || self.poll_interval.is_zero()
            || self.session_revalidate.is_zero()
            || !(1..=200).contains(&self.page_size)
        {
            return Err(AppError::new(
                ErrorCode::ConfigInvalid,
                "SSE timing or page options are invalid",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SSE 流失败/终止的稳定可观测分类。
pub enum SseFailureCode {
    /// 持久 outbox 重放/查询失败。
    ReaderFailed,
    /// 更新逐事件投递记账失败。
    DeliveryAttemptFailed,
    /// 会话或就绪状态重新验证关闭了流。
    SessionClosed,
    /// 客户端丢弃了响应体。
    ClientDisconnected,
}

impl SseFailureCode {
    #[must_use]
    /// 返回与此失败类别关联的日志/watch 代码。
    pub const fn stable_code(self) -> &'static str {
        match self {
            Self::ReaderFailed => "sse.reader_failed",
            Self::DeliveryAttemptFailed => "sse.delivery_attempt_failed",
            Self::SessionClosed => "sse.session_closed",
            Self::ClientDisconnected => "sse.client_disconnected",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 发布给观察者的最近 SSE 失败。
pub struct SseFailure {
    /// 稳定失败分类。
    pub code: SseFailureCode,
    /// 观察者收到报告时的 UTC 微秒。
    pub occurred_at_us: i64,
}

#[derive(Clone)]
/// 用于最新 SSE 失败的可克隆 watch 通道。
///
/// [`Self::new`] 对测试/嵌入保持静默；生产观察者还会向标准错误输出稳定代码。
pub struct SseObserver {
    failures: watch::Sender<Option<SseFailure>>,
    log_failures: bool,
}

impl Default for SseObserver {
    fn default() -> Self {
        Self::new()
    }
}

impl SseObserver {
    #[must_use]
    /// 创建初始失败为 `None` 的静默观察者。
    pub fn new() -> Self {
        let (failures, _) = watch::channel(None);
        Self {
            failures,
            log_failures: false,
        }
    }

    fn production() -> Self {
        let (failures, _) = watch::channel(None);
        Self {
            failures,
            log_failures: true,
        }
    }

    #[must_use]
    /// 订阅最新失败值。
    pub fn subscribe(&self) -> watch::Receiver<Option<SseFailure>> {
        self.failures.subscribe()
    }

    fn report(&self, code: SseFailureCode) {
        if self.log_failures {
            eprintln!("{}", code.stable_code());
        }
        self.failures.send_replace(Some(SseFailure {
            code,
            occurred_at_us: chrono::Utc::now().timestamp_micros(),
        }));
    }
}

#[derive(Clone)]
struct SseState {
    config: AppConfig,
    db: Db,
    identity: IdentityService,
    reader: OutboxReader,
    notifier: OutboxNotifier,
    options: SseOptions,
    observer: SseObserver,
    deployment_roots_fingerprint: Option<DeploymentRootsFingerprint>,
}

impl SseState {
    async fn ensure_ready(&self) -> Result<(), AppError> {
        if !self.config.readiness_issues().is_empty() {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            ));
        }
        if self
            .deployment_roots_fingerprint
            .as_ref()
            .is_none_or(|fingerprint| !fingerprint.matches_path(&self.config.deployment_roots_file))
        {
            return Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            ));
        }
        match tokio::time::timeout(
            Duration::from_millis(250),
            sqlx::query_scalar::<_, i64>("SELECT 1").fetch_one(self.db.pool()),
        )
        .await
        {
            Ok(Ok(1)) => Ok(()),
            _ => Err(AppError::new(
                ErrorCode::NotReady,
                "business routes are closed",
            )),
        }
    }
}

/// 使用本地通知器构建已认证的任务事件 SSE 路由。
///
/// # Panics
///
/// 任一时序为零或 `page_size` 不在 `1..=200` 时发生 panic。
pub fn router(config: AppConfig, db: &Db, options: SseOptions) -> Router {
    router_with_notifier(config, db, OutboxNotifier::new(), options)
}

/// 构建与事务性任务写入器共享 `notifier` 的任务事件 SSE 路由。
///
/// 共享可避免提交后等待轮询间隔；持久重放仍然是权威来源。
///
/// # Panics
///
/// 任一时序为零或 `page_size` 不在 `1..=200` 时发生 panic。
pub fn router_with_notifier(
    config: AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
    options: SseOptions,
) -> Router {
    router_with_observer(config, db, notifier, options, SseObserver::production())
}

/// 构建具有可注入观察者的事件路由，用于确定性运行时测试。
///
/// # Panics
///
/// 调用方提供零时序值或超范围数据库分页大小时发生 panic。
pub fn router_with_observer(
    config: AppConfig,
    db: &Db,
    notifier: OutboxNotifier,
    options: SseOptions,
    observer: SseObserver,
) -> Router {
    let options = options.validate().expect("validated SSE options");
    let deployment_roots_fingerprint =
        DeploymentRootSet::load(&config.deployment_roots_file, config.mode)
            .ok()
            .map(|roots| roots.fingerprint());
    Router::new()
        .route("/api/v1/events", get(events))
        .with_state(Arc::new(SseState {
            identity: IdentityService::new(db.pool().clone(), config.config_dir.clone()),
            reader: OutboxReader::new(db.pool().clone()),
            config,
            db: db.clone(),
            notifier,
            options,
            observer,
            deployment_roots_fingerprint,
        }))
}

async fn events(
    State(state): State<Arc<SseState>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    state.ensure_ready().await?;
    SessionGuard::authenticate_without_sliding(&headers, &state.identity).await?;
    if let Err(error) = validate_same_origin(&headers, &state.config.public_origin) {
        state.identity.audit_denial("origin").await?;
        return Err(error);
    }
    let cursor = parse_last_event_id(&headers)?;
    let raw_token = session_token(&headers)
        .ok_or_else(|| AppError::new(ErrorCode::SessionExpired, "session cookie missing"))?
        .to_owned();

    // 在首次数据库检查前订阅。广播仅是唤醒提示；重放和实时投递始终从 SQLite 读取已提交的行。
    let receiver = state.notifier.subscribe();
    let stream = event_stream(StreamState::new(
        state.reader.clone(),
        receiver,
        state.identity.clone(),
        raw_token,
        cursor,
        state.options,
        state.observer.clone(),
    ));
    let mut response = Sse::new(stream).into_response();
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, no-transform"),
    );
    response_headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    Ok(response)
}

fn parse_last_event_id(headers: &HeaderMap) -> Result<Option<i64>, AppError> {
    let values = headers.get_all("last-event-id");
    let mut iter = values.iter();
    let Some(value) = iter.next() else {
        return Ok(None);
    };
    if iter.next().is_some() {
        return Err(invalid_cursor());
    }
    let value = value.to_str().map_err(|_| invalid_cursor())?;
    if value.is_empty() || value.bytes().any(|byte| !byte.is_ascii_digit()) {
        return Err(invalid_cursor());
    }
    let parsed = value.parse::<i64>().map_err(|_| invalid_cursor())?;
    if !(1..=MAX_PUBLIC_EVENT_ID).contains(&parsed) {
        return Err(invalid_cursor());
    }
    Ok(Some(parsed))
}

fn invalid_cursor() -> AppError {
    AppError::new(
        ErrorCode::ValidationFailed,
        "Last-Event-ID must be an integer from 1 through 9007199254740991",
    )
}

struct StreamState {
    reader: OutboxReader,
    receiver: watch::Receiver<u64>,
    identity: IdentityService,
    raw_token: String,
    cursor: Option<i64>,
    options: SseOptions,
    observer: SseObserver,
    pending: VecDeque<TaskEventEnvelope>,
    in_flight: Option<i64>,
    fetch: bool,
    close_after_pending: bool,
    last_output: tokio::time::Instant,
    last_revalidation: tokio::time::Instant,
    terminal: bool,
}

impl StreamState {
    fn new(
        reader: OutboxReader,
        receiver: watch::Receiver<u64>,
        identity: IdentityService,
        raw_token: String,
        cursor: Option<i64>,
        options: SseOptions,
        observer: SseObserver,
    ) -> Self {
        let now = tokio::time::Instant::now();
        Self {
            reader,
            receiver,
            identity,
            raw_token,
            cursor,
            options,
            observer,
            pending: VecDeque::new(),
            in_flight: None,
            fetch: true,
            close_after_pending: false,
            last_output: now,
            last_revalidation: now,
            terminal: false,
        }
    }

    fn mark_terminal(&mut self) {
        self.terminal = true;
    }

    async fn revalidate_session(&mut self) -> bool {
        if self
            .identity
            .authenticate_without_sliding(&self.raw_token)
            .await
            .is_err()
        {
            self.observer.report(SseFailureCode::SessionClosed);
            self.mark_terminal();
            return false;
        }
        self.last_revalidation = tokio::time::Instant::now();
        true
    }

    async fn revalidate_session_if_due(&mut self) -> bool {
        if tokio::time::Instant::now() < self.last_revalidation + self.options.session_revalidate {
            return true;
        }
        self.revalidate_session().await
    }
}

impl Drop for StreamState {
    fn drop(&mut self) {
        if !self.terminal {
            self.observer.report(SseFailureCode::ClientDisconnected);
            if let Some(id) = self.in_flight.take() {
                let reader = self.reader.clone();
                let observer = self.observer.clone();
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        if reader
                            .record_delivery_attempt(
                                id,
                                Some(crate::platform::outbox::DeliveryFailureCode::ClientDisconnected),
                            )
                            .await
                            .is_err()
                        {
                            observer.report(SseFailureCode::DeliveryAttemptFailed);
                        }
                    });
                } else {
                    self.observer.report(SseFailureCode::DeliveryAttemptFailed);
                }
            }
        }
    }
}

fn event_stream(
    state: StreamState,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    stream::unfold(state, |mut state| async move {
        loop {
            // Axum 已请求下一帧，因此先前 yield 的帧已越过响应体流接收边界，现在可标记为已发送。
            if let Some(id) = state.in_flight.take()
                && state
                    .reader
                    .record_delivery_attempt(id, None)
                    .await
                    .is_err()
            {
                state.observer.report(SseFailureCode::DeliveryAttemptFailed);
                state.mark_terminal();
                return None;
            }
            // 待处理分页和繁忙的实时流绝不能使非滑动认证截止时间得不到处理。
            if !state.revalidate_session_if_due().await {
                return None;
            }
            if let Some(envelope) = state.pending.pop_front() {
                state.cursor = Some(envelope.id());
                state.last_output = tokio::time::Instant::now();
                let event = public_event(&envelope);
                if envelope.event_type() != "stream.gap" {
                    state.in_flight = Some(envelope.id());
                }
                return Some((Ok(event), state));
            }
            if state.close_after_pending {
                state.mark_terminal();
                return None;
            }
            if state.fetch {
                // 完整分页会立即接续另一个数据库分页。在短读取事务之间协作式 yield，不休眠。
                tokio::task::yield_now().await;
                if !state.revalidate_session_if_due().await {
                    return None;
                }
                state.fetch = false;
                if let Ok(batch) = state
                    .reader
                    .replay(
                        state.cursor,
                        state.options.page_size,
                        chrono::Utc::now().timestamp_micros(),
                    )
                    .await
                {
                    if let Some(gap) = batch.gap {
                        state.pending.push_back(gap);
                        state.close_after_pending = true;
                        continue;
                    }
                    if !batch.events.is_empty() {
                        state.fetch = batch.events.len() == state.options.page_size as usize;
                        state.pending.extend(batch.events);
                        continue;
                    }
                } else {
                    state.observer.report(SseFailureCode::ReaderFailed);
                    state.mark_terminal();
                    return None;
                }
            }

            let heartbeat_at = state.last_output + state.options.heartbeat;
            let revalidate_at = state.last_revalidation + state.options.session_revalidate;
            tokio::select! {
                biased;
                () = tokio::time::sleep_until(revalidate_at) => {
                    if !state.revalidate_session().await {
                        return None;
                    }
                }
                () = tokio::time::sleep_until(heartbeat_at) => {
                    if !state.revalidate_session().await {
                        return None;
                    }
                    state.last_output = tokio::time::Instant::now();
                    return Some((Ok(Event::default().comment("heartbeat")), state));
                }
                result = state.receiver.changed() => {
                    let _ = result;
                    state.fetch = true;
                }
                () = tokio::time::sleep(state.options.poll_interval) => {
                    state.fetch = true;
                }
            }
        }
    })
}

fn public_event(envelope: &TaskEventEnvelope) -> Event {
    let json = serde_json::to_string(envelope).expect("event envelope is serializable");
    Event::default()
        .id(envelope.id().to_string())
        .event(envelope.event_type())
        .data(json)
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 从一次持久重放读取中选定的即时动作。
pub enum ReplayDecision {
    /// 当前在游标之后没有可用的保留事件。
    Wait,
    /// 游标早于保留历史，客户端必须重新同步。
    Gap(TaskEventEnvelope),
    /// 一个或多个有序保留事件已可投递。
    Events(Vec<TaskEventEnvelope>),
}

impl ReplayDecision {
    /// 在 `cursor` 之后加载至多默认分页大小的数据，并分类为缺口/空/事件。
    ///
    /// 在读取前建立订阅以避免遗漏并发通知器变化；返回的决策本身不等待。
    ///
    /// # Errors
    ///
    /// 游标无效，或任一 outbox 事务、查询、解码或缺口时间戳失败时，返回 [`AppError`]。
    pub async fn load(
        reader: &OutboxReader,
        notifier: &OutboxNotifier,
        cursor: Option<i64>,
    ) -> Result<Self, AppError> {
        let _receiver = notifier.subscribe();
        let batch = reader
            .replay(
                cursor,
                DEFAULT_PAGE_SIZE,
                chrono::Utc::now().timestamp_micros(),
            )
            .await?;
        if let Some(gap) = batch.gap {
            Ok(Self::Gap(gap))
        } else if batch.events.is_empty() {
            Ok(Self::Wait)
        } else {
            Ok(Self::Events(batch.events))
        }
    }
}
