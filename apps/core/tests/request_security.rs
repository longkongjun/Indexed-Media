mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt;

#[tokio::test]
async fn public_identity_mutations_need_same_origin_but_not_cookie_or_csrf() {
    let (app, _) = app_with_secret().await;
    let accepted = app.clone().oneshot(public_post(
        "/api/v1/system/bootstrap", Some("http://127.0.0.1:3000"), "same-origin",
        json!({"bootstrap_secret":"bootstrap secret value","administrator_name":"admin","password":"correct horse battery staple"}),
    )).await.unwrap();
    assert_eq!(accepted.status(), StatusCode::CREATED);

    let missing = app
        .clone()
        .oneshot(public_post(
            "/api/v1/sessions",
            None,
            "",
            json!({"administrator_name":"admin","password":"correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(missing).await, "origin.untrusted");

    let cross_site = app
        .oneshot(public_post(
            "/api/v1/sessions",
            Some("http://127.0.0.1:3000"),
            "cross-site",
            json!({"administrator_name":"admin","password":"correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(cross_site.status(), StatusCode::FORBIDDEN);
    assert_eq!(error_code(cross_site).await, "origin.untrusted");
}

#[tokio::test]
async fn public_identity_mutations_require_json_content_type_with_stable_error() {
    let (app, _) = app_with_secret().await;
    let response = app.oneshot(
        Request::post("/api/v1/system/bootstrap")
            .header(header::ORIGIN, "http://127.0.0.1:3000")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(json!({"bootstrap_secret":"bootstrap secret value","administrator_name":"admin","password":"correct horse battery staple"}).to_string()))
            .unwrap(),
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(response).await, "validation.failed");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn authenticated_mutation_guard_order_is_session_then_origin_then_csrf() {
    let (app, db) = app_with_secret().await;
    let bootstrap = app.clone().oneshot(public_post("/api/v1/system/bootstrap", Some("http://127.0.0.1:3000"), "same-origin", json!({"bootstrap_secret":"bootstrap secret value","administrator_name":"admin","password":"correct horse battery staple"}))).await.unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    let login = app
        .clone()
        .oneshot(public_post(
            "/api/v1/sessions",
            Some("http://127.0.0.1:3000"),
            "same-origin",
            json!({"administrator_name":"admin","password":"correct horse battery staple"}),
        ))
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let csrf = json_body(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let before_rejection: (i64, i64) = sqlx::query_as(
        "SELECT idle_expires_at_us,last_used_at_us FROM identity_sessions WHERE revoked_at_us IS NULL",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();

    let no_session = app
        .clone()
        .oneshot(logout(
            None,
            Some("https://evil.test"),
            "cross-site",
            Some("bad"),
        ))
        .await
        .unwrap();
    assert_eq!(error_code(no_session).await, "session.expired");
    let bad_origin = app
        .clone()
        .oneshot(logout(
            Some(&cookie),
            Some("https://evil.test"),
            "same-origin",
            Some("bad"),
        ))
        .await
        .unwrap();
    assert_eq!(error_code(bad_origin).await, "origin.untrusted");
    let after_origin: (i64, i64) = sqlx::query_as(
        "SELECT idle_expires_at_us,last_used_at_us FROM identity_sessions WHERE revoked_at_us IS NULL",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        after_origin, before_rejection,
        "origin rejection must not slide the session"
    );
    let bad_csrf = app
        .clone()
        .oneshot(logout(
            Some(&cookie),
            Some("http://127.0.0.1:3000"),
            "same-origin",
            Some("bad"),
        ))
        .await
        .unwrap();
    assert_eq!(error_code(bad_csrf).await, "csrf.invalid");
    let after_csrf: (i64, i64) = sqlx::query_as(
        "SELECT idle_expires_at_us,last_used_at_us FROM identity_sessions WHERE revoked_at_us IS NULL",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        after_csrf, before_rejection,
        "CSRF rejection must not slide the session"
    );

    let revoked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM identity_sessions WHERE revoked_at_us IS NOT NULL",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(
        revoked, 0,
        "guards must reject before the logout side effect"
    );
    let denied_security_events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM platform_audit_events WHERE action IN ('origin','csrf') AND outcome='denied'")
        .fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        denied_security_events, 2,
        "failed origin and CSRF checks must be audited without secrets"
    );
    let valid = app
        .oneshot(logout(
            Some(&cookie),
            Some("http://127.0.0.1:3000"),
            "same-origin",
            Some(&csrf),
        ))
        .await
        .unwrap();
    assert_eq!(valid.status(), StatusCode::NO_CONTENT);
}

async fn app_with_secret() -> (axum::Router, mediaflow_core::platform::db::Db) {
    let fixture = Box::leak(Box::new(TestConfigDir::new(RunMode::Development)));
    let path = fixture.config().config_dir.join("bootstrap.secret");
    std::fs::write(&path, "bootstrap secret value").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    (build_router(fixture.config().clone(), Some(db.clone())), db)
}

#[allow(clippy::needless_pass_by_value)]
fn public_post(path: &str, origin: Option<&str>, fetch_site: &str, body: Value) -> Request<Body> {
    let mut builder = Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("sec-fetch-site", fetch_site);
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    builder.body(Body::from(body.to_string())).unwrap()
}
fn logout(
    cookie: Option<&str>,
    origin: Option<&str>,
    fetch_site: &str,
    csrf: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::delete("/api/v1/session").header("sec-fetch-site", fetch_site);
    if let Some(value) = cookie {
        builder = builder.header(header::COOKIE, value);
    }
    if let Some(value) = origin {
        builder = builder.header(header::ORIGIN, value);
    }
    if let Some(value) = csrf {
        builder = builder.header("x-csrf-token", value);
    }
    builder.body(Body::empty()).unwrap()
}
async fn error_code(response: axum::response::Response) -> String {
    json_body(response).await["error"]["code"]
        .as_str()
        .unwrap()
        .to_owned()
}
async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
