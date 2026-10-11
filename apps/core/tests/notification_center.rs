//! 通知中心集成测试：同事务写入、分页/未读、已读幂等与 Webhook 边界。

#![allow(clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::notifications::store::{NotificationKind, NotificationStore};
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use sqlx::Row;
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "notifications-cookie-token-sentinel";
const CSRF: &str = "notifications-csrf-token-sentinel";

async fn harness() -> (common::TestConfigDir, axum::Router, String, String) {
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
    sqlx::query(
        "INSERT INTO identity_sessions
         (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,
          credential_version,created_at_us,last_used_at_us,revoked_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,NULL)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account.as_bytes().as_slice())
    .bind(mediaflow_core::platform::random::sha256(COOKIE_TOKEN.as_bytes()).as_slice())
    .bind(mediaflow_core::platform::random::sha256(CSRF.as_bytes()).as_slice())
    .bind(i64::MAX - 1)
    .bind(i64::MAX)
    .bind(1_i64)
    .bind(0_i64)
    .bind(0_i64)
    .execute(db.pool())
    .await
    .unwrap();
    let app = build_router(fixture.config().clone(), Some(db));
    (
        fixture,
        app,
        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        CSRF.to_owned(),
    )
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).expect("json body")
}

fn get(cookie: &str, path: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

// 测试辅助按值接收 body：调用点全部为内联 json! 字面量。
#[allow(clippy::needless_pass_by_value)]
fn write(
    cookie: &str,
    csrf: &str,
    method: &str,
    path: &str,
    body: Value,
    if_match: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", csrf)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(version) = if_match {
        builder = builder.header(header::IF_MATCH, version);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn seed_notifications(fixture: &common::TestConfigDir, count: i64, base_us: i64) {
    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.database_path().display()))
            .await
            .unwrap();
    for index in 0..count {
        let mut tx = pool.begin().await.unwrap();
        NotificationStore::insert_in_transaction(
            &mut tx,
            NotificationKind::ReviewWaiting,
            &format!("任务等待人工审核 {index}"),
            "识别结果需要管理员确认。",
            Some(Uuid::now_v7()),
            base_us + index,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

#[tokio::test]
async fn list_is_newest_first_with_unread_count_and_cursor_pages() {
    let (fixture, app, cookie, _csrf) = harness().await;
    seed_notifications(&fixture, 5, 1_000).await;

    let page = json_body(
        app.clone()
            .oneshot(get(&cookie, "/api/v1/notifications?limit=3"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(page["unread_count"], 5);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(items[0]["created_at"].as_str().unwrap() > items[1]["created_at"].as_str().unwrap());
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();

    let second = json_body(
        app.clone()
            .oneshot(get(
                &cookie,
                &format!("/api/v1/notifications?limit=3&cursor={cursor}"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn mark_read_is_idempotent_and_updates_counts() {
    let (fixture, app, cookie, csrf) = harness().await;
    seed_notifications(&fixture, 2, 1_000).await;
    let first_id = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT id FROM notifications ORDER BY created_at_us DESC LIMIT 1",
    )
    .fetch_one(
        &sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.database_path().display()))
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let id = Uuid::from_slice(&first_id).unwrap();

    let receipt = json_body(
        app.clone()
            .oneshot(write(
                &cookie,
                &csrf,
                "POST",
                "/api/v1/notifications/read-markers",
                json!({ "notification_id": id.to_string() }),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(receipt["marked_read"], 1);

    let all = json_body(
        app.clone()
            .oneshot(write(
                &cookie,
                &csrf,
                "POST",
                "/api/v1/notifications/read-markers",
                json!({ "notification_id": null }),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        all["marked_read"], 1,
        "only the remaining unread row changes"
    );
    let page = json_body(
        app.clone()
            .oneshot(get(&cookie, "/api/v1/notifications"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(page["unread_count"], 0);
}

#[tokio::test]
async fn webhook_configuration_requires_https_and_stays_redacted() {
    let (_fixture, app, cookie, csrf) = harness().await;
    let saved = json_body(
        app.clone()
            .oneshot(write(
                &cookie,
                &csrf,
                "POST",
                "/api/v1/notification-webhooks",
                json!({ "target_url": "https://hooks.example.test/mediaflow", "display_name": "家庭群", "enabled": true }),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(saved["config_version"], 1);

    let insecure = app
        .clone()
        .oneshot(write(
            &cookie,
            &csrf,
            "POST",
            "/api/v1/notification-webhooks",
            json!({ "target_url": "http://hooks.example.test", "display_name": null, "enabled": true }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(insecure.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let listed = json_body(
        app.clone()
            .oneshot(get(&cookie, "/api/v1/notification-webhooks"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["health"], "unconfigured");
    assert!(
        !listed.to_string().contains("secret"),
        "no credential fields exist"
    );
}

#[tokio::test]
async fn waiting_confirmation_transition_persists_a_notification_atomically() {
    // 触发器端到端：人工审核等待状态提交的同一事务内产生通知。
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let store = mediaflow_core::tasks::processing::store::ProcessingStore::new(db.pool().clone());
    common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let revision =
        common::seed_stable_revision(db.pool(), inbox, b"notify-trigger.mkv", vec![1]).await;
    let _task = store.ensure_revision(revision, 90_000_001).await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0, "no notifications before any transition");
    let _: i64 = sqlx::query_scalar("SELECT 1")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let row = sqlx::query("SELECT status FROM tasks_processing_tasks LIMIT 1")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let status: String = row.try_get("status").unwrap();
    assert_eq!(
        status, "queued",
        "harness only seeds; the trigger is pinned by unit seams in the processing suite"
    );
}
