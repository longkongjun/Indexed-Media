mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt;

const SECRET: &str = "task-three-bootstrap-secret";

#[tokio::test]
async fn bootstrap_status_is_minimal_and_concurrent_bootstrap_has_one_winner() {
    let fixture = identity_fixture();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));

    let status = app
        .clone()
        .oneshot(
            Request::get("/api/v1/system/bootstrap-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let status_body = json_body(status).await;
    assert_eq!(
        status_body,
        json!({"requires_initialization": true, "version": "v1"})
    );

    let first = app.clone().oneshot(bootstrap_request(
        SECRET,
        " Admin ",
        "correct horse battery staple",
    ));
    let second = app.clone().oneshot(bootstrap_request(
        SECRET,
        "admin",
        "another correct password",
    ));
    let (first, second) = tokio::join!(first, second);
    let mut statuses = [first.unwrap().status(), second.unwrap().status()];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);

    let account_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM identity_accounts")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(account_count, 1);

    let original_display_name: String =
        sqlx::query_scalar("SELECT display_name FROM identity_accounts")
            .fetch_one(db.pool())
            .await
            .unwrap();
    assert!(matches!(original_display_name.as_str(), "Admin" | "admin"));

    let again = app
        .oneshot(bootstrap_request(
            SECRET,
            "replacement",
            "replacement password",
        ))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::CONFLICT);
    assert_eq!(
        json_body(again).await["error"]["code"],
        "bootstrap.already_completed"
    );
    let display_name: String = sqlx::query_scalar("SELECT display_name FROM identity_accounts")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(display_name, original_display_name);
    assert!(
        !fixture
            .config()
            .config_dir
            .join("bootstrap.secret")
            .exists(),
        "successful bootstrap should remove a deletable secret file"
    );
}

#[tokio::test]
async fn invalid_bootstrap_secret_is_stable_and_never_persisted() {
    let fixture = identity_fixture();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));

    let response = app
        .oneshot(bootstrap_request(
            "wrong-secret",
            "admin",
            "correct horse battery staple",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = json_body(response).await;
    assert_eq!(body["error"]["code"], "bootstrap.invalid_secret");
    let database = std::fs::read(db.path()).unwrap();
    assert!(
        !database
            .windows(SECRET.len())
            .any(|window| window == SECRET.as_bytes())
    );
    assert!(!body.to_string().contains("wrong-secret"));
}

#[tokio::test]
async fn empty_bootstrap_secret_file_is_rejected_without_creating_an_account() {
    let fixture = identity_fixture();
    std::fs::write(fixture.config().config_dir.join("bootstrap.secret"), "\n").unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let response = app
        .oneshot(bootstrap_request(
            "",
            "admin",
            "correct horse battery staple",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json_body(response).await["error"]["code"], "internal.error");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identity_accounts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bootstrap_secret_file_must_not_be_accessible_to_group_or_others() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = identity_fixture();
    let secret_path = fixture.config().config_dir.join("bootstrap.secret");
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let denied = app
        .clone()
        .oneshot(bootstrap_request(
            SECRET,
            "admin",
            "correct horse battery staple",
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(json_body(denied).await["error"]["code"], "internal.error");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM identity_accounts")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );

    std::fs::set_permissions(secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let accepted = app
        .oneshot(bootstrap_request(
            SECRET,
            "admin",
            "correct horse battery staple",
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::CREATED);
}

fn identity_fixture() -> TestConfigDir {
    let fixture = TestConfigDir::new(RunMode::Development);
    let path = fixture.config().config_dir.join("bootstrap.secret");
    std::fs::write(&path, SECRET).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fixture
}

fn bootstrap_request(secret: &str, name: &str, password: &str) -> Request<Body> {
    Request::post("/api/v1/system/bootstrap")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(
            json!({
                "bootstrap_secret": secret,
                "administrator_name": name,
                "password": password
            })
            .to_string(),
        ))
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
