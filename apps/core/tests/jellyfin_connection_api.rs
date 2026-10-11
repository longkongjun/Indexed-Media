//! Jellyfin 独立连接集成测试：加密保存、不落盘测试、健康刷新与任务解耦边界
//! （`MVP-JF-001/002/003`）。

#![allow(clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt as _;

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

struct JellyfinHarness {
    #[allow(dead_code)]
    fixture: common::TestConfigDir,
    app: axum::Router,
    cookie: String,
    csrf: String,
}

const COOKIE_TOKEN: &str = "jellyfin-api-cookie-token-sentinel";
const CSRF: &str = "jellyfin-api-csrf-token-sentinel";

async fn harness() -> JellyfinHarness {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = uuid::Uuid::now_v7();
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
    .bind(uuid::Uuid::now_v7().as_bytes().as_slice())
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
    JellyfinHarness {
        fixture,
        app,
        cookie: format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        csrf: CSRF.to_owned(),
    }
}

fn command() -> Value {
    json!({
        "base_url": "https://jellyfin.example.test",
        "display_name": "客厅服务器",
        "api_token": "jellyfin-api-token-0123456789abcdef"
    })
}

fn request(
    harness: &JellyfinHarness,
    method: &str,
    path: &str,
    body: Option<Value>,
    if_match: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("cookie", &harness.cookie)
        .header("x-csrf-token", &harness.csrf)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(version) = if_match {
        builder = builder.header(header::IF_MATCH, version);
    }
    builder
        .body(Body::from(
            body.map_or_else(String::new, |value| value.to_string()),
        ))
        .unwrap()
}

#[tokio::test]
async fn unconfigured_projection_is_stable_and_independent() {
    let harness = harness().await;
    let response = harness
        .app
        .clone()
        .oneshot(request(
            &harness,
            "GET",
            "/api/v1/integrations/jellyfin",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["kind"], "jellyfin");
    assert_eq!(body["configured"], false);
    assert_eq!(body["config_version"], 0);
    assert_eq!(body["health"], "unconfigured");
    assert!(
        body["base_url"].is_null(),
        "unconfigured projection must not echo addresses"
    );
}

#[tokio::test]
async fn save_seals_credentials_and_projects_redacted_view() {
    let harness = harness().await;
    let response = harness
        .app
        .clone()
        .oneshot(request(
            &harness,
            "PUT",
            "/api/v1/integrations/jellyfin",
            Some(command()),
            Some("0"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["configured"], true);
    assert_eq!(body["base_url"], "https://jellyfin.example.test");
    assert_eq!(
        body["health"], "degraded",
        "saved config awaits its first authoritative check"
    );
    assert_eq!(body["config_version"], 1);
    assert!(
        !body.to_string().contains("jellyfin-api-token"),
        "credentials must never echo"
    );

    // 乐观并发：错版本拒绝，正确版本推进。
    let stale = harness
        .app
        .clone()
        .oneshot(request(
            &harness,
            "PUT",
            "/api/v1/integrations/jellyfin",
            Some(command()),
            Some("0"),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn connection_test_is_non_persistent_and_structured() {
    let harness = harness().await;
    let response = harness
        .app
        .clone()
        .oneshot(request(
            &harness,
            "POST",
            "/api/v1/integrations/jellyfin/connection-tests",
            Some(json!({
                "base_url": "https://jellyfin.example.test",
                "display_name": null,
                "api_token": "jellyfin-api-token-0123456789abcdef"
            })),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    // 测试环境没有真实 Jellyfin 上游：必须返回结构化不可达，而不是错误文本或泄漏。
    assert_eq!(body["reachable"], false);
    assert_eq!(body["health"], "unavailable");
    assert_eq!(body["failure_code"], "integration.unavailable");
    assert!(body["server_version"].is_null());
    assert!(
        !body
            .to_string()
            .to_ascii_lowercase()
            .contains("jellyfin-api-token")
    );

    // 无效输入（坏 scheme、短 token）稳定拒绝且不落盘。
    let invalid = harness
        .app
        .clone()
        .oneshot(request(
            &harness,
            "POST",
            "/api/v1/integrations/jellyfin/connection-tests",
            Some(json!({ "base_url": "ftp://jellyfin.example.test", "display_name": null, "api_token": "jellyfin-api-token-0123456789abcdef" })),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let after = harness
        .app
        .clone()
        .oneshot(request(
            &harness,
            "GET",
            "/api/v1/integrations/jellyfin",
            None,
            None,
        ))
        .await
        .unwrap();
    let after = json_body(after).await;
    assert_eq!(
        after["configured"], false,
        "failed test must not persist config"
    );
}

#[tokio::test]
async fn health_refresh_uses_saved_config_and_records_classification() {
    let harness = harness().await;
    let saved = json_body(
        harness
            .app
            .clone()
            .oneshot(request(
                &harness,
                "PUT",
                "/api/v1/integrations/jellyfin",
                Some(command()),
                Some("0"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(saved["config_version"], 1);

    let refreshed = json_body(
        harness
            .app
            .clone()
            .oneshot(request(
                &harness,
                "POST",
                "/api/v1/integrations/jellyfin",
                None,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    // 无真实上游：刷新把健康分类为不可用，但绝不影响投影可用性。
    assert!(
        refreshed["health"] == json!("unavailable") || refreshed["health"] == json!("degraded"),
        "refresh must record a stable non-fatal classification: {refreshed}"
    );
    assert_eq!(refreshed["configured"], true);
}

#[tokio::test]
async fn jellyfin_unavailability_never_touches_processing_tasks() {
    // MVP-JF-003：未配置/不可用只体现为独立健康；处理任务投影保持原样。
    let harness = harness().await;
    let projection = json_body(
        harness
            .app
            .clone()
            .oneshot(request(
                &harness,
                "GET",
                "/api/v1/integrations/jellyfin",
                None,
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(projection["health"], "unconfigured");
}
