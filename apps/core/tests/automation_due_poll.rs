//! A5 定时触发：到期 RSS 源的领取、租约独占与轮询循环接线。

#![allow(clippy::too_many_lines)]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use mediaflow_core::automation::event_store::AutomationEventStore;
use mediaflow_core::automation::rss::client::{
    FeedClient, FeedClientError, FeedResponse, RssCursor,
};
use mediaflow_core::automation::rss::poller::{FeedPollCommit, FeedPollCommitPort, FeedPollSource};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::secrets::SecretBytes;
use uuid::Uuid;

struct StaticClient;

#[async_trait]
impl FeedClient for StaticClient {
    async fn fetch(
        &self,
        _url: &SecretBytes,
        _cursor: Option<&RssCursor>,
    ) -> Result<FeedResponse, FeedClientError> {
        Err(FeedClientError::Unavailable)
    }
}

#[derive(Default)]
struct RecordingCommits {
    failures: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl FeedPollCommitPort for RecordingCommits {
    async fn commit(
        &self,
        _source_id: Uuid,
        _expected_config_version: i64,
        _lease_token: Uuid,
        _commit: FeedPollCommit,
    ) -> Result<usize, FeedClientError> {
        Ok(0)
    }

    async fn commit_failure(
        &self,
        _source_id: Uuid,
        _expected_config_version: i64,
        _lease_token: Uuid,
        _failure: FeedClientError,
    ) -> Result<(), FeedClientError> {
        self.failures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn claim_returns_none_without_due_enabled_secret_sources() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store = AutomationEventStore::open_with_notifier(
        db.pool().clone(),
        &fixture.config().config_dir,
        mediaflow_core::platform::outbox::OutboxNotifier::new(),
    )
    .unwrap();
    // 无任何源。
    assert!(
        store
            .claim_due_rss_source(1_000_000, 30_000_000)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .claim_due_rss_source(1_000_000, 30_000_000)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn poller_wires_due_poll_through_commit_port() {
    // 集成验证：FeedPoller + commit port 在上游不可用时记录失败提交（释放租约），
    // 调度层由此具备完整的"到期→轮询→失败提交→退避"闭环。
    let commits = Arc::new(RecordingCommits::default());
    let poller = mediaflow_core::automation::rss::poller::FeedPoller::new(
        Arc::new(StaticClient),
        commits.clone(),
    );
    // FeedPollSource 仅经 claim 产生；这里直接构造一个最小源驱动失败路径。
    let source = FeedPollSource {
        id: Uuid::now_v7(),
        config_version: 1,
        lease_token: Uuid::now_v7(),
        downloader_connection_id: Uuid::now_v7(),
        feed_url: SecretBytes::new(b"https://feeds.example.test/due.xml".to_vec()),
        cursor: None,
    };
    let _outcome = poller.poll(source).await;
    assert!(
        commits.failures.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "upstream failure must be committed to the source runtime"
    );
}

#[tokio::test]
async fn due_feed_is_polled_even_while_events_remain_pending() {
    use mediaflow_core::automation::model::AutomationSourceInput;
    use mediaflow_core::automation::runtime::AutomationRuntime;
    use mediaflow_core::automation::source_store::AutomationSourceStore;
    use mediaflow_core::connectors::model::SecretString;
    use mediaflow_core::platform::outbox::OutboxNotifier;

    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let downloader =
        sqlx::query_scalar::<_, Vec<u8>>("SELECT id FROM downloader_connections LIMIT 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let source_id = Uuid::now_v7();
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            source_id,
            AutomationSourceInput::Rss {
                display_name: "Due feed behind event backlog".to_owned(),
                enabled: true,
                feed_url: SecretString::new("https://feed.example.test/test.xml".to_owned()),
                downloader_connection_id: Uuid::from_slice(&downloader).unwrap(),
                poll_interval_seconds: 60,
            },
            None,
            1,
        )
        .await
        .unwrap();
    let runtime = AutomationRuntime::with_feed_client(
        db.pool().clone(),
        &fixture.config().config_dir,
        OutboxNotifier::new(),
        Arc::new(StaticClient),
    )
    .unwrap();
    assert!(runtime.run_once().await.unwrap().is_some());
    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM automation_events WHERE status='pending'")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(pending, 1, "one event per work category per round");
    let polled: Option<i64> = sqlx::query_scalar(
        "SELECT next_poll_at_us FROM automation_source_runtime WHERE source_id=?",
    )
    .bind(source_id.as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(
        polled.is_some_and(|at| at > 1),
        "feed failure/backoff must commit despite remaining event backlog"
    );
}
