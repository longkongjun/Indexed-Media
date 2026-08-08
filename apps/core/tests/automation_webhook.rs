#![allow(clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::automation::model::{AutomationSourceInput, WebhookAction};
use mediaflow_core::automation::source_store::AutomationSourceStore;
use mediaflow_core::automation::webhook::router;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::downloader::connection_store::DownloaderConnectionStore;
use mediaflow_core::connectors::downloader::model::{DownloaderConnectionInput, DownloaderKind};
use mediaflow_core::connectors::model::SecretString;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tower::ServiceExt as _;
use uuid::Uuid;

const WEBHOOK_SECRET: &str = "webhook-secret-for-hmac-tests-0001";

#[tokio::test]
async fn valid_fixed_actions_create_one_durable_redacted_event_without_a_session() {
    let (fixture, db, source_id, downloader_id, inbox_id) = fixture().await;
    let app = router(fixture.config().clone(), &db);
    let timestamp = chrono::Utc::now().timestamp().to_string();

    for (nonce, body, action) in [
        (
            "webhook-nonce-0001",
            json!({
                "kind":"download.create","downloader_connection_id":downloader_id,
                "source":"magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567",
                "display_name":"Webhook movie"
            }),
            "create-download",
        ),
        (
            "webhook-nonce-0002",
            json!({"kind":"inbox.reconcile","inbox_directory_id":inbox_id}),
            "reconcile-inbox",
        ),
    ] {
        let bytes = body.to_string().into_bytes();
        let response = app
            .clone()
            .oneshot(signed_request(
                source_id,
                &timestamp,
                nonce,
                &bytes,
                WEBHOOK_SECRET,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let receipt: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(receipt["status"], "pending");
        assert!(receipt["id"].is_string());

        let stored = sqlx::query_as::<_, (String, String, Vec<u8>)>(
            "SELECT action,status,payload_ciphertext FROM automation_events WHERE id=?",
        )
        .bind(
            Uuid::parse_str(receipt["id"].as_str().unwrap())
                .unwrap()
                .as_bytes()
                .as_slice(),
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(stored.0, action);
        assert_eq!(stored.1, "pending");
        assert!(
            !stored
                .2
                .windows(b"magnet:?".len())
                .any(|part| part == b"magnet:?")
        );
    }

    let composed = build_router(fixture.config().clone(), Some(db.clone()));
    let composed_body = json!({"kind":"inbox.reconcile","inbox_directory_id":inbox_id})
        .to_string()
        .into_bytes();
    let composed_response = composed
        .oneshot(signed_request(
            source_id,
            &timestamp,
            "webhook-nonce-0003",
            &composed_body,
            WEBHOOK_SECRET,
        ))
        .await
        .unwrap();
    assert_eq!(composed_response.status(), StatusCode::ACCEPTED);

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM automation_webhook_nonces")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM automation_events")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        3
    );
    let store =
        AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    store
        .replace(
            source_id,
            1,
            AutomationSourceInput::Webhook {
                display_name: "Signed webhook".to_owned(),
                enabled: false,
                allowed_actions: vec![WebhookAction::DownloadCreate, WebhookAction::InboxReconcile],
            },
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        store.delete(source_id, 2).await.unwrap_err().code(),
        mediaflow_core::shared::error::ErrorCode::ResourceConflict
    );
}

#[tokio::test]
async fn replay_is_rejected_and_does_not_create_a_second_event() {
    let (fixture, db, source_id, downloader_id, _) = fixture().await;
    let app = router(fixture.config().clone(), &db);
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let nonce = "webhook-replay-001";
    let body = json!({
        "kind":"download.create","downloader_connection_id":downloader_id,
        "source":"https://downloads.example.test/a.torrent?token=private",
        "display_name":"Webhook replay"
    })
    .to_string()
    .into_bytes();

    let first = app
        .clone()
        .oneshot(signed_request(
            source_id,
            &timestamp,
            nonce,
            &body,
            WEBHOOK_SECRET,
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    for replay_body in [body.clone(), b"{\"kind\":\"unknown\"}".to_vec()] {
        let replay = app
            .clone()
            .oneshot(signed_request(
                source_id,
                &timestamp,
                nonce,
                &replay_body,
                WEBHOOK_SECRET,
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::CONFLICT);
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM automation_events")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn secret_rotation_invalidates_the_old_signature_and_exact_retry_returns_the_same_secret() {
    let (fixture, db, source_id, _, inbox_id) = fixture().await;
    let store =
        AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir).unwrap();
    let new_secret = "webhook-secret-after-rotation-0002";
    let rotated = store
        .rotate_webhook_secret(
            source_id,
            1,
            "webhook-rotation-idempotency-001",
            SecretString::new(new_secret.to_owned()),
            2,
        )
        .await
        .unwrap();
    assert_eq!(rotated.source.config_version, 2);
    assert_eq!(rotated.secret.expose(), new_secret.as_bytes());
    let replay = store
        .rotate_webhook_secret(
            source_id,
            1,
            "webhook-rotation-idempotency-001",
            SecretString::new("ignored-new-random-secret-value-03".to_owned()),
            3,
        )
        .await
        .unwrap();
    assert_eq!(replay.secret.expose(), new_secret.as_bytes());

    let app = router(fixture.config().clone(), &db);
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let body = json!({"kind":"inbox.reconcile","inbox_directory_id":inbox_id})
        .to_string()
        .into_bytes();
    let old = app
        .clone()
        .oneshot(signed_request(
            source_id,
            &timestamp,
            "rotated-secret-nonce-01",
            &body,
            WEBHOOK_SECRET,
        ))
        .await
        .unwrap();
    assert_eq!(old.status(), StatusCode::UNAUTHORIZED);
    let current = app
        .oneshot(signed_request(
            source_id,
            &timestamp,
            "rotated-secret-nonce-02",
            &body,
            new_secret,
        ))
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::ACCEPTED);
}

async fn fixture() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    Uuid,
    Uuid,
    Uuid,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let downloader_id = Uuid::now_v7();
    DownloaderConnectionStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            downloader_id,
            DownloaderConnectionInput {
                kind: DownloaderKind::Qbittorrent,
                display_name: "Webhook qBit".to_owned(),
                base_url: "https://download.example.test".to_owned(),
                username: SecretString::new(String::new()),
                password: SecretString::new(String::new()),
                enabled: true,
            },
            1,
        )
        .await
        .unwrap();
    let inbox_id = common::seed_inbox(db.pool()).await;
    let source_id = Uuid::now_v7();
    AutomationSourceStore::open(db.pool().clone(), &fixture.config().config_dir)
        .unwrap()
        .insert(
            source_id,
            AutomationSourceInput::Webhook {
                display_name: "Signed webhook".to_owned(),
                enabled: true,
                allowed_actions: vec![WebhookAction::DownloadCreate, WebhookAction::InboxReconcile],
            },
            Some(SecretString::new(WEBHOOK_SECRET.to_owned())),
            1,
        )
        .await
        .unwrap();
    (fixture, db, source_id, downloader_id, inbox_id)
}

fn signed_request(
    source_id: Uuid,
    timestamp: &str,
    nonce: &str,
    body: &[u8],
    secret: &str,
) -> Request<Body> {
    let signature = hmac_sha256(
        secret.as_bytes(),
        &[timestamp.as_bytes(), b"\n", nonce.as_bytes(), b"\n", body].concat(),
    );
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
