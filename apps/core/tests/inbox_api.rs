#![allow(clippy::needless_pass_by_value)]

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::capability::CapabilityFs;
use mediaflow_core::discovery::model::{
    DeploymentRootView, DirectoryCapability, DirectoryEntry, DirectoryIdentity, EntryMetadata,
    FsBoundaryError, RelativePath, RootAccess, RootId,
};
use mediaflow_core::discovery::routes::router as discovery_router;
use mediaflow_core::discovery::service::DiscoveryService;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt;

const SECRET: &str = "task-four-bootstrap-secret";

#[tokio::test]
async fn authenticated_preflight_create_list_detail_and_unknown_id_follow_contract() {
    let (fixture, db, app, cookie, csrf) = authenticated_app().await;
    let roots = app
        .clone()
        .oneshot(get("/api/v1/deployment-roots", &cookie))
        .await
        .unwrap();
    assert_eq!(roots.status(), StatusCode::OK);
    let roots_body = json_body(roots).await;
    assert_eq!(roots_body["items"].as_array().unwrap().len(), 1);
    assert_eq!(roots_body["items"][0]["id"], "incoming");
    assert!(roots_body["items"][0].get("container_path").is_none());

    let preflight = app
        .clone()
        .oneshot(post(
            "/api/v1/inbox-directories/preflight",
            &cookie,
            &csrf,
            json!({"root_id":"incoming","relative_path":"movies//./ready"}),
        ))
        .await
        .unwrap();
    assert_eq!(preflight.status(), StatusCode::OK);
    assert_eq!(
        json_body(preflight).await,
        json!({"root_id":"incoming","relative_path":"movies/ready","readable":true,"overlaps_existing":false})
    );

    let created = app
        .clone()
        .oneshot(post(
            "/api/v1/inbox-directories",
            &cookie,
            &csrf,
            json!({"root_id":"incoming","relative_path":"movies/ready"}),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body = json_body(created).await;
    let id = created_body["id"].as_str().unwrap();
    assert_eq!(uuid::Uuid::parse_str(id).unwrap().get_version_num(), 7);
    assert_eq!(created_body["health"], "available");
    assert_eq!(created_body["relative_path"], "movies/ready");
    assert!(created_body["last_checked_at"].as_str().is_some());
    assert!(
        !created_body
            .to_string()
            .contains(fixture.root_path().to_str().unwrap())
    );

    let list = app
        .clone()
        .oneshot(get("/api/v1/inbox-directories", &cookie))
        .await
        .unwrap();
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(json_body(list).await["items"][0]["id"], id);
    let detail = app
        .clone()
        .oneshot(get(&format!("/api/v1/inbox-directories/{id}"), &cookie))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);
    assert_eq!(json_body(detail).await["id"], id);

    let snapshots: (Vec<u8>, Vec<u8>, i64, i64) = sqlx::query_as(
        "SELECT root_identity, directory_identity, version, last_checked_at_us FROM discovery_inbox_directories WHERE id=?",
    )
    .bind(uuid::Uuid::parse_str(id).unwrap().as_bytes().as_slice())
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!snapshots.0.is_empty());
    assert!(!snapshots.1.is_empty());
    assert_eq!(snapshots.2, 1);
    assert!(snapshots.3 > 0);

    let missing = app
        .oneshot(get(
            "/api/v1/inbox-directories/019f0000-0000-7000-8000-000000000099",
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(json_body(missing).await["error"]["code"], "inbox.not_found");
}

#[tokio::test]
async fn atomic_deployment_root_config_replacement_closes_readiness_and_discovery_routes() {
    let (fixture, _db, app, cookie, csrf) = authenticated_app().await;
    let replacement_root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("replacement-root");
    std::fs::create_dir_all(replacement_root.join("movies/ready")).unwrap();
    let staged = fixture
        .config()
        .deployment_roots_file
        .with_extension("json.next");
    std::fs::write(
        &staged,
        serde_json::to_vec(&json!({"roots":[{
            "id":"replacement",
            "label":"Replacement",
            "container_path":replacement_root,
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    std::fs::rename(&staged, &fixture.config().deployment_roots_file).unwrap();

    let ready = app
        .clone()
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);

    for request in [
        get("/api/v1/deployment-roots", &cookie),
        post(
            "/api/v1/inbox-directories/preflight",
            &cookie,
            &csrf,
            json!({"root_id":"incoming","relative_path":"movies/ready"}),
        ),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = json_body(response).await;
        assert_eq!(body["error"]["code"], "internal.error");
        let text = body.to_string();
        assert!(!text.contains("incoming"));
        assert!(!text.contains("replacement"));
    }
}

#[tokio::test]
async fn readiness_and_session_guards_run_before_query_and_path_validation() {
    let (fixture, _db, app, _cookie, _csrf) = authenticated_app().await;

    for path in [
        "/api/v1/inbox-directories?cursor=%ZZ",
        "/api/v1/inbox-directories/not-a-uuid",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "session.expired"
        );
    }

    let staged = fixture
        .config()
        .deployment_roots_file
        .with_extension("json.invalid");
    std::fs::write(&staged, b"not-json").unwrap();
    std::fs::rename(&staged, &fixture.config().deployment_roots_file).unwrap();
    for path in [
        "/api/v1/inbox-directories?cursor=%ZZ",
        "/api/v1/inbox-directories/not-a-uuid",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json_body(response).await["error"]["code"], "internal.error");
    }
}

#[tokio::test]
async fn authenticated_malformed_inputs_are_422_and_unknown_json_fields_have_no_side_effects() {
    let (_fixture, db, app, cookie, csrf) = authenticated_app().await;
    for path in [
        "/api/v1/inbox-directories?cursor=%ZZ",
        "/api/v1/inbox-directories/not-a-uuid",
        &format!("/api/v1/inbox-directories?cursor={}", "A".repeat(513)),
    ] {
        let response = app.clone().oneshot(get(path, &cookie)).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "validation.failed"
        );
    }

    let response = app
        .oneshot(post(
            "/api/v1/inbox-directories",
            &cookie,
            &csrf,
            json!({"root_id":"incoming","relative_path":"movies/ready","unknown":true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json_body(response).await["error"]["code"],
        "validation.failed"
    );
    assert_eq!(inbox_count(&db).await, 0);
}

#[tokio::test]
async fn concurrent_equal_parent_child_creation_has_at_most_one_winner() {
    for (left, right) in [
        ("movies", "movies/ready"),
        ("shows/ready", "shows"),
        ("music", "music"),
    ] {
        let (fixture, db, app, cookie, csrf) = authenticated_app().await;
        for path in [left, right] {
            std::fs::create_dir_all(fixture.root_path().join(path)).unwrap();
        }
        let first = app.clone().oneshot(post(
            "/api/v1/inbox-directories",
            &cookie,
            &csrf,
            json!({"root_id":"incoming","relative_path":left}),
        ));
        let second = app.clone().oneshot(post(
            "/api/v1/inbox-directories",
            &cookie,
            &csrf,
            json!({"root_id":"incoming","relative_path":right}),
        ));
        let (first, second) = tokio::join!(first, second);
        let statuses = [first.unwrap().status(), second.unwrap().status()];
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CREATED)
                .count(),
            1
        );
        assert_eq!(
            statuses
                .iter()
                .filter(|status| **status == StatusCode::CONFLICT)
                .count(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_inbox_directories")
                .fetch_one(db.pool())
                .await
                .unwrap(),
            1
        );
    }
}

#[tokio::test]
async fn session_origin_csrf_path_and_root_failures_have_no_side_effects_or_path_leaks() {
    let (fixture, db, app, cookie, csrf) = authenticated_app().await;
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let root_id = RootId::parse("incoming").unwrap();
    let service = DiscoveryService::new(
        [(
            root_id.clone(),
            DeploymentRootView {
                id: root_id,
                label: "Incoming".to_owned(),
                access: RootAccess::ReadOnly,
            },
        )]
        .into_iter()
        .collect(),
        std::sync::Arc::new(CountingFs {
            calls: std::sync::Arc::clone(&calls),
        }),
        db.pool().clone(),
    );
    let guarded = discovery_router(fixture.config().clone(), &db, service);
    let cases = [
        post_without_auth(json!({"root_id":"incoming","relative_path":"movies/ready"})),
        post_with_security(
            &cookie,
            "https://evil.test",
            &csrf,
            json!({"root_id":"incoming","relative_path":"movies/ready"}),
        ),
        post_with_security(
            &cookie,
            "http://127.0.0.1:3000",
            "bad-csrf",
            json!({"root_id":"incoming","relative_path":"movies/ready"}),
        ),
    ];
    let expected = [
        (StatusCode::UNAUTHORIZED, "session.expired"),
        (StatusCode::FORBIDDEN, "origin.untrusted"),
        (StatusCode::FORBIDDEN, "csrf.invalid"),
    ];
    for (request, (status, code)) in cases.into_iter().zip(expected) {
        let response = guarded.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(json_body(response).await["error"]["code"], code);
    }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(inbox_count(&db).await, 0);

    for (payload, code) in [
        (
            json!({"root_id":"unknown","relative_path":"movies"}),
            "root.not_found",
        ),
        (
            json!({"root_id":"incoming","relative_path":"/private/outside"}),
            "path.invalid",
        ),
        (
            json!({"root_id":"incoming","relative_path":"../outside"}),
            "path.invalid",
        ),
    ] {
        let response = app
            .clone()
            .oneshot(post("/api/v1/inbox-directories", &cookie, &csrf, payload))
            .await
            .unwrap();
        let text = String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(text.contains(code));
        assert!(!text.contains(fixture.root_path().to_str().unwrap()));
    }
    assert_eq!(inbox_count(&db).await, 0);
    let audit_text: String = sqlx::query_scalar(
        "SELECT COALESCE(group_concat(COALESCE(subject_id,'') || safe_details_json, ''), '') FROM platform_audit_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert!(!audit_text.contains(fixture.root_path().to_str().unwrap()));
}

struct CountingFs {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl CapabilityFs for CountingFs {
    fn preflight_directory(
        &self,
        _root: &RootId,
        _relative: &RelativePath,
    ) -> Result<DirectoryIdentity, FsBoundaryError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(FsBoundaryError::Unavailable)
    }

    fn read_directory(
        &self,
        _capability: &DirectoryCapability,
    ) -> Result<Vec<DirectoryEntry>, FsBoundaryError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(FsBoundaryError::Unavailable)
    }

    fn metadata_no_follow(
        &self,
        _capability: &DirectoryCapability,
        _name: &std::ffi::OsStr,
    ) -> Result<EntryMetadata, FsBoundaryError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(FsBoundaryError::Unavailable)
    }
}

async fn authenticated_app() -> (
    Box<TestConfigDir>,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    String,
) {
    let fixture = Box::new(TestConfigDir::new(RunMode::Development));
    let root = fixture
        .config()
        .config_dir
        .parent()
        .unwrap()
        .join("incoming-root");
    std::fs::create_dir_all(root.join("movies/ready")).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&json!({"roots":[{
            "id":"incoming",
            "label":"Incoming",
            "container_path":root,
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
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
    let login = app
        .clone()
        .oneshot(public_post(
            "/api/v1/sessions",
            json!({"administrator_name":"admin","password":"correct horse battery staple"}),
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
    (fixture, db, app, cookie, csrf)
}

trait RootPath {
    fn root_path(&self) -> std::path::PathBuf;
}

impl RootPath for TestConfigDir {
    fn root_path(&self) -> std::path::PathBuf {
        std::fs::canonicalize(
            self.config()
                .config_dir
                .parent()
                .unwrap()
                .join("incoming-root"),
        )
        .unwrap()
    }
}

fn get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn post(path: &str, cookie: &str, csrf: &str, body: Value) -> Request<Body> {
    post_with_path_and_security(path, cookie, "http://127.0.0.1:3000", csrf, body)
}

fn post_without_auth(body: Value) -> Request<Body> {
    Request::post("/api/v1/inbox-directories")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "https://evil.test")
        .header("sec-fetch-site", "cross-site")
        .header("x-csrf-token", "bad")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn post_with_security(cookie: &str, origin: &str, csrf: &str, body: Value) -> Request<Body> {
    post_with_path_and_security("/api/v1/inbox-directories", cookie, origin, csrf, body)
}

fn post_with_path_and_security(
    path: &str,
    cookie: &str,
    origin: &str,
    csrf: &str,
    body: Value,
) -> Request<Body> {
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", csrf)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn public_post(path: &str, body: Value) -> Request<Body> {
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn inbox_count(db: &mediaflow_core::platform::db::Db) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM discovery_inbox_directories")
        .fetch_one(db.pool())
        .await
        .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
