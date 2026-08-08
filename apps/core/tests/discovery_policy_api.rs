mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt;

const SECRET: &str = "m3-discovery-policy-bootstrap-secret";

#[tokio::test]
async fn policy_api_requires_authentication_and_returns_contract_shape() {
    let (_fixture, _db, app, inbox, cookie, _csrf) = authenticated_app().await;
    let path = format!("/api/v1/inbox-directories/{inbox}/discovery-policy");

    let unauthenticated = app
        .clone()
        .oneshot(Request::get(&path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::get(&path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        json_body(response).await,
        json!({
            "inbox_directory_id": inbox,
            "minimum_age_seconds": 60,
            "stable_observation_interval_seconds": 30,
            "reconcile_interval_seconds": 900,
            "watcher_enabled": true,
            "config_version": 1
        })
    );
}

#[tokio::test]
async fn policy_put_enforces_request_guards_schema_and_optimistic_version() {
    let (_fixture, _db, app, inbox, cookie, csrf) = authenticated_app().await;
    let path = format!("/api/v1/inbox-directories/{inbox}/discovery-policy");
    let body = json!({
        "minimum_age_seconds": 120,
        "stable_observation_interval_seconds": 45,
        "reconcile_interval_seconds": 1800,
        "watcher_enabled": false
    });

    let missing_version = app
        .clone()
        .oneshot(put(&path, &cookie, &csrf, None, &body))
        .await
        .unwrap();
    assert_eq!(missing_version.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let changed = app
        .clone()
        .oneshot(put(&path, &cookie, &csrf, Some("1"), &body))
        .await
        .unwrap();
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(json_body(changed).await["config_version"], 2);

    let stale = app
        .clone()
        .oneshot(put(&path, &cookie, &csrf, Some("1"), &body))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "request.conflict");

    let mut unknown = body;
    unknown["host_path"] = json!("/private/nas");
    let invalid = app
        .oneshot(put(&path, &cookie, &csrf, Some("2"), &unknown))
        .await
        .unwrap();
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

async fn authenticated_app() -> (
    common::TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
    uuid::Uuid,
    String,
    String,
) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    std::fs::write(fixture.config().config_dir.join("bootstrap.secret"), SECRET).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            fixture.config().config_dir.join("bootstrap.secret"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let inbox = common::seed_inbox(db.pool()).await;
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let bootstrap = app
        .clone()
        .oneshot(public_post(
            "/api/v1/system/bootstrap",
            &json!({
                "bootstrap_secret": SECRET,
                "administrator_name": "admin",
                "password": "correct horse battery staple"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    let login = app
        .clone()
        .oneshot(public_post(
            "/api/v1/sessions",
            &json!({
                "administrator_name": "admin",
                "password": "correct horse battery staple"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::CREATED);
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    let csrf = json_body(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    (fixture, db, app, inbox, cookie, csrf)
}

fn put(path: &str, cookie: &str, csrf: &str, version: Option<&str>, body: &Value) -> Request<Body> {
    let mut request = Request::put(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", csrf);
    if let Some(version) = version {
        request = request.header(header::IF_MATCH, version);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

fn public_post(path: &str, body: &Value) -> Request<Body> {
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
