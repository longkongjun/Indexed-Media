//! 家庭成员最小角色集成测试：创建/列出/删除、管理端拒绝 member 会话。

#![allow(clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const ADMIN_COOKIE: &str = "family-admin-cookie-token-sentinel";
const ADMIN_CSRF: &str = "family-admin-csrf-token-sentinel";

async fn harness() -> (common::TestConfigDir, axum::Router) {
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
    .bind(mediaflow_core::platform::random::sha256(ADMIN_COOKIE.as_bytes()).as_slice())
    .bind(mediaflow_core::platform::random::sha256(ADMIN_CSRF.as_bytes()).as_slice())
    .bind(i64::MAX - 1)
    .bind(i64::MAX)
    .bind(1_i64)
    .bind(0_i64)
    .bind(0_i64)
    .execute(db.pool())
    .await
    .unwrap();
    let app = build_router(fixture.config().clone(), Some(db));
    (fixture, app)
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).expect("json body")
}

fn admin_get(path: &str) -> Request<Body> {
    Request::get(path)
        .header(
            header::COOKIE,
            format!("__Host-mediaflow_session={ADMIN_COOKIE}"),
        )
        .body(Body::empty())
        .unwrap()
}

#[allow(clippy::needless_pass_by_value)]
fn admin_write(method: &str, path: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(
            header::COOKIE,
            format!("__Host-mediaflow_session={ADMIN_COOKIE}"),
        )
        .header("x-csrf-token", ADMIN_CSRF)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn members_lifecycle_and_administrator_exclusivity() {
    let (_fixture, app) = harness().await;
    let created = json_body(
        app.clone()
            .oneshot(admin_write(
                "POST",
                "/api/v1/family-members",
                json!({ "display_name": "  小龙  ", "password": "family-password-01" }),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(created["member"]["display_name"], "小龙");
    let member_id = created["member"]["id"].as_str().unwrap().to_owned();

    let listed = json_body(
        app.clone()
            .oneshot(admin_get("/api/v1/family-members"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);

    // 名称唯一性。
    let duplicate = app
        .clone()
        .oneshot(admin_write(
            "POST",
            "/api/v1/family-members",
            json!({ "display_name": "小龙", "password": "family-password-02" }),
        ))
        .await
        .unwrap();
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    // 删除。
    let removed = app
        .clone()
        .oneshot(admin_write(
            "DELETE",
            &format!("/api/v1/family-members/{member_id}"),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn family_members_have_no_management_surface() {
    // 家庭成员不进入 identity_accounts，也就永远无法获得 MediaFlow 会话；
    // 伪造的会话 Cookie 在一切管理路由上都是 401。
    let (fixture, app) = harness().await;
    for path in [
        "/api/v1/notifications",
        "/api/v1/media-items",
        "/api/v1/capability-events",
        "/api/v1/integrations/jellyfin",
        "/api/v1/family-members",
    ] {
        let denied = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header(
                        header::COOKIE,
                        "__Host-mediaflow_session=forged-member-cookie-sentinel",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            denied.status() == StatusCode::UNAUTHORIZED || denied.status() == StatusCode::FORBIDDEN,
            "forged sessions must be denied on {path}"
        );
    }
    let _ = fixture;
}
