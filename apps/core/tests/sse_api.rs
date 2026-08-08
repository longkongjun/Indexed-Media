#![allow(clippy::too_many_lines)]

mod common;

use std::time::Duration;

use axum::body::{Body, BodyDataStream, to_bytes};
use axum::http::{HeaderValue, Request, StatusCode, header};
use futures_util::StreamExt as _;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::random;
use mediaflow_core::platform::sse::{
    SseFailureCode, SseObserver, SseOptions, router_with_observer,
};
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

const ORIGIN: &str = "http://127.0.0.1:3000";

#[tokio::test]
async fn production_http_composition_exposes_the_authenticated_event_stream() {
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
    let cookie = seed_session(db.pool(), account, "composed-token").await;
    let response = build_router(fixture.config().clone(), Some(db))
        .oneshot(event_request(&cookie, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "no-cache, no-store, no-transform"
    );
}

#[tokio::test]
async fn production_composition_persists_in_flight_client_disconnect() {
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
    let cookie = seed_session(db.pool(), account, "production-disconnect").await;
    insert_state_event(db.pool(), 1, Uuid::now_v7()).await;
    let response = build_router(fixture.config().clone(), Some(db.clone()))
        .oneshot(event_request(&cookie, None))
        .await
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    assert!(next_chunk(&mut stream).await.unwrap().contains("id: 1"));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT delivery_attempts FROM platform_outbox_events WHERE id=1"
        )
        .fetch_one(db.pool())
        .await
        .unwrap(),
        0,
        "success cannot be claimed before the delivery layer asks for another frame"
    );
    drop(stream);
    let code = tokio::time::timeout(Duration::from_millis(500), async {
        loop {
            let code: Option<String> = sqlx::query_scalar(
                "SELECT last_delivery_error FROM platform_outbox_events WHERE id=1",
            )
            .fetch_one(db.pool())
            .await
            .unwrap();
            if code.is_some() {
                break code;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("production disconnect state");
    assert_eq!(code.as_deref(), Some("client.disconnected"));
    assert_eq!(
        [
            SseFailureCode::ReaderFailed.stable_code(),
            SseFailureCode::DeliveryAttemptFailed.stable_code(),
            SseFailureCode::SessionClosed.stable_code(),
            SseFailureCode::ClientDisconnected.stable_code(),
        ],
        [
            "sse.reader_failed",
            "sse.delivery_attempt_failed",
            "sse.session_closed",
            "sse.client_disconnected",
        ]
    );
}

#[tokio::test]
async fn deployment_root_snapshot_change_closes_sse_at_the_readiness_guard() {
    let (fixture, _db, app, cookie, _observer) = authenticated_sse(SseOptions::for_tests()).await;
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&serde_json::json!({"roots":[{
            "id":"incoming",
            "label":"Incoming",
            "container_path":fixture.config().config_dir,
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let response = app
        .oneshot(event_request(&cookie, Some("not-a-cursor")))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(response).await["error"]["code"], "internal.error");
}

#[tokio::test]
async fn guard_order_is_readiness_session_origin_then_last_event_id() {
    let (_fixture, db, app, cookie, _observer) = authenticated_sse(SseOptions::for_tests()).await;
    let mut unauthenticated = Request::get("/api/v1/events").body(Body::empty()).unwrap();
    unauthenticated
        .headers_mut()
        .insert("last-event-id", HeaderValue::from_bytes(b"\xff").unwrap());
    let response = app.clone().oneshot(unauthenticated).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "session.expired"
    );

    let mut untrusted = Request::get("/api/v1/events")
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, "https://evil.test")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .unwrap();
    untrusted
        .headers_mut()
        .insert("last-event-id", HeaderValue::from_bytes(b"\xff").unwrap());
    let response = app.clone().oneshot(untrusted).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "origin.untrusted"
    );

    for value in ["", "0", "wat", "9007199254740992", "9223372036854775808"] {
        let response = app
            .clone()
            .oneshot(event_request(&cookie, Some(value)))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{value:?}"
        );
        assert_eq!(
            json_body(response).await["error"]["code"],
            "validation.failed"
        );
    }
    let maximum = app
        .clone()
        .oneshot(event_request(&cookie, Some("9007199254740991")))
        .await
        .unwrap();
    assert_eq!(maximum.status(), StatusCode::OK);
    drop(maximum);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM platform_outbox_events")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn stream_is_unbuffered_uncompressed_and_heartbeat_never_slides_idle_expiry() {
    let options = SseOptions {
        heartbeat: Duration::from_millis(15),
        poll_interval: Duration::from_mins(1),
        session_revalidate: Duration::from_millis(5),
        page_size: 50,
    };
    let (_fixture, db, app, cookie, observer) = authenticated_sse(options).await;
    let before: (i64, i64) = sqlx::query_as(
        "SELECT idle_expires_at_us,last_used_at_us FROM identity_sessions WHERE revoked_at_us IS NULL",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let response = app.oneshot(event_request(&cookie, None)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "text/event-stream"
    );
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "no-cache, no-store, no-transform"
    );
    assert!(response.headers().get(header::CONTENT_ENCODING).is_none());
    let mut stream = response.into_body().into_data_stream();
    let heartbeat = next_chunk(&mut stream).await.expect("heartbeat chunk");
    assert!(heartbeat.contains(": heartbeat"), "{heartbeat:?}");
    let after: (i64, i64) = sqlx::query_as(
        "SELECT idle_expires_at_us,last_used_at_us FROM identity_sessions WHERE revoked_at_us IS NULL",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        after, before,
        "initial auth and heartbeat must be non-sliding"
    );

    sqlx::query("UPDATE identity_sessions SET idle_expires_at_us=0")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        next_chunk(&mut stream).await.is_none(),
        "expired session closes stream"
    );
    let failure = observer
        .subscribe()
        .borrow()
        .clone()
        .expect("observable closure");
    assert_eq!(failure.code, SseFailureCode::SessionClosed);
    assert_eq!(SseOptions::production().heartbeat, Duration::from_secs(15));
}

#[tokio::test]
async fn open_stream_closes_after_logout_and_credential_revocation() {
    let options = SseOptions {
        heartbeat: Duration::from_mins(1),
        poll_interval: Duration::from_mins(1),
        session_revalidate: Duration::from_millis(5),
        page_size: 50,
    };
    let (_fixture, db, app, first_cookie, _) = authenticated_sse(options).await;
    let first = app
        .clone()
        .oneshot(event_request(&first_cookie, None))
        .await
        .unwrap();
    let mut first_stream = first.into_body().into_data_stream();
    sqlx::query("UPDATE identity_sessions SET revoked_at_us=1,revoked_reason='logout'")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(next_chunk(&mut first_stream).await.is_none());

    let account: Vec<u8> = sqlx::query_scalar("SELECT id FROM identity_accounts")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let second_cookie = seed_session(
        db.pool(),
        Uuid::from_slice(&account).unwrap(),
        "second-token",
    )
    .await;
    let second = app
        .oneshot(event_request(&second_cookie, None))
        .await
        .unwrap();
    let mut second_stream = second.into_body().into_data_stream();
    sqlx::query("UPDATE identity_accounts SET credential_version=credential_version+1")
        .execute(db.pool())
        .await
        .unwrap();
    assert!(next_chunk(&mut second_stream).await.is_none());
}

#[tokio::test]
async fn due_session_revalidation_preempts_pending_backlog_for_every_closure_reason() {
    let options = SseOptions {
        heartbeat: Duration::from_mins(1),
        poll_interval: Duration::from_mins(1),
        session_revalidate: Duration::from_millis(5),
        page_size: 2,
    };
    for closure in ["idle", "absolute", "logout", "credential"] {
        let (_fixture, db, app, cookie, observer) = authenticated_sse(options).await;
        let task = Uuid::now_v7();
        for id in 1..=6 {
            insert_state_event(db.pool(), id, task).await;
        }
        let response = app.oneshot(event_request(&cookie, None)).await.unwrap();
        let mut stream = response.into_body().into_data_stream();
        assert!(next_chunk(&mut stream).await.unwrap().contains("id: 1"));
        match closure {
            "idle" => {
                sqlx::query("UPDATE identity_sessions SET idle_expires_at_us=0")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            "absolute" => {
                sqlx::query("UPDATE identity_sessions SET absolute_expires_at_us=0")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            "logout" => {
                sqlx::query("UPDATE identity_sessions SET revoked_at_us=1,revoked_reason='logout'")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            "credential" => {
                sqlx::query("UPDATE identity_accounts SET credential_version=credential_version+1")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        tokio::time::sleep(Duration::from_millis(8)).await;
        assert!(
            next_chunk(&mut stream).await.is_none(),
            "{closure} must stop a due stream before the next pending event"
        );
        assert_eq!(
            observer.subscribe().borrow().clone().unwrap().code,
            SseFailureCode::SessionClosed
        );
    }
}

#[tokio::test]
async fn due_session_revalidation_preempts_post_connect_live_events_for_every_closure_reason() {
    let options = SseOptions {
        heartbeat: Duration::from_mins(1),
        poll_interval: Duration::from_mins(1),
        session_revalidate: Duration::from_millis(5),
        page_size: 2,
    };
    for closure in ["idle", "absolute", "logout", "credential"] {
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
        let cookie = seed_session(db.pool(), account, closure).await;
        let notifier = OutboxNotifier::new();
        let observer = SseObserver::new();
        let app = router_with_observer(
            fixture.config().clone(),
            &db,
            notifier.clone(),
            options,
            observer.clone(),
        );
        let response = app.oneshot(event_request(&cookie, None)).await.unwrap();
        let mut stream = response.into_body().into_data_stream();
        let task = Uuid::now_v7();
        insert_state_event(db.pool(), 1, task).await;
        notifier.notify_after_commit();
        assert!(next_chunk(&mut stream).await.unwrap().contains("id: 1"));
        match closure {
            "idle" => {
                sqlx::query("UPDATE identity_sessions SET idle_expires_at_us=0")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            "absolute" => {
                sqlx::query("UPDATE identity_sessions SET absolute_expires_at_us=0")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            "logout" => {
                sqlx::query("UPDATE identity_sessions SET revoked_at_us=1,revoked_reason='logout'")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            "credential" => {
                sqlx::query("UPDATE identity_accounts SET credential_version=credential_version+1")
                    .execute(db.pool())
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        for id in 2..=10 {
            insert_state_event(db.pool(), id, task).await;
            notifier.notify_after_commit();
        }
        tokio::time::sleep(Duration::from_millis(8)).await;
        assert!(
            next_chunk(&mut stream).await.is_none(),
            "{closure} must preempt post-connect live events when revalidation is due"
        );
        assert_eq!(
            observer.subscribe().borrow().clone().unwrap().code,
            SseFailureCode::SessionClosed
        );
    }
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

async fn authenticated_sse(
    options: SseOptions,
) -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    SseObserver,
) {
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
    let cookie = seed_session(db.pool(), account, "first-token").await;
    let observer = SseObserver::new();
    let app = router_with_observer(
        fixture.config().clone(),
        &db,
        OutboxNotifier::new(),
        options,
        observer.clone(),
    );
    (fixture, db, app, cookie, observer)
}

async fn seed_session(pool: &sqlx::SqlitePool, account: Uuid, token: &str) -> String {
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
    .bind([7_u8; 32].as_slice())
    .bind(now + 60 * 60 * 1_000_000)
    .bind(now + 60 * 60 * 1_000_000)
    .bind(1_i64)
    .bind(now - 1_000_000)
    .bind(now - 1_000_000)
    .execute(pool)
    .await
    .unwrap();
    format!("__Host-mediaflow_session={token}")
}

fn event_request(cookie: &str, cursor: Option<&str>) -> Request<Body> {
    let mut builder = Request::get("/api/v1/events")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin");
    if let Some(cursor) = cursor {
        builder = builder.header("last-event-id", cursor);
    }
    builder.body(Body::empty()).unwrap()
}

async fn next_chunk(stream: &mut BodyDataStream) -> Option<String> {
    let result = tokio::time::timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("stream made progress")?;
    Some(String::from_utf8(result.expect("body chunk").to_vec()).unwrap())
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap()).unwrap()
}
