#![allow(clippy::too_many_lines)]

mod common;

use std::time::Duration;

use axum::body::{Body, BodyDataStream, to_bytes};
use axum::http::{Request, StatusCode, header};
use futures_util::StreamExt as _;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::{OutboxNotifier, OutboxReader, OutboxRetention};
use mediaflow_core::platform::random;
use mediaflow_core::platform::sse::{
    SseFailureCode, SseObserver, SseOptions, router_with_observer,
};
use mediaflow_core::tasks::events::TaskEventEnvelope;
use mediaflow_core::tasks::model::NewScanTask;
use mediaflow_core::tasks::store::TaskStore;
use tower::ServiceExt as _;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:3000";

#[tokio::test]
async fn ids_after_42_replay_as_43_44_on_every_reconnect_and_attempts_are_tracked() {
    let harness = Harness::new(SseOptions::for_tests()).await;
    let task = Uuid::now_v7();
    for id in 41..=44 {
        insert_state_event(harness.db.pool(), id, task).await;
    }
    for attempt in 1..=2 {
        let response = harness
            .app
            .clone()
            .oneshot(harness.request(Some("42")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        let first = next_chunk(&mut stream).await.unwrap();
        let second = next_chunk(&mut stream).await.unwrap();
        assert!(first.contains("id: 43"), "{first}");
        assert!(second.contains("id: 44"), "{second}");
        assert!(first.contains("event: task.state-changed"));
        drop(stream);
        let attempts = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                let attempts: Vec<i64> = sqlx::query_scalar(
                    "SELECT delivery_attempts FROM platform_outbox_events WHERE id IN (43,44) ORDER BY id",
                )
                .fetch_all(harness.db.pool())
                .await
                .unwrap();
                if attempts == [attempt, attempt] {
                    break attempts;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("in-flight disconnect attempt");
        assert_eq!(attempts, [attempt, attempt]);
    }
}

#[tokio::test]
async fn production_replay_drains_three_full_pages_without_poll_interval_gaps() {
    let harness = Harness::new(SseOptions::production()).await;
    let task = Uuid::now_v7();
    for id in 1..=151 {
        insert_state_event(harness.db.pool(), id, task).await;
    }
    let response = harness
        .app
        .clone()
        .oneshot(harness.request(None))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let ids = tokio::time::timeout(Duration::from_millis(950), async {
        let mut ids = Vec::new();
        for _ in 0..151 {
            let chunk = stream.next().await.unwrap().unwrap();
            let chunk = String::from_utf8(chunk.to_vec()).unwrap();
            let id = chunk
                .lines()
                .find_map(|line| line.strip_prefix("id: "))
                .unwrap()
                .parse::<i64>()
                .unwrap();
            ids.push(id);
        }
        ids
    })
    .await
    .expect("full replay pages must not wait for the one-second production poll");
    assert_eq!(ids, (1..=151).collect::<Vec<_>>());
}

#[tokio::test]
async fn post_commit_hint_wakes_long_poll_and_lost_hint_falls_back_to_database_polling() {
    let long = SseOptions {
        heartbeat: Duration::from_mins(1),
        poll_interval: Duration::from_mins(1),
        session_revalidate: Duration::from_mins(1),
        page_size: 50,
    };
    let harness = Harness::new(long).await;
    let response = harness
        .app
        .clone()
        .oneshot(harness.request(None))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let pending = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(300), stream.next()).await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    insert_state_event(harness.db.pool(), 1, Uuid::now_v7()).await;
    harness.notifier.notify_after_commit();
    let chunk = pending
        .await
        .unwrap()
        .expect("hint wake-up")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(chunk.to_vec()).unwrap().contains("id: 1"));

    let polling = SseOptions {
        heartbeat: Duration::from_mins(1),
        poll_interval: Duration::from_millis(20),
        session_revalidate: Duration::from_mins(1),
        page_size: 50,
    };
    let fallback = Harness::new(polling).await;
    let response = fallback
        .app
        .clone()
        .oneshot(fallback.request(None))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let pending = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_millis(300), stream.next()).await
    });
    tokio::time::sleep(Duration::from_millis(25)).await;
    insert_state_event(fallback.db.pool(), 1, Uuid::now_v7()).await;
    let chunk = pending
        .await
        .unwrap()
        .expect("poll fallback")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8(chunk.to_vec()).unwrap().contains("id: 1"));
}

