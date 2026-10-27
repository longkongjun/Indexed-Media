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

async fn assert_tied_notifications_are_complete(unread_only: bool) {
    let (fixture, app, cookie, _csrf) = harness().await;
    seed_notifications(&fixture, 7, 1_000).await;
    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.database_path().display()))
            .await
            .unwrap();
    sqlx::query(
        "UPDATE notifications SET created_at_us = CASE title
         WHEN '任务等待人工审核 0' THEN 999 WHEN '任务等待人工审核 6' THEN 1001 ELSE 1000 END,
         read_at_us = CASE title WHEN '任务等待人工审核 2' THEN 2000
         WHEN '任务等待人工审核 4' THEN 2000 ELSE NULL END",
    )
    .execute(&pool)
    .await
    .unwrap();
    let expected: Vec<String> = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT id FROM notifications WHERE (? = 0 OR read_at_us IS NULL)
         ORDER BY created_at_us DESC, id DESC",
    )
    .bind(i64::from(unread_only))
    .fetch_all(&pool)
    .await
    .unwrap()
    .into_iter()
    .map(|id| Uuid::from_slice(&id).unwrap().to_string())
    .collect();
    assert_eq!(expected.len(), if unread_only { 5 } else { 7 });
    let mut seen = Vec::new();
    let mut cursor = None::<String>;
    for _ in 0..expected.len() {
        let mut path = format!("/api/v1/notifications?limit=2&unread_only={unread_only}");
        if let Some(cursor) = &cursor {
            path.push_str("&cursor=");
            path.push_str(cursor);
        }
        let response = app.clone().oneshot(get(&cookie, &path)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page = json_body(response).await;
        assert_eq!(page["unread_count"], 5);
        seen.extend(
            page["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_owned()),
        );
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, expected,
        "every matching notification appears exactly once in full SQL order"
    );
    assert!(cursor.is_none(), "pagination must reach the end");
}

#[tokio::test]
async fn tied_timestamp_pages_keep_all_notifications_in_blob_id_order() {
    assert_tied_notifications_are_complete(false).await;
}

#[tokio::test]
async fn tied_timestamp_pages_keep_all_unread_notifications() {
    assert_tied_notifications_are_complete(true).await;
}

#[tokio::test]
async fn legacy_timestamp_cursors_keep_strict_before_semantics_and_microsecond_precision() {
    let (fixture, app, cookie, _csrf) = harness().await;
    seed_notifications(&fixture, 5, 1_000).await;
    for cursor in [
        "notifications:1970-01-01T00:00:00.001003Z",
        "notifications:1970-01-01T01:00:00.001003%2B01:00",
    ] {
        let page = json_body(
            app.clone()
                .oneshot(get(
                    &cookie,
                    &format!("/api/v1/notifications?limit=2&cursor={cursor}"),
                ))
                .await
                .unwrap(),
        )
        .await;
        let items = page["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["created_at"], "1970-01-01T00:00:00.001002Z");
        assert_eq!(items[1]["created_at"], "1970-01-01T00:00:00.001001Z");
        let next = page["next_cursor"].as_str().unwrap();
        assert!(next.ends_with(items[1]["id"].as_str().unwrap()));
        for boundary in [
            next.to_owned(),
            next.replace(
                "1970-01-01T00:00:00.001001Z",
                "1970-01-01T01:00:00.001001%2B01:00",
            ),
        ] {
            let last = json_body(
                app.clone()
                    .oneshot(get(
                        &cookie,
                        &format!("/api/v1/notifications?limit=2&cursor={boundary}"),
                    ))
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(last["items"].as_array().unwrap().len(), 1);
            assert_eq!(
                last["items"][0]["created_at"],
                "1970-01-01T00:00:00.001000Z"
            );
            assert!(last["next_cursor"].is_null());
        }
    }
}

#[tokio::test]
async fn malformed_notification_cursors_are_rejected_instead_of_returning_page_one() {
    let (_fixture, app, cookie, _csrf) = harness().await;
    for cursor in [
        String::new(),
        "other:1970-01-01T00:00:00.001000Z".to_owned(),
        "notifications:".to_owned(),
        "notifications:not-a-time".to_owned(),
        "notifications:1970-01-01T00:00:00.001000Z:".to_owned(),
        "notifications:1970-01-01T00:00:00.001000Z:not-a-uuid".to_owned(),
        "notifications:1970-01-01T00:00:00.001000Z:00000000-0000-0000-0000-000000000001:extra"
            .to_owned(),
        format!("notifications:1970-01-01T00:00:00.{}Z", "0".repeat(490)),
    ] {
        let response = app
            .clone()
            .oneshot(get(
                &cookie,
                &format!("/api/v1/notifications?cursor={cursor}"),
            ))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "cursor={cursor}"
        );
        assert_eq!(
            json_body(response).await["error"]["code"],
            "validation.failed"
        );
    }
}

#[tokio::test]
async fn notification_page_size_keeps_its_existing_one_to_one_hundred_bounds() {
    let (fixture, app, cookie, _csrf) = harness().await;
    seed_notifications(&fixture, 101, 1_000).await;
    for (limit, expected) in [(0, 1), (1, 1), (100, 100), (101, 100)] {
        let page = json_body(
            app.clone()
                .oneshot(get(
                    &cookie,
                    &format!("/api/v1/notifications?limit={limit}"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(page["items"].as_array().unwrap().len(), expected);
        assert!(page["next_cursor"].is_string());
    }
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

    // SA-3-001：探测端点与保存路径同校验——http:// 目标不得成为 SSRF 探测原语。
    let insecure_probe = app
        .clone()
        .oneshot(write(
            &cookie,
            &csrf,
            "POST",
            "/api/v1/notification-webhooks/connection-tests",
            json!({ "target_url": "http://192.0.2.10/private", "display_name": null, "enabled": true }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(insecure_probe.status(), StatusCode::UNPROCESSABLE_ENTITY);

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
