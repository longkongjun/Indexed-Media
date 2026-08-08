#![allow(clippy::too_many_lines)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::catalog::model::{
    ArtworkState, LocalStatus, MediaItemKind, NfoStatus, VerifiedArtworkRef, VerifiedFileAsset,
    VerifiedLocalResult, VerifiedMediaTree, VerifiedMediaVersion,
};
use mediaflow_core::catalog::store::CatalogStore;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::random;
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "catalog-api-session";

#[tokio::test]
async fn catalog_reads_require_auth_and_return_honest_empty_pages() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    seed_session(db.pool(), account).await;
    let app = build_router(fixture.config().clone(), Some(db));

    let unauthenticated = app
        .clone()
        .oneshot(get("/api/v1/media-items", ""))
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let empty = app
        .oneshot(get("/api/v1/media-items", cookie().as_str()))
        .await
        .unwrap();
    assert_eq!(empty.status(), StatusCode::OK);
    let body = json_body(empty).await;
    assert_eq!(body["items"], serde_json::json!([]));
    assert!(body["next_cursor"].is_null());
}

#[tokio::test]
async fn catalog_list_and_detail_enforce_exact_filters_and_redacted_contract() {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    seed_session(db.pool(), account).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease = common::seed_processing_lease(
        db.pool(),
        inbox,
        b"incoming/Dune.2021.mkv",
        vec![7],
        "catalog-api",
    )
    .await;
    let library_id = Uuid::now_v7();
    let file_id = Uuid::now_v7();
    let media_id = Uuid::now_v7();
    CatalogStore::new(db.pool().clone())
        .apply_verified_local_result(
            account,
            &VerifiedLocalResult {
                result_id: Uuid::now_v7(),
                task_id: lease.task.id,
                library_id,
                media: VerifiedMediaTree {
                    id: media_id,
                    kind: MediaItemKind::Movie,
                    title: "Dune".to_owned(),
                    year: Some(2021),
                    local_status: LocalStatus::Partial,
                    metadata: vec![],
                    artwork_refs: vec![VerifiedArtworkRef {
                        id: Uuid::now_v7(),
                        kind: mediaflow_core::catalog::model::ArtworkKind::Poster,
                        state: ArtworkState::Missing,
                        local_relative_path: None,
                    }],
                    nodes: vec![],
                    versions: vec![VerifiedMediaVersion {
                        id: Uuid::now_v7(),
                        owner_node_id: None,
                        label: None,
                        file_asset_ids: vec![file_id],
                    }],
                },
                file_assets: vec![VerifiedFileAsset {
                    id: file_id,
                    file_revision_id: lease.task.file_revision_id,
                    source_relative_path: lease.task.relative_path,
                    current_relative_path: "Movies/Dune (2021)/Dune (2021).mkv".to_owned(),
                    size_bytes: 1024,
                }],
                nfo_status: NfoStatus::Partial,
            },
            200_000_000,
        )
        .await
        .unwrap();
    let app = build_router(fixture.config().clone(), Some(db));
    let filter = format!(
        "/api/v1/media-items?type=movie&library_id={library_id}&local_status=partial&q=dune&limit=1"
    );
    let list = app
        .clone()
        .oneshot(get(&filter, cookie().as_str()))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(
        json_body(list).await["items"][0]["id"],
        media_id.to_string()
    );

    let detail = app
        .clone()
        .oneshot(get(
            &format!("/api/v1/media-items/{media_id}"),
            cookie().as_str(),
        ))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    let text = body_text(detail).await;
    assert!(!text.contains("image.tmdb.org"));
    assert!(!text.to_lowercase().contains("jellyfin"));
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["item"]["artwork_ref"]["state"], "missing");
    assert_eq!(body["nfo_status"], "partial");

    for path in [
        "/api/v1/media-items?type=movie&type=series",
        "/api/v1/media-items?unknown=value",
        "/api/v1/media-items?limit=201",
        "/api/v1/media-items?q=",
    ] {
        let response = app
            .clone()
            .oneshot(get(path, cookie().as_str()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

fn cookie() -> String {
    format!("__Host-mediaflow_session={COOKIE_TOKEN}")
}

fn get(path: &str, cookie: &str) -> Request<Body> {
    let mut request = Request::get(path);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::empty()).unwrap()
}

async fn seed_session(pool: &sqlx::SqlitePool, account: Uuid) {
    sqlx::query(
        "INSERT INTO identity_sessions
         (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,
          credential_version,created_at_us,last_used_at_us,revoked_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,NULL)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account.as_bytes().as_slice())
    .bind(random::sha256(COOKIE_TOKEN.as_bytes()).as_slice())
    .bind(random::sha256(b"catalog-csrf").as_slice())
    .bind(i64::MAX - 1)
    .bind(i64::MAX)
    .bind(1_i64)
    .bind(0_i64)
    .bind(0_i64)
    .execute(pool)
    .await
    .unwrap();
}

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_str(&body_text(response).await).unwrap()
}