#[tokio::test]
async fn client_disconnect_is_observable_and_cannot_change_persisted_events() {
    let harness = Harness::new(SseOptions::for_tests()).await;
    insert_state_event(harness.db.pool(), 1, Uuid::now_v7()).await;
    let payload_before: String =
        sqlx::query_scalar("SELECT payload_json FROM platform_outbox_events WHERE id=1")
            .fetch_one(harness.db.pool())
            .await
            .unwrap();
    let response = harness
        .app
        .clone()
        .oneshot(harness.request(None))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert!(next_chunk(&mut stream).await.unwrap().contains("id: 1"));
    let before_drop: (i64, Option<String>) = sqlx::query_as(
        "SELECT delivery_attempts,last_delivery_error FROM platform_outbox_events WHERE id=1",
    )
    .fetch_one(harness.db.pool())
    .await
    .unwrap();
    assert_eq!(
        before_drop,
        (0, None),
        "yielding a frame must leave it in-flight until the next body poll"
    );
    drop(stream);
    let failure = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let row: (i64, Option<String>) = sqlx::query_as(
                "SELECT delivery_attempts,last_delivery_error FROM platform_outbox_events WHERE id=1",
            )
            .fetch_one(harness.db.pool())
            .await
            .unwrap();
            if row.1.is_some() {
                break row;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnect delivery state");
    assert_eq!(failure, (1, Some("client.disconnected".to_owned())));
    assert_eq!(
        harness.observer.subscribe().borrow().clone().unwrap().code,
        SseFailureCode::ClientDisconnected
    );
    let payload_after: String =
        sqlx::query_scalar("SELECT payload_json FROM platform_outbox_events WHERE id=1")
            .fetch_one(harness.db.pool())
            .await
            .unwrap();
    assert_eq!(payload_after, payload_before);
}

#[tokio::test]
async fn stale_cursor_gets_exactly_one_gap_then_eof_and_no_cursor_starts_at_minimum() {
    let harness = Harness::new(SseOptions::for_tests()).await;
    insert_state_event(harness.db.pool(), 100, Uuid::now_v7()).await;
    let response = harness
        .app
        .clone()
        .oneshot(harness.request(Some("42")))
        .await
        .unwrap();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert_eq!(body.matches("event: stream.gap").count(), 1, "{body}");
    assert!(body.contains("id: 99"));
    assert!(body.contains("\"minimum_available_id\":100"));
    assert!(!body.contains("/config"));

    let response = harness
        .app
        .clone()
        .oneshot(harness.request(None))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let first = next_chunk(&mut stream).await.unwrap();
    assert!(first.contains("id: 100"));
    assert!(!first.contains("stream.gap"));
}

#[tokio::test]
async fn cleanup_and_read_race_never_returns_a_silent_hole_and_reopen_uses_sqlite() {
    for _ in 0..20 {
        let harness = Harness::new(SseOptions::for_tests()).await;
        for id in 100..=102 {
            insert_state_event(harness.db.pool(), id, Uuid::now_v7()).await;
        }
        let reader = OutboxReader::new(harness.db.pool().clone());
        let deleting = sqlx::query("DELETE FROM platform_outbox_events WHERE id=100")
            .execute(harness.db.pool());
        let replaying = reader.replay(Some(99), 50, 1);
        let (deleted, replay) = tokio::join!(deleting, replaying);
        deleted.unwrap();
        let replay = replay.unwrap();
        assert!(
            replay.gap.is_some() || replay.events.first().is_some_and(|event| event.id() == 100)
        );
    }

    let harness = Harness::new(SseOptions::for_tests()).await;
    insert_state_event(harness.db.pool(), 41, Uuid::now_v7()).await;
    drop(harness.notifier);
    let reopened = OutboxReader::new(harness.db.pool().clone());
    assert_eq!(reopened.after(0, 50).await.unwrap()[0].id(), 41);
}

#[tokio::test]
async fn stream_delivery_reader_and_maintenance_failures_do_not_change_rest_truth() {
    let harness = Harness::new(SseOptions::for_tests()).await;
    let (account, inbox) = seed_inbox(harness.db.pool()).await;
    let store = TaskStore::new_with_notifier(harness.db.pool().clone(), harness.notifier.clone());
    let task = store
        .create(NewScanTask {
            account_id: account,
            inbox_directory_id: inbox,
            idempotency_key: "sse-failure-truth".to_owned(),
            now_us: 1,
        })
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_delivery BEFORE UPDATE OF delivery_attempts ON platform_outbox_events
         BEGIN SELECT RAISE(FAIL,'private delivery detail /config/secret'); END",
    )
    .execute(harness.db.pool())
    .await
    .unwrap();
    let response = harness
        .app
        .clone()
        .oneshot(harness.request(None))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert!(
        next_chunk(&mut stream)
            .await
            .unwrap()
            .contains(&task.id.to_string())
    );
    assert!(
        next_chunk(&mut stream).await.is_none(),
        "a delivery-state failure closes only the stream"
    );
    assert_eq!(store.get(account, task.id).await.unwrap(), task);
    assert_eq!(
        harness.observer.subscribe().borrow().clone().unwrap().code,
        SseFailureCode::DeliveryAttemptFailed
    );
    sqlx::query("DROP TRIGGER fail_delivery")
        .execute(harness.db.pool())
        .await
        .unwrap();
    assert_eq!(
        OutboxReader::new(harness.db.pool().clone())
            .after(0, 50)
            .await
            .unwrap()[0]
            .id(),
        1
    );

    sqlx::query(
        "INSERT INTO platform_outbox_events
         (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
         VALUES ('unknown.private','1',?,json_object('sql','SELECT password_phc','path','/config/private'),2)",
    )
    .bind(task.id.as_bytes().as_slice())
    .execute(harness.db.pool())
    .await
    .unwrap();
    let response = harness
        .app
        .clone()
        .oneshot(harness.request(Some("1")))
        .await
        .unwrap();
    let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
    assert!(
        body.is_empty(),
        "reader failure closes without diagnostic payload"
    );
    assert_eq!(
        harness.observer.subscribe().borrow().clone().unwrap().code,
        SseFailureCode::ReaderFailed
    );
    assert_eq!(store.get(account, task.id).await.unwrap(), task);

    sqlx::query("DELETE FROM platform_outbox_events WHERE event_type='unknown.private'")
        .execute(harness.db.pool())
        .await
        .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_cleanup BEFORE DELETE ON platform_outbox_events
         BEGIN SELECT RAISE(FAIL,'maintenance private path /config/private'); END",
    )
    .execute(harness.db.pool())
    .await
    .unwrap();
    // 保留条件会有意阻止在此小型夹具中删除；
    // 注入足够大的直接旧前缀，以便执行维护触发器。
    seed_successor_rows(harness.db.pool(), task.id).await;
    assert!(
        OutboxRetention::new(harness.db.pool().clone())
            .cleanup(30 * 60 * 60 * 1_000_000)
            .await
            .is_err()
    );
    assert_eq!(store.get(account, task.id).await.unwrap(), task);
}

