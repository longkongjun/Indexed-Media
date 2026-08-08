mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use mediaflow_core::automation::model::{AutomationSourceInput, WebhookAction};
use mediaflow_core::automation::source_store::AutomationSourceStore;
use mediaflow_core::automation::webhook::router;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::SecretString;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tower::ServiceExt as _;
use uuid::Uuid;

const SECRET: &str = "webhook-security-secret-value-0001";

#[tokio::test]
async fn signature_timestamp_nonce_and_body_limits_fail_before_persistence() {
    let (test_fixture, db, source_id) = fixture(true).await;
    let app = router(test_fixture.config().clone(), &db);
    let now = chrono::Utc::now().timestamp();
    let body = json!({"kind":"inbox.reconcile","inbox_directory_id":Uuid::now_v7()})
        .to_string()
        .into_bytes();

    let cases = [
        signed_request(
            source_id,
            &(now - 301).to_string(),
            "security-nonce-001",
            &body,
            SECRET,
        ),
        signed_request(
            source_id,
            &(now + 301).to_string(),
            "security-nonce-002",
            &body,
            SECRET,
        ),
        signed_request(source_id, &now.to_string(), "short", &body, SECRET),
        signed_request(
            source_id,
            &now.to_string(),
            "security-nonce-003",
            &body,
            "wrong-secret-value-that-is-long-enough",
        ),
        Request::post(format!("/api/v1/source-webhooks/{source_id}/events"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.clone()))
            .unwrap(),
    ];
    for request in cases {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let oversized = vec![b'x'; 64 * 1024 + 1];
    let response = app
        .clone()
        .oneshot(signed_request(
            source_id,
            &now.to_string(),
            "security-nonce-004",
            &oversized,
            SECRET,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(row_count(db.pool(), "automation_webhook_nonces").await, 0);
    assert_eq!(row_count(db.pool(), "automation_events").await, 0);
}

#[tokio::test]
async fn authenticated_body_is_strict_closed_and_never_accepts_paths_scripts_or_unknown_actions() {
    let (test_fixture, db, source_id) = fixture(true).await;
    let app = router(test_fixture.config().clone(), &db);
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let bodies = [
        json!({"kind":"inbox.reconcile","inbox_directory_id":Uuid::now_v7(),"path":"/mnt/private"}),
        json!({"kind":"script.run","script":"rm -rf /"}),
        json!({"kind":"download.create","downloader_connection_id":Uuid::now_v7(),"source":"file:///tmp/a.torrent","display_name":"unsafe"}),
        json!({"kind":"download.create","downloader_connection_id":Uuid::now_v7(),"source":"https://example.test/a.torrent","display_name":"unsafe","command":"execute"}),
    ];
    for (index, body) in bodies.into_iter().enumerate() {
        let body = body.to_string().into_bytes();
        let response = app
            .clone()
            .oneshot(signed_request(
                source_id,
                &timestamp,
                &format!("strict-body-nonce-{index:02}"),
                &body,
                SECRET,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
    assert_eq!(row_count(db.pool(), "automation_webhook_nonces").await, 0);
    assert_eq!(row_count(db.pool(), "automation_events").await, 0);

    let (disabled_fixture, disabled_db, disabled_id) = fixture(false).await;
    let disabled_app = router(disabled_fixture.config().clone(), &disabled_db);
    let body = json!({"kind":"inbox.reconcile","inbox_directory_id":Uuid::now_v7()})
        .to_string()
        .into_bytes();
    let response = disabled_app
        .oneshot(signed_request(
            disabled_id,
            &timestamp,
            "disabled-nonce-001",
            &body,
            SECRET,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

async fn fixture(
    enabled: bool,
) -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    Uuid,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let source_id = Uuid::now_v7();
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            source_id,
            AutomationSourceInput::Webhook {
                display_name: "Security webhook".to_owned(),
                enabled,
                allowed_actions: vec![WebhookAction::DownloadCreate, WebhookAction::InboxReconcile],
            },
            Some(SecretString::new(SECRET.to_owned())),
            1,
        )
        .await
        .unwrap();
    (fixture, db, source_id)
}

fn signed_request(
    source_id: Uuid,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
    secret: &str,
) -> Request<Body> {
    let material = [timestamp.as_bytes(), b"\n", nonce.as_bytes(), b"\n", body].concat();
    let signature = hmac_sha256(secret.as_bytes(), &material);
    Request::post(format!("/api/v1/source-webhooks/{source_id}/events"))
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-mediaflow-timestamp", timestamp)
        .header("x-mediaflow-nonce", nonce)
        .header(
            "x-mediaflow-signature",
            format!("sha256={}", hex::encode(signature)),
        )
        .body(Body::from(body.to_vec()))
        .unwrap()
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut padded = [0_u8; 64];
    if key.len() > padded.len() {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner_key = padded;
    let mut outer_key = padded;
    for byte in &mut inner_key {
        *byte ^= 0x36;
    }
    for byte in &mut outer_key {
        *byte ^= 0x5c;
    }
    let mut inner = Sha256::new();
    inner.update(inner_key);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(outer_key);
    outer.update(inner.finalize());
    outer.finalize().into()
}

async fn row_count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}
