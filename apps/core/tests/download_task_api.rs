#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::{DownloaderConnectionInput, DownloaderKind};
use mediaflow_core::connectors::downloader::task_routes::router;
use mediaflow_core::connectors::model::SecretString;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "download-task-session-token";
const CSRF: &str = "download-task-csrf-token";
const ORIGIN: &str = "http://127.0.0.1:3000";
const SOURCE: &str = "magnet:?xt=urn:btih:API_SOURCE_MUST_NOT_LEAK";

#[tokio::test]
async fn create_replay_conflict_list_and_detail_follow_the_redacted_contract() {
    let (fixture, db, app, cookie, connection_id) = authenticated_app().await;
    let unauthenticated = app
        .clone()
        .oneshot(json_request(
            None,
            None,
            None,
            Some("task-key"),
            connection_id,
        ))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    let missing_key = app
        .clone()
        .oneshot(json_request(
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            None,
            connection_id,
        ))
        .await
        .unwrap();
    assert_eq!(missing_key.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let created = app
        .clone()
        .oneshot(json_request(
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("task-key"),
            connection_id,
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let created_text = body_text(created).await;
    assert!(!created_text.contains("API_SOURCE_MUST_NOT_LEAK"));
    assert!(!created_text.contains("magnet"));
    let created: Value = serde_json::from_str(&created_text).unwrap();
    let task_id = created["id"].as_str().unwrap();
    assert_eq!(created["status"], "queued");
    assert_eq!(created["linked"], false);
    assert_eq!(created["connection_display_name"], "Primary qBit");

    let replay = app
        .clone()
        .oneshot(json_request(
            Some(&cookie),
            Some(CSRF),
            Some(ORIGIN),
            Some("task-key"),
            connection_id,
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(json_body(replay).await["id"], task_id);

    let conflict = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/v1/download-tasks")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .header("x-csrf-token", CSRF)
                .header(header::ORIGIN, ORIGIN)
                .header("sec-fetch-site", "same-origin")
                .header("idempotency-key", "task-key")
                .body(Body::from(
                    json!({"connection_id":connection_id,"source":SOURCE,"display_name":"different"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let list = app
        .clone()
        .oneshot(authenticated_get(
            &format!("/api/v1/download-tasks?connection_id={connection_id}&status=queued&limit=20"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    let list_text = body_text(list).await;
    assert!(!list_text.contains("API_SOURCE_MUST_NOT_LEAK"));
    assert_eq!(
        serde_json::from_str::<Value>(&list_text).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let detail = app
        .clone()
        .oneshot(authenticated_get(
            &format!("/api/v1/download-tasks/{task_id}"),
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert!(!body_text(detail).await.contains("API_SOURCE_MUST_NOT_LEAK"));

    let audit = sqlx::query_scalar::<_, String>(
        "SELECT COALESCE(GROUP_CONCAT(action || safe_details_json), '') FROM platform_audit_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(audit.contains("download-task.create"));
    assert!(!audit.contains("API_SOURCE_MUST_NOT_LEAK"));
    assert!(!audit.contains("task-key"));

    let composed = build_router(fixture.config().clone(), Some(db));
    let response = composed
        .oneshot(authenticated_get("/api/v1/download-tasks", &cookie))
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
    let connections =
        DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let connection_id = Uuid::now_v7();
    connections
        .insert(
            connection_id,
            DownloaderConnectionInput {
                kind: DownloaderKind::Qbittorrent,
                display_name: "Primary qBit".to_owned(),
                base_url: "https://download.test/qbit".to_owned(),
                username: SecretString::new("user".to_owned()),
                password: SecretString::new("password".to_owned()),
                enabled: true,
            },
            100,
        )
        .await
        .unwrap();
    let app = router(fixture.config().clone(), &db);
    (
        fixture,
        db,
        app,
        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
        connection_id,
    )
}

fn json_request(
    cookie: Option<&str>,
    csrf: Option<&str>,
    origin: Option<&str>,
    idempotency_key: Option<&str>,
    connection_id: Uuid,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/download-tasks")
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
    if let Some(value) = idempotency_key {
        builder = builder.header("idempotency-key", value);
    }
    builder
        .body(Body::from(
            json!({"connection_id":connection_id,"source":SOURCE,"display_name":"Ubuntu ISO"})
                .to_string(),
        ))
        .unwrap()
}

fn authenticated_get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).unwrap()
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