struct Harness {
    _fixture: common::TestConfigDir,
    db: mediaflow_core::platform::db::Db,
    app: axum::Router,
    cookie: String,
    notifier: OutboxNotifier,
    observer: SseObserver,
}

impl Harness {
    async fn new(options: SseOptions) -> Self {
        let fixture = common::TestConfigDir::new(RunMode::Development);
        let db = migrate_with_backup(fixture.config()).await.unwrap();
        let account = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO identity_accounts
             (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
             VALUES (1,?,'admin','Admin','not-used',0,0)",
        )
        .bind(account.as_bytes().as_slice())
        .execute(db.pool())
        .await
        .unwrap();
        let cookie = seed_session(db.pool(), account).await;
        let notifier = OutboxNotifier::new();
        let observer = SseObserver::new();
        let app = router_with_observer(
            fixture.config().clone(),
            &db,
            notifier.clone(),
            options,
            observer.clone(),
        );
        Self {
            _fixture: fixture,
            db,
            app,
            cookie,
            notifier,
            observer,
        }
    }

    fn request(&self, cursor: Option<&str>) -> Request<Body> {
        let mut builder = Request::get("/api/v1/events")
            .header(header::COOKIE, &self.cookie)
            .header(header::ORIGIN, ORIGIN)
            .header("sec-fetch-site", "same-origin");
        if let Some(cursor) = cursor {
            builder = builder.header("last-event-id", cursor);
        }
        builder.body(Body::empty()).unwrap()
    }
}

