#![allow(clippy::needless_pass_by_value, clippy::too_many_lines)]

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const SECRET: &str = "m3-organization-bootstrap-secret";

#[tokio::test]
async fn session_origin_and_csrf_guards_run_before_organization_input_parsing() {
    let (_fixture, _db, app, cookie, csrf, _roots) = authenticated_app().await;

    let unauthenticated_get = app
        .clone()
        .oneshot(
            Request::get("/api/v1/organization-targets/not-a-uuid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauthenticated_get.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        json_body(unauthenticated_get).await["error"]["code"],
        "session.expired"
    );

    let unauthenticated_write = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets",
            "",
            "https://evil.test",
            "bad",
            json!({"unknown":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unauthenticated_write.status(), StatusCode::UNAUTHORIZED);

    let bad_origin = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets",
            &cookie,
            "https://evil.test",
            &csrf,
            json!({"unknown":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bad_origin.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        json_body(bad_origin).await["error"]["code"],
        "origin.untrusted"
    );

    let bad_csrf = app
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets",
            &cookie,
            "http://127.0.0.1:3000",
            "bad",
            json!({"unknown":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(bad_csrf.status(), StatusCode::FORBIDDEN);
    assert_eq!(json_body(bad_csrf).await["error"]["code"], "csrf.invalid");
}

#[tokio::test]
async fn preflight_is_strict_safe_non_mutating_and_reports_capability_or_overlap() {
    let (fixture, db, app, cookie, csrf, roots) = authenticated_app().await;
    seed_inbox(db.pool(), "media", "incoming").await;
    let before = tree_snapshot(&roots.media);

    let malformed = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets/preflights",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            json!({"root_id":"media","relative_path":"Movies","unknown":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let unknown_root = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets/preflights",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            json!({"root_id":"missing","relative_path":"Movies"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unknown_root.status(), StatusCode::NOT_FOUND);

    let invalid_path = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets/preflights",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            json!({"root_id":"media","relative_path":"../escape"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(invalid_path.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let read_only = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets/preflights",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            json!({"root_id":"incoming","relative_path":"dropbox"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(read_only.status(), StatusCode::OK);
    let read_only_body = json_body(read_only).await;
    assert_eq!(
        read_only_body,
        json!({
            "root_id":"incoming",
            "relative_path":"dropbox",
            "writable":false,
            "overlaps_existing":false,
            "same_filesystem_hint":null,
            "failure_code":"organization.root-read-only"
        })
    );
    assert!(
        !read_only_body
            .to_string()
            .contains(roots.incoming.to_str().unwrap())
    );

    let overlap = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets/preflights",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            json!({"root_id":"media","relative_path":"incoming/child"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(overlap.status(), StatusCode::OK);
    let overlap_body = json_body(overlap).await;
    assert_eq!(overlap_body["writable"], true);
    assert_eq!(overlap_body["overlaps_existing"], true);
    assert_eq!(overlap_body["failure_code"], "organization.target-overlap");

    let available = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets/preflights",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            json!({"root_id":"media","relative_path":"Movies//./"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(available.status(), StatusCode::OK);
    assert_eq!(
        json_body(available).await,
        json!({
            "root_id":"media",
            "relative_path":"Movies",
            "writable":true,
            "overlaps_existing":false,
            "same_filesystem_hint":null,
            "failure_code":null
        })
    );
    assert_eq!(tree_snapshot(&roots.media), before);

    let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM organization_targets")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
    let audit_text = audit_text(&db).await;
    assert!(audit_text.contains("organization.target-preflight"));
    assert!(!audit_text.contains(roots.media.to_str().unwrap()));
    assert!(!audit_text.contains(fixture.config().config_dir.to_str().unwrap()));
}

#[tokio::test]
async fn authenticated_crud_is_versioned_audited_and_never_returns_host_paths() {
    let (_fixture, db, app, cookie, csrf, roots) = authenticated_app().await;
    let input = target_json("Movies", "Movie library", "copy", true);
    let created = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            input,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_body = json_body(created).await;
    let id = created_body["id"].as_str().unwrap();
    assert_eq!(created_body["config_version"], 1);
    assert_eq!(created_body["operation"], "copy");
    assert!(created_body["updated_at"].as_str().unwrap().ends_with('Z'));
    assert!(
        !created_body
            .to_string()
            .contains(roots.media.to_str().unwrap())
    );

    let listed = app
        .clone()
        .oneshot(get("/api/v1/organization-targets?limit=10", &cookie))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    assert_eq!(json_body(listed).await["items"][0]["id"], id);

    let detail = app
        .clone()
        .oneshot(get(&format!("/api/v1/organization-targets/{id}"), &cookie))
        .await
        .unwrap();
    assert_eq!(detail.status(), StatusCode::OK);

    let stale = app
        .clone()
        .oneshot(write_request(
            "PUT",
            &format!("/api/v1/organization-targets/{id}"),
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            target_json("Films", "Film library", "hardlink", false),
            Some(2),
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(stale).await["error"]["code"], "request.conflict");

    let replaced = app
        .clone()
        .oneshot(write_request(
            "PUT",
            &format!("/api/v1/organization-targets/{id}"),
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            target_json("Films", "Film library", "hardlink", false),
            Some(1),
        ))
        .await
        .unwrap();
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced_body = json_body(replaced).await;
    assert_eq!(replaced_body["config_version"], 2);
    assert_eq!(replaced_body["relative_path"], "Films");
    assert_eq!(replaced_body["operation"], "hardlink");

    let stale_delete = app
        .clone()
        .oneshot(delete_request(
            &format!("/api/v1/organization-targets/{id}"),
            &cookie,
            &csrf,
            1,
        ))
        .await
        .unwrap();
    assert_eq!(stale_delete.status(), StatusCode::CONFLICT);

    let deleted = app
        .clone()
        .oneshot(delete_request(
            &format!("/api/v1/organization-targets/{id}"),
            &cookie,
            &csrf,
            2,
        ))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let missing = app
        .oneshot(get(&format!("/api/v1/organization-targets/{id}"), &cookie))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let audit = audit_text(&db).await;
    for action in [
        "organization.target-create",
        "organization.target-replace",
        "organization.target-delete",
    ] {
        assert!(audit.contains(action), "missing audit action {action}");
    }
    assert!(!audit.contains(roots.media.to_str().unwrap()));
}

#[tokio::test]
async fn inbox_creation_cannot_bypass_an_existing_organization_target() {
    let (_fixture, db, app, cookie, csrf, roots) = authenticated_app().await;
    let created = app
        .clone()
        .oneshot(write_request(
            "POST",
            "/api/v1/organization-targets",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            target_json("Movies", "Movie library", "copy", true),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    std::fs::create_dir_all(roots.media.join("Movies/new-inbox")).unwrap();

    let inbox = app
        .oneshot(write_request(
            "POST",
            "/api/v1/inbox-directories",
            &cookie,
            "http://127.0.0.1:3000",
            &csrf,
            json!({"root_id":"media","relative_path":"Movies/new-inbox"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(inbox.status(), StatusCode::CONFLICT);
    assert_eq!(json_body(inbox).await["error"]["code"], "inbox.overlap");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM discovery_inbox_directories")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn deployment_root_config_replacement_closes_organization_routes() {
    let (fixture, _db, app, cookie, _csrf, roots) = authenticated_app().await;
    let staged = fixture
        .config()
        .deployment_roots_file
        .with_extension("json.next");
    std::fs::write(
        &staged,
        serde_json::to_vec(&json!({"roots":[{
            "id":"media",
            "label":"Changed",
            "container_path":roots.media,
            "access":"read-write"
        }]}))
        .unwrap(),
    )
    .unwrap();
    std::fs::rename(&staged, &fixture.config().deployment_roots_file).unwrap();

    let response = app
        .oneshot(get("/api/v1/organization-targets", &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(json_body(response).await["error"]["code"], "internal.error");
}

struct TestRoots {
    incoming: PathBuf,
    media: PathBuf,
}

async fn authenticated_app() -> (
    Box<TestConfigDir>,
    mediaflow_core::platform::db::Db,
    axum::Router,
    String,
    String,
    TestRoots,
) {
    let fixture = Box::new(TestConfigDir::new(RunMode::Development));
    let base = fixture.config().config_dir.parent().unwrap();
    let incoming = base.join("incoming-root");
    let media = base.join("media-root");
    std::fs::create_dir_all(incoming.join("dropbox")).unwrap();
    for path in ["Movies", "Films", "incoming"] {
        std::fs::create_dir_all(media.join(path)).unwrap();
    }
    let incoming = std::fs::canonicalize(incoming).unwrap();
    let media = std::fs::canonicalize(media).unwrap();
    std::fs::write(
        &fixture.config().deployment_roots_file,
        serde_json::to_vec(&json!({"roots":[
            {"id":"incoming","label":"Incoming","container_path":incoming,"access":"read-only"},
            {"id":"media","label":"Media","container_path":media,"access":"read-write"}
        ]}))
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
    (
        fixture,
        db,
        app,
        cookie,
        csrf,
        TestRoots { incoming, media },
    )
}

fn target_json(relative_path: &str, display_name: &str, operation: &str, automatic: bool) -> Value {
    json!({
        "kind":"movie",
        "display_name":display_name,
        "root_id":"media",
        "relative_path":relative_path,
        "operation":operation,
        "naming_pattern":"movie",
        "nfo_policy":"generate-missing",
        "automatic":automatic,
        "enabled":true,
        "rules":[{
            "media_kind":"movie",
            "inbox_directory_id":null,
            "explicit_tag":"movies",
            "enabled":true
        }]
    })
}

async fn seed_inbox(pool: &sqlx::SqlitePool, root_id: &str, path: &str) {
    sqlx::query(
        "INSERT INTO discovery_inbox_directories
         (id,root_id,relative_path_bytes,relative_path_display,root_identity,directory_identity,
          health,last_checked_at_us,version,created_at_us,updated_at_us)
         VALUES (?,?,?,?,x'01',x'02','available',0,1,0,0)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(root_id)
    .bind(path.as_bytes())
    .bind(path)
    .execute(pool)
    .await
    .unwrap();
}

fn tree_snapshot(root: &Path) -> BTreeSet<PathBuf> {
    fn visit(root: &Path, current: &Path, entries: &mut BTreeSet<PathBuf>) {
        for entry in std::fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            entries.insert(path.strip_prefix(root).unwrap().to_owned());
            if path.is_dir() {
                visit(root, &path, entries);
            }
        }
    }
    let mut entries = BTreeSet::new();
    visit(root, root, &mut entries);
    entries
}

fn get(path: &str, cookie: &str) -> Request<Body> {
    Request::get(path)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn write_request(
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    csrf: &str,
    body: Value,
    version: Option<i64>,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, origin)
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", csrf);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    if let Some(version) = version {
        request = request.header(header::IF_MATCH, version.to_string());
    }
    request.body(Body::from(body.to_string())).unwrap()
}

fn delete_request(path: &str, cookie: &str, csrf: &str, version: i64) -> Request<Body> {
    Request::delete(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, "http://127.0.0.1:3000")
        .header("sec-fetch-site", "same-origin")
        .header("x-csrf-token", csrf)
        .header(header::IF_MATCH, version.to_string())
        .body(Body::empty())
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

async fn audit_text(db: &mediaflow_core::platform::db::Db) -> String {
    sqlx::query_scalar(
        "SELECT COALESCE(group_concat(action || COALESCE(subject_id,'') || safe_details_json, ''), '')
         FROM platform_audit_events",
    )
    .fetch_one(db.pool())
    .await
    .unwrap()
}

async fn json_body(response: axum::response::Response) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}
