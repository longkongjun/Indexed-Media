#![allow(clippy::needless_pass_by_value)]

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::capability::CapabilityFs;
use mediaflow_core::discovery::model::{
    DeploymentRootView, DirectoryCapability, DirectoryEntry, DirectoryIdentity, EntryMetadata,
    FsBoundaryError, RelativePath, RootAccess, RootId,
};
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::tasks::routes::router as scan_router;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "task-five-bootstrap-secret";

#[tokio::test]
async fn scan_api_guards_run_before_path_query_and_idempotency_validation() {
    let (_fixture, _db, app, cookie, csrf, _account, inbox) = authenticated_app().await;
    for request in [
        Request::post("/api/v1/inbox-directories/not-a-uuid/scan-tasks")
            .body(Body::empty())
            .unwrap(),
        Request::get("/api/v1/scan-tasks?cursor=%ZZ")
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "session.expired"
        );
    }
    let wrong_origin = Request::post(format!("/api/v1/inbox-directories/{inbox}/scan-tasks"))
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, "https://evil.test")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(wrong_origin).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "origin.untrusted"
    );
    let bad_csrf = Request::post(format!("/api/v1/inbox-directories/{inbox}/scan-tasks"))
        .header(header::COOKIE, &cookie)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", "bad")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(bad_csrf).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(response).await["error"]["code"], "csrf.invalid");
    for request in [
        scan_post(
            "/api/v1/inbox-directories/not-a-uuid/scan-tasks",
            &cookie,
            &csrf,
            "valid-key",
        ),
        scan_post(
            &format!("/api/v1/inbox-directories/{inbox}/scan-tasks"),
            &cookie,
            &csrf,
            "",
        ),
        get("/api/v1/scan-tasks?cursor=%ZZ", &cookie),
        get("/api/v1/scan-tasks?limit=201", &cookie),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "validation.failed"
        );
    }
}

#[tokio::test]
async fn authenticated_create_is_idempotent_and_list_detail_follow_contract() {
    let (_fixture, db, app, cookie, csrf, _account, inbox) = authenticated_app().await;
    let path = format!("/api/v1/inbox-directories/{inbox}/scan-tasks");
    let first = app
        .clone()
        .oneshot(scan_post(&path, &cookie, &csrf, "scan-create-key"));
    let duplicate = app
        .clone()
        .oneshot(scan_post(&path, &cookie, &csrf, "scan-create-key"));
    let (first, duplicate) = tokio::join!(first, duplicate);
    let first = first.unwrap();
    let duplicate = duplicate.unwrap();
    assert_eq!(first.status(), StatusCode::ACCEPTED);
    assert_eq!(duplicate.status(), StatusCode::ACCEPTED);
    let first = json_body(first).await;
    let task_id = first["id"].as_str().unwrap();
    assert_eq!(first["status"], "queued");
    assert_eq!(json_body(duplicate).await["id"], task_id);
    let list = app
        .clone()
        .oneshot(get("/api/v1/scan-tasks?limit=200", &cookie))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(json_body(list).await["items"][0]["id"], task_id);
    let detail = app
        .clone()
        .oneshot(get(&format!("/api/v1/scan-tasks/{task_id}"), &cookie))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json_body(detail).await["id"], task_id);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tasks_scan_tasks")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn create_transaction_failure_rolls_back_every_fact_before_filesystem_access() {
    let (fixture, db, _app, cookie, csrf, _account, inbox) = authenticated_app().await;
    sqlx::query(
        "CREATE TRIGGER fail_scan_outbox BEFORE INSERT ON platform_outbox_events
         WHEN NEW.event_type='task.state-changed'
         BEGIN SELECT RAISE(FAIL, 'forced scan transaction failure'); END",
    )
    .execute(db.pool())
    .await
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let root_id = RootId::parse("incoming").unwrap();
    let discovery = DiscoveryService::new(
        BTreeMap::from([(
            root_id.clone(),
            DeploymentRootView {
                id: root_id,
                label: "Incoming".to_owned(),
                access: RootAccess::ReadOnly,
            },
        )]),
        Arc::new(CountingFs {
            calls: Arc::clone(&calls),
        }),
        db.pool().clone(),
    );
    let guarded = scan_router(fixture.config().clone(), &db, discovery);
    let response = guarded
        .oneshot(scan_post(
            &format!("/api/v1/inbox-directories/{inbox}/scan-tasks"),
            &cookie,
            &csrf,
            "forced-failure",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    for table in [
        "tasks_scan_tasks",
        "discovery_scan_batches",
        "tasks_scan_attempts",
        "tasks_idempotency_keys",
        "platform_outbox_events",
    ] {
        let count = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

struct CountingFs {
    calls: Arc<AtomicUsize>,
}

impl CapabilityFs for CountingFs {
    fn preflight_directory(
        &self,
        _root: &RootId,
        _relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(FsBoundaryError::Unavailable)
    }

    fn read_directory(
        &self,
        _capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(FsBoundaryError::Unavailable)
    }

    fn metadata_no_follow(
        &self,
        _capability: &DirectoryCapability,
        _name: &std::ffi::OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(FsBoundaryError::Unavailable)
    }
}

async fn authenticated_app() -> (
    Box<common::TestConfigDir>,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    String,
    Uuid,
    Uuid,
) {
    let fixture = Box::new(common::TestConfigDir::new(RunMode::Development));
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
    let app = build_router(fixture.config().clone(), Some(db.clone()));
    let bootstrap = app
        .clone()
        .oneshot(public_post(
            "/api/v1/system/bootstrap",
            json!({"bootstrap_secret":SECRET,"administrator_name":"admin","password":"correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(bootstrap.status(), StatusCode::CREATED);
    let account = Uuid::parse_str(
        json_body(bootstrap).await["account"]["id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let login = app
        .clone()
        .oneshot(public_post(
            "/api/v1/sessions",
            json!({"administrator_name":"admin","password":"correct horse battery staple"}),
        ))
        .await
        .unwrap();
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    let csrf = json_body(login).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    let inbox = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,'incoming',X'2E','.',X'01',X'02','available',1,1,1,1)",
    )
    .bind(inbox.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    (fixture, db, app, cookie, csrf, account, inbox)
}

fn public_post(path: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn scan_post(path: &str, cookie: &str, csrf: &str, key: &str) -> Request<Body> {
    Request::post(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", csrf)
        .header("idempotency-key", key)
        .body(Body::empty())
        .unwrap()
}

fn get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