async fn seed_session(pool: &sqlx::SqlitePool, account: Uuid) -> String {
    let token = "recovery-session-token";
    let now = chrono::Utc::now().timestamp_micros();
    sqlx::query(
        "INSERT INTO identity_sessions
         (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,
          credential_version,last_used_at_us,created_at_us)
         VALUES (?,?,?,?,?,?,?,?,?)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account.as_bytes().as_slice())
    .bind(random::sha256(token.as_bytes()).as_slice())
    .bind([3_u8; 32].as_slice())
    .bind(now + 60 * 60 * 1_000_000)
    .bind(now + 60 * 60 * 1_000_000)
    .bind(1_i64)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .unwrap();
    format!("__Host-mediaflow_session={token}")
}

async fn insert_state_event(pool: &sqlx::SqlitePool, id: i64, task: Uuid) {
    sqlx::query(
        "INSERT INTO platform_outbox_events
         (id,event_type,schema_version,aggregate_id,payload_json,committed_at_us)
         VALUES (?,'task.state-changed','1',?,json_object('status','running','recovering',json('false')),?)",
    )
    .bind(id)
    .bind(task.as_bytes().as_slice())
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_inbox(pool: &sqlx::SqlitePool) -> (Uuid, Uuid) {
    let account_bytes: Vec<u8> = sqlx::query_scalar("SELECT id FROM identity_accounts")
        .fetch_one(pool)
        .await
        .unwrap();
    let account = Uuid::from_slice(&account_bytes).unwrap();
    let inbox = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'726F6F74',X'646972','available',0,0,0)",
    )
    .bind(inbox.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
    (account, inbox)
}

async fn seed_successor_rows(pool: &sqlx::SqlitePool, task: Uuid) {
    sqlx::query(
        "WITH RECURSIVE seq(id) AS (
             VALUES(2) UNION ALL SELECT id+1 FROM seq WHERE id<=100001
         )
         INSERT OR IGNORE INTO platform_outbox_events
           (id,event_type,schema_version,aggregate_id,payload_json,committed_at_us)
         SELECT id,'task.state-changed','1',?,
                json_object('status','running','recovering',json('false')),1 FROM seq",
    )
    .bind(task.as_bytes().as_slice())
    .execute(pool)
    .await
    .unwrap();
}

async fn next_chunk(stream: &mut BodyDataStream) -> Option<String> {
    let result = tokio::time::timeout(Duration::from_millis(500), stream.next())
        .await
        .expect("stream made progress")?;
    Some(String::from_utf8(result.expect("body chunk").to_vec()).unwrap())
}

#[allow(dead_code)]
fn _ids(events: &[TaskEventEnvelope]) -> Vec<i64> {
    events.iter().map(TaskEventEnvelope::id).collect()
}
