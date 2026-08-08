#![allow(clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use mediaflow_core::automation::event_routes::router;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "automation-event-session-token";
const CSRF: &str = "automation-event-csrf-token";
const ORIGIN: &str = "http://127.0.0.1:3000";

#[tokio::test]
async fn event_admin_routes_filter_read_retry_cancel_and_mount_with_strict_guards() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let first = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let second = common::seed_automation_event(db.pool(), &fixture.config().config_dir).await;
    let source_id: Vec<u8> =
        sqlx::query_scalar("SELECT source_id FROM automation_events WHERE id=?")
            .bind(first.as_bytes().as_slice())
            .fetch_one(db.pool())
            .await
            .unwrap();
    seed_session(db.pool()).await;
    sqlx::query(
        "UPDATE automation_events SET status='failed',failure_code='integration.unavailable',
         projection_version=projection_version+1 WHERE id=?",
    )
    .bind(second.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    let cookie = format!("__Host-mediaflow_session={COOKIE_TOKEN}");
    let app = router(fixture.config().clone(), &db);

    assert_eq!(
        app.clone()
            .oneshot(
                Request::get("/api/v1/automation-events")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.clone()
            .oneshot(authenticated_get(
                "/api/v1/automation-events?status=not-real",
                &cookie,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let source_id = Uuid::from_slice(&source_id).unwrap();
    let listed = app
        .clone()
        .oneshot(authenticated_get(
            &format!(
                "/api/v1/automation-events?source_id={source_id}&status=pending&action=create-download&limit=20"
            ),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let text = body_text(listed).await;
    assert!(text.contains(&first.to_string()));
    assert!(!text.contains("magnet:"));
    assert!(!text.contains("payload_ciphertext"));

    let detail = app
        .clone()
        .oneshot(authenticated_get(
            &format!("/api/v1/automation-events/{first}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json_body(detail).await["status"], "pending");

    let missing_csrf = command_request(
        Method::POST,
        &format!("/api/v1/automation-events/{first}/cancellations"),
        &cookie,
        "cancel-event-1",
        false,
    );
    assert_eq!(
        app.clone().oneshot(missing_csrf).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    let cancelled = app
        .clone()
        .oneshot(command_request(
            Method::POST,
            &format!("/api/v1/automation-events/{first}/cancellations"),
            &cookie,
            "cancel-event-1",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::ACCEPTED);
    let cancelled = json_body(cancelled).await;
    assert_eq!(cancelled["status"], "cancelled");
    let projection = cancelled["projection_version"].clone();
    let replay = app
        .clone()
        .oneshot(command_request(
            Method::POST,
            &format!("/api/v1/automation-events/{first}/cancellations"),
            &cookie,
            "cancel-event-1",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(json_body(replay).await["projection_version"], projection);

    let retried = app
        .clone()
        .oneshot(command_request(
            Method::POST,
            &format!("/api/v1/automation-events/{second}/retries"),
            &cookie,
            "retry-event-2",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(retried.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(retried).await["status"], "pending");

    let audit: String = sqlx::query_scalar(
        "SELECT COALESCE(GROUP_CONCAT(action || safe_details_json), '') FROM platform_audit_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(audit.contains("automation.event-cancel"));
    assert!(audit.contains("automation.event-retry"));
    assert!(!audit.contains("magnet:"));

    let composed = build_router(fixture.config().clone(), Some(db));
    assert_eq!(
        composed
            .oneshot(authenticated_get(
                "/api/v1/automation-events?action=create-download",
                &cookie,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

async fn seed_session(pool: &sqlx::SqlitePool) {
    let account = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','not-used',0,0)",
    )
    .bind(account.as_bytes().as_slice())
    .execute(pool)
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
    .bind(random::sha256(COOKIE_TOKEN.as_bytes()).as_slice())
    .bind(random::sha256(CSRF.as_bytes()).as_slice())
    .bind(i64::MAX - 1)
    .bind(i64::MAX)
    .bind(1_i64)
    .bind(0_i64)
    .bind(0_i64)
    .execute(pool)
    .await
    .unwrap();
}

fn authenticated_get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn command_request(
    method: Method,
    path: &str,
    cookie: &str,
    key: &str,
    csrf: bool,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header("idempotency-key", key);
    if csrf {
        request = request.header("x-csrf-token", CSRF);
    }
    request.body(Body::empty()).unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}
