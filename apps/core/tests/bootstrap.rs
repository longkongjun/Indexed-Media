mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::Value;
use tower::ServiceExt;

#[test]
fn cli_exposes_the_five_task_two_subcommands() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mediaflow-core"))
        .arg("--help")
        .output()
        .expect("CLI help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
    for command in [
        "serve",
        "healthcheck",
        "verify-database",
        "restore-backup",
        "argon2-calibrate",
    ] {
        assert!(stdout.contains(command), "missing {command} subcommand");
    }
}

#[test]
fn verify_database_cli_rejects_corruption_without_leaking_sqlite_details() {
    let root = tempfile::tempdir().expect("CLI fixture");
    let database = root.path().join("corrupt.db");
    std::fs::write(&database, b"not a sqlite database").expect("corrupt database");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mediaflow-core"))
        .args(["verify-database", "--database"])
        .arg(database)
        .output()
        .expect("verify-database command");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
    assert!(stderr.contains("database could not be verified"));
    assert!(!stderr.contains("SQLITE_"));
    assert!(!stderr.contains("file is not a database"));
}

#[test]
fn serve_and_healthcheck_cli_have_real_exit_semantics() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("reserve port");
    let listen = listener.local_addr().expect("listen address");
    drop(listener);

    let mut serve = configured_cli(&fixture, listen);
    serve
        .arg("serve")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let child = serve.spawn().expect("start Core server");
    let mut child = ChildGuard(Some(child));

    let mut ready = false;
    for _ in 0..100 {
        if let Some(status) = child.0.as_mut().unwrap().try_wait().expect("server status") {
            panic!("Core server exited before readiness with {status}");
        }
        let output = configured_cli(&fixture, listen)
            .arg("healthcheck")
            .output()
            .expect("healthcheck command");
        if output.status.success() {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    assert!(ready, "healthcheck should return zero after Core is ready");
    child.0.as_mut().unwrap().kill().expect("stop Core server");
    child.0.as_mut().unwrap().wait().expect("reap Core server");
    child.0 = None;
}

#[test]
fn invalid_deployment_roots_fail_before_database_migration_or_backup() {
    let fixture = TestConfigDir::new(RunMode::Development);
    std::fs::write(
        &fixture.config().deployment_roots_file,
        b"{\"roots\":[{\"id\":\"incoming\",\"label\":\"Incoming\",\"container_path\":\"relative\",\"access\":\"read-only\"}]}",
    )
    .unwrap();

    let output = configured_cli(&fixture, "127.0.0.1:0".parse().unwrap())
        .arg("serve")
        .output()
        .expect("serve with invalid deployment roots");

    assert!(!output.status.success());
    assert!(!fixture.database_path().exists());
    assert!(fixture.backup_files().is_empty());
    assert!(fixture.manifest_files().is_empty());
}

#[test]
fn argon2_calibrate_reports_real_machine_readable_security_parameters() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mediaflow-core"))
        .arg("argon2-calibrate")
        .output()
        .expect("argon2-calibrate command");

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let body: Value = serde_json::from_slice(&output.stdout).expect("calibration JSON");
    assert_eq!(body["algorithm"], "argon2id");
    assert_eq!(body["operation"], "verify");
    assert_eq!(body["memory_kib"], 65_536);
    assert_eq!(body["parallelism"], 1);
    assert!(body["suggested_time_cost"].as_u64().unwrap() >= 2);
    assert!(body["measured_ms"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn development_can_be_ready_without_static_distribution() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("development database");
    let app = build_router(fixture.config().clone(), Some(db));

    let response = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn closed_database_pool_drops_readiness_but_keeps_liveness() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("development database");
    db.pool().close().await;
    let app = build_router(fixture.config().clone(), Some(db));

    let live = app
        .clone()
        .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let ready = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn non_http_public_origin_never_opens_business_readiness() {
    let mut fixture = TestConfigDir::new(RunMode::Development);
    fixture.config_mut().public_origin = url::Url::parse("ftp://mediaflow.example.test").unwrap();
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("development database");
    let app = build_router(fixture.config().clone(), Some(db));

    let response = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn production_non_https_origin_is_live_but_not_ready_and_closes_business_routes() {
    let mut fixture = TestConfigDir::new(RunMode::Production);
    fixture.create_web_dist();
    fixture.config_mut().public_origin = url::Url::parse("http://mediaflow.example.test").unwrap();
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("production database");
    let app = build_router(fixture.config().clone(), Some(db));

    let live = app
        .clone()
        .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let ready = app
        .clone()
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let business = app
        .oneshot(
            Request::get("/api/v1/system/bootstrap-status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(live.status(), StatusCode::OK);
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(business.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_json_error(business, "internal.error").await;
}

#[tokio::test]
async fn production_missing_web_distribution_is_not_ready() {
    let mut fixture = TestConfigDir::new(RunMode::Production);
    fixture.config_mut().public_origin = url::Url::parse("https://mediaflow.example.test").unwrap();
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("production database");
    let app = build_router(fixture.config().clone(), Some(db));

    let response = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn production_requires_an_explicit_trusted_proxy_boundary() {
    let mut fixture = TestConfigDir::new(RunMode::Production);
    fixture.create_web_dist();
    fixture.config_mut().public_origin = url::Url::parse("https://mediaflow.example.test").unwrap();
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("production database");
    let app = build_router(fixture.config().clone(), Some(db));

    let response = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn production_is_ready_with_https_web_dist_and_a_scoped_trusted_proxy() {
    let mut fixture = TestConfigDir::new(RunMode::Production);
    fixture.create_web_dist();
    fixture.config_mut().public_origin = url::Url::parse("https://mediaflow.example.test").unwrap();
    fixture.config_mut().trusted_proxy_cidrs = vec!["127.0.0.1/32".parse().unwrap()];
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("production database");
    let app = build_router(fixture.config().clone(), Some(db));

    let response = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn production_rejects_an_unbounded_trusted_proxy_cidr() {
    let mut fixture = TestConfigDir::new(RunMode::Production);
    fixture.create_web_dist();
    fixture.config_mut().public_origin = url::Url::parse("https://mediaflow.example.test").unwrap();
    fixture.config_mut().trusted_proxy_cidrs = vec!["0.0.0.0/0".parse().unwrap()];
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("production database");
    let app = build_router(fixture.config().clone(), Some(db));

    let response = app
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn api_not_found_is_json_and_never_falls_back_to_spa_index() {
    let fixture = TestConfigDir::new(RunMode::Development);
    fixture.create_web_dist();
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("development database");
    let app = build_router(fixture.config().clone(), Some(db));

    let response = app
        .oneshot(
            Request::get("/api/v1/not-a-real-route")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("MediaFlow test SPA"));
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "internal.error");
}

#[tokio::test]
async fn static_fallback_serves_assets_and_spa_index_only_outside_api() {
    let fixture = TestConfigDir::new(RunMode::Development);
    fixture.create_web_dist();
    let db = migrate_with_backup(fixture.config())
        .await
        .expect("development database");
    let app = build_router(fixture.config().clone(), Some(db));

    let asset = app
        .clone()
        .oneshot(Request::get("/app.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let spa = app
        .oneshot(
            Request::get("/scan-tasks/example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(
        asset.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/javascript; charset=utf-8"
    );
    let spa_bytes = to_bytes(spa.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&spa_bytes).contains("MediaFlow test SPA"));
}

async fn assert_json_error(response: axum::response::Response, code: &str) {
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], code);
    assert!(body["error"]["request_id"].as_str().is_some());
}

fn configured_cli(fixture: &TestConfigDir, listen: std::net::SocketAddr) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_mediaflow-core"));
    command
        .env("MEDIAFLOW_MODE", "development")
        .env("MEDIAFLOW_LISTEN", listen.to_string())
        .env("MEDIAFLOW_CONFIG_DIR", &fixture.config().config_dir)
        .env("MEDIAFLOW_PUBLIC_ORIGIN", format!("http://{listen}"))
        .env(
            "MEDIAFLOW_DEPLOYMENT_ROOTS_FILE",
            &fixture.config().deployment_roots_file,
        )
        .env("MEDIAFLOW_WEB_DIST", &fixture.config().web_dist)
        .env("MEDIAFLOW_TRUSTED_PROXY_CIDRS", "");
    command
}

struct ChildGuard(Option<std::process::Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
