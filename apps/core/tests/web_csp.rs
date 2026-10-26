mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use tower::ServiceExt;

#[tokio::test]
async fn html_style_nonce_is_fresh_matches_policy_and_does_not_weaken_other_responses() {
    let fixture = TestConfigDir::new(RunMode::Development);
    fixture.create_web_dist();
    std::fs::write(
        fixture.config().web_dist.join("index.html"),
        "<html><head><meta name=\"csp-nonce\" content=\"__MEDIAFLOW_STYLE_NONCE__\"></head></html>",
    )
    .unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db));
    let mut previous = String::new();
    for path in ["/", "/index.html", "/media/example"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .to_owned();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        let nonce = html
            .split("content=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_ne!(nonce, "__MEDIAFLOW_STYLE_NONCE__");
        assert_eq!(nonce.len(), 43);
        assert!(
            nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        );
        assert_ne!(nonce, previous);
        assert_eq!(
            policy,
            format!(
                "default-src 'self'; connect-src 'self'; frame-ancestors 'none'; style-src 'self' 'nonce-{nonce}'"
            )
        );
        previous = nonce.to_owned();
    }
    for path in ["/app.js", "/health/live", "/api/missing"] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'self'; connect-src 'self'; frame-ancestors 'none'"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(response.headers()[header::REFERRER_POLICY], "same-origin");
    }
}

#[tokio::test]
async fn old_html_keeps_strict_policy_without_fake_nonce() {
    let fixture = TestConfigDir::new(RunMode::Development);
    fixture.create_web_dist();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let response = build_router(fixture.config().clone(), Some(db))
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::CONTENT_SECURITY_POLICY],
        "default-src 'self'; connect-src 'self'; frame-ancestors 'none'"
    );
}
