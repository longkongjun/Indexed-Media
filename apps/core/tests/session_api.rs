mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt;

use argon2::password_hash::PasswordHash;
use mediaflow_core::identity::IdentityUseCases;
use mediaflow_core::identity::model::LoginCommand;
use mediaflow_core::identity::service::IdentityService;
use mediaflow_core::platform::password::{MIN_TIME_COST, PasswordEngine};

#[tokio::test]
async fn login_is_indistinguishable_and_tokens_are_only_stored_as_hashes() {
    let (fixture, db, app) = bootstrapped().await;
    let unknown_started = std::time::Instant::now();
    let unknown = app
        .clone()
        .oneshot(login_request("unknown", "wrong password value"))
        .await
        .unwrap();
    let unknown_elapsed = unknown_started.elapsed();
    let wrong_started = std::time::Instant::now();
    let wrong = app
        .clone()
        .oneshot(login_request("admin", "wrong password value"))
        .await
        .unwrap();
    let wrong_elapsed = wrong_started.elapsed();
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    let mut unknown_body = json_body(unknown).await;
    let mut wrong_body = json_body(wrong).await;
    unknown_body["error"]
        .as_object_mut()
        .unwrap()
        .remove("request_id");
    wrong_body["error"]
        .as_object_mut()
        .unwrap()
        .remove("request_id");
    assert_eq!(unknown_body, wrong_body);
    assert!(unknown_elapsed >= std::time::Duration::from_millis(50));
    assert!(wrong_elapsed >= std::time::Duration::from_millis(50));

    let response = app
        .clone()
        .oneshot(login_request(" ADMIN ", "correct horse battery staple"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(cookie.starts_with("__Host-mediaflow_session="));
    assert!(cookie.contains("; Secure"));
    assert!(cookie.contains("; HttpOnly"));
    assert!(cookie.contains("; SameSite=Strict"));
    assert!(cookie.contains("; Path=/"));
    assert!(!cookie.contains("Domain="));
    let raw_session = cookie.split_once('=').unwrap().1.split(';').next().unwrap();
    let body = json_body(response).await;
    let csrf = body["csrf_token"].as_str().unwrap();
    assert_ne!(raw_session, csrf);
    assert!(raw_session.len() >= 43);
    assert!(csrf.len() >= 43);
    let stored: (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT token_sha256, csrf_sha256 FROM identity_sessions")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert_eq!(stored.0.len(), 32);
    assert_eq!(stored.1.len(), 32);
    assert_ne!(stored.0, raw_session.as_bytes());
    assert_ne!(stored.1, csrf.as_bytes());

    drop(fixture);
}

#[tokio::test]
async fn cold_start_unknown_and_known_wrong_credentials_each_run_exactly_one_verify() {
    let (fixture, db, _app) = bootstrapped().await;
    let (passwords, operations) = PasswordEngine::new_counted(2, MIN_TIME_COST);
    let service = IdentityService::new_with_password_engine(
        db.pool().clone(),
        fixture.config().config_dir.clone(),
        passwords,
    );

    let unknown = service
        .create_session(LoginCommand {
            administrator_name: "unknown".to_owned(),
            password: "wrong password value".to_owned(),
            source_key: "unknown-source".to_owned(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        unknown.code(),
        mediaflow_core::shared::error::ErrorCode::InvalidCredentials
    );
    assert_eq!(operations.hashes(), 0);
    assert_eq!(operations.verifications(), 1);

    let wrong = service
        .create_session(LoginCommand {
            administrator_name: "admin".to_owned(),
            password: "wrong password value".to_owned(),
            source_key: "known-source".to_owned(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        wrong.code(),
        mediaflow_core::shared::error::ErrorCode::InvalidCredentials
    );
    assert_eq!(operations.hashes(), 0);
    assert_eq!(operations.verifications(), 2);
}

#[tokio::test]
async fn get_session_rotates_csrf_and_old_token_cannot_mutate() {
    let (_fixture, _db, app) = bootstrapped().await;
    let login = app
        .clone()
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let old_csrf = json_body(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let current = app
        .clone()
        .oneshot(
            Request::get("/api/v1/session")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let new_csrf = json_body(current).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(new_csrf, old_csrf);
    let stale = app
        .clone()
        .oneshot(mutation_request(
            "DELETE",
            "/api/v1/session",
            &cookie,
            &old_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(stale).await["error"]["code"], "csrf.invalid");
    let fresh = app
        .oneshot(mutation_request(
            "DELETE",
            "/api/v1/session",
            &cookie,
            &new_csrf,
        ))
        .await
        .unwrap();
    assert_eq!(fresh.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn session_get_logout_and_credential_version_invalidation_are_enforced() {
    let (_fixture, db, app) = bootstrapped().await;
    let login = app
        .clone()
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let _body = json_body(login).await;

    let current = app
        .clone()
        .oneshot(
            Request::get("/api/v1/session")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(current.status(), StatusCode::OK);
    let refreshed_csrf = json_body(current).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        refreshed_csrf.len() >= 43,
        "GET session must issue a usable CSRF synchronizer token"
    );

    sqlx::query("UPDATE identity_accounts SET credential_version = credential_version + 1")
        .execute(db.pool())
        .await
        .unwrap();
    let expired = app
        .clone()
        .oneshot(
            Request::get("/api/v1/session")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(expired).await["error"]["code"], "session.expired");

    let relogin = app
        .clone()
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = relogin
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let csrf = json_body(relogin).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let logout = app
        .clone()
        .oneshot(mutation_request(
            "DELETE",
            "/api/v1/session",
            &cookie,
            &csrf,
        ))
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let expired_cookie = logout
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(expired_cookie.starts_with("__Host-mediaflow_session=; Max-Age=0;"));
    assert!(expired_cookie.contains("Secure; HttpOnly; SameSite=Strict; Path=/"));
    let reused = app
        .oneshot(
            Request::get("/api/v1/session")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(reused).await["error"]["code"], "session.expired");
}

#[tokio::test]
async fn idle_and_absolute_expiry_are_independently_fail_closed() {
    let (_fixture, db, app) = bootstrapped().await;
    let login = app
        .clone()
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    sqlx::query("UPDATE identity_sessions SET idle_expires_at_us=0,absolute_expires_at_us=9223372036854775807")
        .execute(db.pool()).await.unwrap();
    let idle = app
        .clone()
        .oneshot(
            Request::get("/api/v1/session")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(idle.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(json_body(idle).await["error"]["code"], "session.expired");

    let login = app
        .clone()
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    sqlx::query("UPDATE identity_sessions SET idle_expires_at_us=9223372036854775807,absolute_expires_at_us=0 WHERE revoked_at_us IS NULL")
        .execute(db.pool()).await.unwrap();
    let absolute = app
        .oneshot(
            Request::get("/api/v1/session")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(absolute.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_body(absolute).await["error"]["code"],
        "session.expired"
    );
}

#[tokio::test]
async fn throttle_persists_across_router_restart_and_success_clears_it() {
    let (fixture, db, app) = bootstrapped().await;
    for attempt in 1..=5 {
        let response = app
            .clone()
            .oneshot(login_request("admin", "definitely wrong password"))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if attempt < 5 {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
    }
    let restarted = build_router(fixture.config().clone(), Some(db.clone()));
    let limited = restarted
        .clone()
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        json_body(limited).await["error"]["details"]["retry_after_seconds"]
            .as_i64()
            .unwrap()
            > 0
    );
    let rate_limited_audits: Vec<(Option<String>, String)> = sqlx::query_as(
        "SELECT subject_id,safe_details_json FROM platform_audit_events WHERE action='login' AND outcome='rate_limited'",
    )
    .fetch_all(db.pool())
    .await
    .unwrap();
    assert_eq!(
        rate_limited_audits,
        vec![(None, "{}".to_owned()), (None, "{}".to_owned())],
        "both the threshold-triggering and already-active 429 responses must be audited"
    );

    sqlx::query("UPDATE identity_login_throttles SET retry_after_us=0,window_started_at_us=0")
        .execute(db.pool())
        .await
        .unwrap();
    let recovered = restarted
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::CREATED);
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM identity_login_throttles")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn argon_parameters_and_non_sliding_authentication_boundary_are_explicit() {
    let (fixture, db, app) = bootstrapped().await;
    let phc: String = sqlx::query_scalar("SELECT password_phc FROM identity_accounts")
        .fetch_one(db.pool())
        .await
        .unwrap();
    let parsed = PasswordHash::new(&phc).unwrap();
    assert_eq!(parsed.algorithm.as_str(), "argon2id");
    assert_eq!(parsed.params.get("m").unwrap().decimal().unwrap(), 65_536);
    assert_eq!(parsed.params.get("p").unwrap().decimal().unwrap(), 1);
    assert!(parsed.params.get("t").unwrap().decimal().unwrap() >= 2);

    let login = app
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let token = cookie.split_once('=').unwrap().1.split(';').next().unwrap();
    let before: i64 = sqlx::query_scalar("SELECT idle_expires_at_us FROM identity_sessions WHERE revoked_at_us IS NULL ORDER BY created_at_us DESC LIMIT 1").fetch_one(db.pool()).await.unwrap();
    let service = IdentityService::new(db.pool().clone(), fixture.config().config_dir.clone());
    service.authenticate_without_sliding(token).await.unwrap();
    let after: i64 = sqlx::query_scalar("SELECT idle_expires_at_us FROM identity_sessions WHERE revoked_at_us IS NULL ORDER BY created_at_us DESC LIMIT 1").fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        after, before,
        "stream authentication must not slide idle expiry"
    );
}

#[tokio::test]
async fn sliding_authentication_linearizes_after_concurrent_credential_revocation() {
    let (fixture, db, app) = bootstrapped().await;
    let login = app
        .oneshot(login_request("admin", "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    let raw_token = cookie
        .split_once('=')
        .unwrap()
        .1
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let service = IdentityService::new(db.pool().clone(), fixture.config().config_dir.clone());

    let mut writer = db.pool().acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *writer)
        .await
        .unwrap();
    let authentication = tokio::spawn(async move { service.authenticate(&raw_token).await });
    for _ in 0..100 {
        tokio::task::yield_now().await;
        if authentication.is_finished() {
            break;
        }
    }
    assert!(
        !authentication.is_finished(),
        "database write lock must hold authentication at its linearization point"
    );
    sqlx::query("UPDATE identity_accounts SET credential_version=credential_version+1")
        .execute(&mut *writer)
        .await
        .unwrap();
    sqlx::query("COMMIT").execute(&mut *writer).await.unwrap();

    let error = authentication.await.unwrap().unwrap_err();
    assert_eq!(
        error.code(),
        mediaflow_core::shared::error::ErrorCode::SessionExpired
    );
}

async fn bootstrapped() -> (
    TestConfigDir,
    mediaflow_core::platform::db::Db,
    axum::Router,
) {
    let fixture = TestConfigDir::new(RunMode::Development);
    let secret_path = fixture.config().config_dir.join("bootstrap.secret");
    std::fs::write(&secret_path, "task-three-bootstrap-secret").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let response = app.clone().oneshot(json_request("/api/v1/system/bootstrap", json!({
        "bootstrap_secret": "task-three-bootstrap-secret", "administrator_name": "admin", "password": "correct horse battery staple"
    }))).await.unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    (fixture, db, app)
}

fn login_request(name: &str, password: &str) -> Request<Body> {
    json_request(
        "/api/v1/sessions",
        json!({"administrator_name": name, "password": password}),
    )
}
#[allow(clippy::needless_pass_by_value)]
fn json_request(path: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body.to_string()))
        .unwrap()
}
fn mutation_request(method: &str, path: &str, cookie: &str, csrf: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", csrf)
        .body(Body::empty())
        .unwrap()
}
async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
