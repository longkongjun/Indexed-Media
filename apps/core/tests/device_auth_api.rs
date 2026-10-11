mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN_PASSWORD: &str = "correct horse battery staple";

#[tokio::test]
async fn pairing_code_enrollment_issues_device_token_that_reads_business_routes() {
    let (fixture, db, app) = bootstrapped().await;
    let (cookie, csrf) = web_login(&app).await;
    let code = issue_pairing_code(&app, &cookie, &csrf).await;

    let enrollment = app
        .clone()
        .oneshot(
            Request::post("/api/v1/devices/enrollment")
                .header(header::CONTENT_TYPE, "application/json")
                // 移动端入口：无 Cookie、无 Origin/Sec-Fetch-Site、无 CSRF 头。
                .body(Body::from(
                    json!({"pairing_code": code.to_lowercase(), "device_name": "Pixel 9"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(enrollment.status(), StatusCode::CREATED);
    let body = json_body(enrollment).await;
    let token = body["token"].as_str().unwrap();
    assert!(token.starts_with("mfdev_"));
    assert_eq!(body["device"]["display_name"], "Pixel 9");

    // 令牌只以摘要落库。
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT token_sha256 FROM identity_devices WHERE revoked_at_us IS NULL")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stored.len(), 32);
    assert_ne!(stored, token.as_bytes());

    // 设备令牌读取业务路由：仅 Authorization 头，无 cookie/origin/csrf。
    let media = app
        .clone()
        .oneshot(
            Request::get("/api/v1/media-items")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(media.status(), StatusCode::OK);

    // SSE 流同样接受设备令牌。
    let events = app
        .clone()
        .oneshot(
            Request::get("/api/v1/events")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    assert_eq!(
        events.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );

    drop(fixture);
}

#[tokio::test]
async fn credentials_enrollment_uses_login_throttle_and_rejects_wrong_passwords() {
    let (fixture, _db, app) = bootstrapped().await;
    let enrollment = |name: &str, password: &str| {
        Request::post("/api/v1/devices/enrollment")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"administrator_name": name, "password": password, "device_name": "iPhone 17"})
                    .to_string(),
            ))
            .unwrap()
    };
    let wrong = app
        .clone()
        .oneshot(enrollment("admin", "wrong password value"))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_body(wrong).await["error"]["code"],
        "auth.invalid_credentials"
    );

    let ok = app
        .clone()
        .oneshot(enrollment("admin", ADMIN_PASSWORD))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::CREATED);
    let token = json_body(ok).await["token"].as_str().unwrap().to_owned();
    let media = app
        .clone()
        .oneshot(
            Request::get("/api/v1/media-items")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(media.status(), StatusCode::OK);
    drop(fixture);
}

#[tokio::test]
async fn pairing_code_is_single_use_and_wrong_codes_are_throttled() {
    let (fixture, _db, app) = bootstrapped().await;
    let (cookie, csrf) = web_login(&app).await;
    let code = issue_pairing_code(&app, &cookie, &csrf).await;

    let first = enroll_with_code(&app, &code, "Pixel 9").await;
    assert_eq!(first.0, StatusCode::CREATED);
    // 同一码第二次使用被拒（单次使用）；该失败同样计入限流窗。
    let second = enroll_with_code(&app, &code, "Pixel 9b").await;
    assert_eq!(second.0, StatusCode::UNAUTHORIZED);
    assert_eq!(second.1["error"]["code"], "auth.invalid_credentials");

    // 复用失败已计 1 次；再错 3 次共 4 次，第 5 次触发锁定（15 分钟窗 / 5 分钟锁）。
    for index in 0..3 {
        let attempt = enroll_with_code(&app, &format!("WRONGNO{index}"), "attacker").await;
        assert_eq!(attempt.0, StatusCode::UNAUTHORIZED);
    }
    let locked = enroll_with_code(&app, "WRONGNO4", "attacker").await;
    assert_eq!(locked.0, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(locked.1["error"]["code"], "auth.rate_limited");
    drop(fixture);
}

#[tokio::test]
async fn expired_pairing_code_is_rejected() {
    let (fixture, db, app) = bootstrapped().await;
    let (cookie, csrf) = web_login(&app).await;
    let code = issue_pairing_code(&app, &cookie, &csrf).await;
    // 直接把码行改成已过期（TTL 不可注入，走存储层模拟时间流逝）。
    sqlx::query("UPDATE identity_pairing_codes SET expires_at_us=1")
        .execute(db.pool())
        .await
        .unwrap();
    let expired = enroll_with_code(&app, &code, "Pixel 9").await;
    assert_eq!(expired.0, StatusCode::UNAUTHORIZED);
    drop(fixture);
}

#[tokio::test]
async fn channels_are_not_interchangeable() {
    let (fixture, _db, app) = bootstrapped().await;
    let (cookie, csrf) = web_login(&app).await;
    let session_token = cookie
        .split_once('=')
        .unwrap()
        .1
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let code = issue_pairing_code(&app, &cookie, &csrf).await;
    let enrollment = enroll_with_code(&app, &code, "Pixel 9").await;
    let device_token = enrollment.1["token"].as_str().unwrap().to_owned();

    // 设备令牌放进 Cookie：查会话表无果。
    let as_cookie = app
        .clone()
        .oneshot(
            Request::get("/api/v1/media-items")
                .header(
                    header::COOKIE,
                    format!("__Host-mediaflow_session={device_token}"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(as_cookie.status(), StatusCode::UNAUTHORIZED);

    // 会话令牌放进 Authorization：查设备表无果。
    let as_bearer = app
        .clone()
        .oneshot(
            Request::get("/api/v1/media-items")
                .header(header::AUTHORIZATION, format!("Bearer {session_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(as_bearer.status(), StatusCode::UNAUTHORIZED);
    drop(fixture);
}

#[tokio::test]
async fn bearer_exemption_does_not_swallow_csrf_for_cookie_channel() {
    let (fixture, _db, app) = bootstrapped().await;
    let (cookie, _csrf) = web_login(&app).await;
    // Cookie 会话 + Authorization 头并存：同源证明按 Bearer 豁免跳过，但 CSRF 仍强制。
    let mixed = app
        .clone()
        .oneshot(
            Request::delete("/api/v1/session")
                .header(header::COOKIE, &cookie)
                .header(header::AUTHORIZATION, "Bearer mfdev_garbage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mixed.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(mixed).await["error"]["code"], "csrf.invalid");
    drop(fixture);
}

#[tokio::test]
async fn device_token_rotation_invalidates_old_token_immediately() {
    let (fixture, _db, app) = bootstrapped().await;
    let device_token = enrolled_device(&app).await;
    let rotated = app
        .clone()
        .oneshot(
            Request::post("/api/v1/devices/current/token-rotation")
                .header(header::AUTHORIZATION, format!("Bearer {device_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rotated.status(), StatusCode::OK);
    let new_token = json_body(rotated).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(new_token, device_token);
    assert!(new_token.starts_with("mfdev_"));

    let old = app
        .clone()
        .oneshot(bearer_get(&device_token, "/api/v1/media-items"))
        .await
        .unwrap();
    assert_eq!(old.status(), StatusCode::UNAUTHORIZED);
    let fresh = app
        .oneshot(bearer_get(&new_token, "/api/v1/media-items"))
        .await
        .unwrap();
    assert_eq!(fresh.status(), StatusCode::OK);
    drop(fixture);
}

#[tokio::test]
async fn device_revocation_by_admin_and_self_logout_are_enforced() {
    let (fixture, db, app) = bootstrapped().await;
    let (cookie, csrf) = web_login(&app).await;
    let first_token = enrolled_device(&app).await;
    let second_token = enrolled_device(&app).await;
    let device_id: Vec<u8> =
        sqlx::query_scalar("SELECT id FROM identity_devices ORDER BY created_at_us LIMIT 1")
            .fetch_one(db.pool())
            .await
            .unwrap();
    let first_id = uuid::Uuid::from_slice(&device_id).unwrap();

    // 设备不能吊销其他设备。
    let cross = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/v1/devices/{first_id}"))
                .header(header::AUTHORIZATION, format!("Bearer {second_token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cross.status(), StatusCode::FORBIDDEN);

    // 管理面列表（仅浏览器会话通道）。
    let listed = app
        .clone()
        .oneshot(
            Request::get("/api/v1/devices")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let items = json_body(listed).await["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 2);

    // 管理员吊销第一台。
    let revoked = app
        .clone()
        .oneshot(
            Request::delete(format!("/api/v1/devices/{first_id}"))
                .header(header::COOKIE, &cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3000")
                .header("x-csrf-token", &csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.clone()
            .oneshot(bearer_get(&first_token, "/api/v1/media-items"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // 设备自删（移动端登出）。
    let second_id: Vec<u8> = sqlx::query_scalar(
        "SELECT id FROM identity_devices ORDER BY created_at_us LIMIT 1 OFFSET 1",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    let self_delete = app
        .clone()
        .oneshot(
            Request::delete(format!(
                "/api/v1/devices/{}",
                uuid::Uuid::from_slice(&second_id).unwrap()
            ))
            .header(header::AUTHORIZATION, format!("Bearer {second_token}"))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(self_delete.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.clone()
            .oneshot(bearer_get(&second_token, "/api/v1/media-items"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    drop(fixture);
}

#[tokio::test]
async fn management_face_rejects_device_tokens() {
    let (fixture, _db, app) = bootstrapped().await;
    let device_token = enrolled_device(&app).await;
    for (method, path) in [
        ("POST", "/api/v1/device-pairing-codes"),
        ("GET", "/api/v1/devices"),
        ("GET", "/api/v1/family-members"),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {device_token}"))
            .body(Body::empty())
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "device token must not reach management face {path}"
        );
    }
    // 会话端点同样拒绝设备主体（登出走设备端点）。
    assert_eq!(
        app.clone()
            .oneshot(bearer_get(&device_token, "/api/v1/session"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    drop(fixture);
}

#[tokio::test]
async fn device_tokens_expire_and_follow_credential_version() {
    let (fixture, db, app) = bootstrapped().await;
    let device_token = enrolled_device(&app).await;
    sqlx::query("UPDATE identity_devices SET absolute_expires_at_us=1")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(bearer_get(&device_token, "/api/v1/media-items"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let fresh_token = enrolled_device(&app).await;
    sqlx::query("UPDATE identity_accounts SET credential_version = credential_version + 1")
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(bearer_get(&fresh_token, "/api/v1/media-items"))
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    drop(fixture);
}

async fn bootstrapped() -> (
    TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
) {
    let fixture = TestConfigDir::new(RunMode::Development);
    let secret_path = fixture.config().config_dir.join("bootstrap.secret");
    std::fs::write(&secret_path, "device-auth-bootstrap-secret").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/system/bootstrap")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "http://127.0.0.1:3000")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(
                    json!({
                        "bootstrap_secret": "device-auth-bootstrap-secret",
                        "administrator_name": "admin",
                        "password": ADMIN_PASSWORD
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    (fixture, db, app)
}

async fn web_login(app: &axum::Router) -> (String, String) {
    let login = app
        .clone()
        .oneshot(
            Request::post("/api/v1/sessions")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "http://127.0.0.1:3000")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(
                    json!({"administrator_name": "admin", "password": ADMIN_PASSWORD}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::CREATED);
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
    (cookie, csrf)
}

async fn issue_pairing_code(app: &axum::Router, cookie: &str, csrf: &str) -> String {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/device-pairing-codes")
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, "http://127.0.0.1:3000")
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = json_body(response).await;
    let code = body["pairing_code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 8);
    code
}

async fn enroll_with_code(
    app: &axum::Router,
    code: &str,
    device_name: &str,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/v1/devices/enrollment")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"pairing_code": code, "device_name": device_name}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    (status, json_body(response).await)
}

async fn enrolled_device(app: &axum::Router) -> String {
    let (cookie, csrf) = web_login(app).await;
    let code = issue_pairing_code(app, &cookie, &csrf).await;
    let (status, body) = enroll_with_code(app, &code, "Pixel 9").await;
    assert_eq!(status, StatusCode::CREATED);
    body["token"].as_str().unwrap().to_owned()
}

fn bearer_get(token: &str, path: &str) -> Request<axum::body::Body> {
    Request::get(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

#[allow(clippy::needless_pass_by_value)]
async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
