#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use mediaflow_core::automation::routes::router;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::{DownloaderConnectionInput, DownloaderKind};
use mediaflow_core::connectors::model::SecretString;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "automation-source-session-token";
const CSRF: &str = "automation-source-csrf-token";
const ORIGIN: &str = "http://127.0.0.1:3000";
const FEED_URL: &str = "https://feeds.example.test/rss.xml?token=api-feed-secret";

#[tokio::test]
async fn create_read_replace_delete_are_guarded_strict_and_redacted() {
    let (fixture, db, app, cookie, downloader_id) = authenticated_app().await;
    let unauthenticated = app
        .clone()
        .oneshot(
            Request::get("/api/v1/automation-sources")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let unknown = json_request(
        Method::POST,
        "/api/v1/automation-sources",
        Some(&cookie),
        Some(CSRF),
        Some(ORIGIN),
        None,
        json!({
            "kind":"rss","display_name":"Feed","enabled":true,"feed_url":FEED_URL,
            "downloader_connection_id":downloader_id,"poll_interval_seconds":300,"secret_echo":true
        }),
    );
    assert_eq!(
        app.clone().oneshot(unknown).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let missing_csrf = json_request(
        Method::POST,
        "/api/v1/automation-sources",
        Some(&cookie),
        None,
        Some(ORIGIN),
        None,
        rss_json(downloader_id),
    );
    assert_eq!(
        app.clone().oneshot(missing_csrf).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let created = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/automation-sources",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            rss_json(downloader_id),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let text = body_text(created).await;
    assert!(!text.contains("api-feed-secret"));
    assert!(!text.contains("feed_url"));
    let created: Value = serde_json::from_str(&text).unwrap();
    let rss_id = created["id"].as_str().unwrap();
    assert_eq!(created["endpoint_summary"], "https://feeds.example.test");

    let webhook = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/v1/automation-sources",
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            json!({"kind":"webhook","display_name":"Hook","enabled":true,"allowed_actions":["download.create"]}),
        ))
        .await
        .unwrap();
    assert_eq!(webhook.status(), StatusCode::CREATED);
    let webhook = json_body(webhook).await;
    let webhook_id = webhook["source"]["id"].as_str().unwrap();
    let one_time_secret = webhook["secret"].as_str().unwrap().to_owned();
    assert!(one_time_secret.len() >= 43);

    let read = app
        .clone()
        .oneshot(authenticated_get(
            &format!("/api/v1/automation-sources/{webhook_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    let read = body_text(read).await;
    assert!(!read.contains(&one_time_secret));
    assert!(!read.contains("secret\""));

    let rotation_path = format!("/api/v1/automation-sources/{webhook_id}/secret-rotations");
    let rotated = app
        .clone()
        .oneshot(rotation_request(
            &rotation_path,
            &cookie,
            "1",
            "automation-source-rotation-001",
        ))
        .await
        .unwrap();
    assert_eq!(rotated.status(), StatusCode::OK);
    let rotated = json_body(rotated).await;
    let rotated_secret = rotated["secret"].as_str().unwrap().to_owned();
    assert_ne!(rotated_secret, one_time_secret);
    assert_eq!(rotated["source"]["config_version"], 2);
    let replay = app
        .clone()
        .oneshot(rotation_request(
            &rotation_path,
            &cookie,
            "1",
            "automation-source-rotation-001",
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(json_body(replay).await["secret"], rotated_secret);

    let post_rotation_read = app
        .clone()
        .oneshot(authenticated_get(
            &format!("/api/v1/automation-sources/{webhook_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    let post_rotation_read = body_text(post_rotation_read).await;
    assert!(!post_rotation_read.contains(&rotated_secret));

    let stale = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            &format!("/api/v1/automation-sources/{rss_id}"),
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("0"),
            rss_json(downloader_id),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);

    let enabled_delete = app
        .clone()
        .oneshot(empty_mutation(
            Method::DELETE,
            &format!("/api/v1/automation-sources/{rss_id}"),
            &cookie,
            "1",
        ))
        .await
        .unwrap();
    assert_eq!(enabled_delete.status(), StatusCode::CONFLICT);

    let audit = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(GROUP_CONCAT(action || safe_details_json), '') FROM platform_audit_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(audit.contains("automation.source-create"));
    assert!(!audit.contains("api-feed-secret"));
    assert!(!audit.contains(&one_time_secret));
    assert!(!audit.contains(&rotated_secret));

    let composed = build_router(fixture.config().clone(), Some(db));
    let response = composed
        .oneshot(authenticated_get(
            "/api/v1/automation-sources?limit=20",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn authenticated_app() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    Uuid,
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
    .execute(db.pool())
    .await
    .unwrap();
    let downloader_id = Uuid::now_v7();
    DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            downloader_id,
            DownloaderConnectionInput {
                kind: DownloaderKind::Qbittorrent,
                display_name: "qBit".to_owned(),
                base_url: "https://download.example.test".to_owned(),
                username: SecretString::new(String::new()),
                password: SecretString::new(String::new()),
                enabled: true,
            },
            1,
        )
        .await
        .unwrap();
    let app = router(fixture.config().clone(), &db);
    (
        fixture,
        db,
        app,
        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        downloader_id,
    )
}

fn rss_json(downloader_id: Uuid) -> Value {
    json!({
        "kind":"rss","display_name":"Feed","enabled":true,"feed_url":FEED_URL,
        "downloader_connection_id":downloader_id,"poll_interval_seconds":300
    })
}

fn authenticated_get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn json_request(
    method: Method,
    path: &str,
    cookie: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
    if_match: Option<&str>,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("sec-fetch-site", "same-origin");
    if let Some(value) = cookie {
        builder = builder.header(header::COOKIE, value);
    }
    if let Some(value) = csrf {
        builder = builder.header("x-csrf-token", value);
    }
    if let Some(value) = origin {
        builder = builder.header(header::ORIGIN, value);
    }
    if let Some(value) = if_match {
        builder = builder.header(header::IF_MATCH, value);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn empty_mutation(method: Method, path: &str, cookie: &str, if_match: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(header::IF_MATCH, if_match)
        .body(Body::empty())
        .unwrap()
}

fn rotation_request(path: &str, cookie: &str, if_match: &str, key: &str) -> Request<Body> {
    Request::post(path)
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", CSRF)
        .header(header::ORIGIN, ORIGIN)
        .header("sec-fetch-site", "same-origin")
        .header(header::IF_MATCH, if_match)
        .header("idempotency-key", key)
        .body(Body::empty())
        .unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).unwrap()
}
