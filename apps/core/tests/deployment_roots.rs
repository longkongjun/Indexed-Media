#![allow(clippy::needless_pass_by_value)]

use std::path::Path;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::capability::DeploymentRootSet;
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::shared::error::ErrorCode;
use serde_json::{Value, json};
use tower::ServiceExt;

mod common;
use common::TestConfigDir;

#[test]
fn strict_configuration_exposes_only_declared_safe_fields() {
    let temp = tempfile::tempdir().unwrap();
    let declared = temp.path().join("declared");
    let undeclared = temp.path().join("undeclared-secret");
    std::fs::create_dir(&declared).unwrap();
    std::fs::create_dir(&undeclared).unwrap();
    let config = temp.path().join("deployment-roots.json");
    write_config(
        &config,
        json!({"roots":[{
            "id":"incoming",
            "label":"Incoming media",
            "container_path":declared,
            "access":"read-only"
        }]}),
    );

    let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
    let response = serde_json::to_value(roots.views()).unwrap();
    assert_eq!(
        response,
        json!([{"id":"incoming","label":"Incoming media","access":"read-only"}])
    );
    let text = response.to_string();
    assert!(!text.contains(declared.to_str().unwrap()));
    assert!(!text.contains(undeclared.to_str().unwrap()));
}

#[test]
fn malformed_duplicate_or_overlapping_root_declarations_fail_closed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    let child = root.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let invalid_documents = [
        "not json".to_owned(),
        json!([]).to_string(),
        json!({"roots":[],"unknown":true}).to_string(),
        json!({"roots":[{"id":"incoming","label":"a","container_path":root,"access":"read-only","unknown":true}]}).to_string(),
        json!({"roots":[
            {"id":"same","label":"a","container_path":root,"access":"read-only"},
            {"id":"same","label":"b","container_path":child,"access":"read-only"}
        ]}).to_string(),
        json!({"roots":[
            {"id":"parent","label":"a","container_path":root,"access":"read-only"},
            {"id":"child","label":"b","container_path":child,"access":"read-only"}
        ]}).to_string(),
        json!({"roots":[{"id":"slash","label":"a","container_path":"/","access":"read-only"}]}).to_string(),
        json!({"roots":[{"id":"relative","label":"a","container_path":"relative","access":"read-only"}]}).to_string(),
        json!({"roots":[{"id":"bad id!","label":"a","container_path":root,"access":"read-only"}]}).to_string(),
        json!({"roots":[{"id":"bad-access","label":"a","container_path":root,"access":"execute"}]}).to_string(),
    ];

    for (index, document) in invalid_documents.into_iter().enumerate() {
        let config = temp.path().join(format!("invalid-{index}.json"));
        std::fs::write(&config, document).unwrap();
        let error = DeploymentRootSet::load(&config, RunMode::Development).unwrap_err();
        assert_eq!(error.code(), ErrorCode::ConfigInvalid, "case {index}");
        assert!(!error.to_string().contains(temp.path().to_str().unwrap()));
    }
}

#[test]
fn lexically_valid_missing_or_file_roots_are_rejected_when_capabilities_open() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("file");
    std::fs::write(&file, b"not a directory").unwrap();
    let missing = temp.path().join("missing");
    for (id, path) in [("missing", missing), ("file", file)] {
        let config = temp.path().join(format!("{id}.json"));
        write_config(&config, one_root(id, &path));
        let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
        let error = OsCapabilityFs::open(roots.declarations(), RunMode::Development)
            .err()
            .unwrap();
        assert_eq!(error.code(), ErrorCode::ConfigInvalid);
    }
}

#[cfg(unix)]
#[test]
fn root_symlink_and_permissionless_root_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("root-link");
    std::fs::create_dir(&target).unwrap();
    symlink(&target, &link).unwrap();
    let config = temp.path().join("symlink.json");
    write_config(&config, one_root("link", &link));
    let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
    let error = OsCapabilityFs::open(roots.declarations(), RunMode::Development)
        .err()
        .unwrap();
    assert_eq!(error.code(), ErrorCode::ConfigInvalid);

    let target = std::fs::canonicalize(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o000)).unwrap();
    write_config(&config, one_root("denied", &target));
    let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
    let denied = OsCapabilityFs::open(roots.declarations(), RunMode::Development)
        .err()
        .unwrap();
    assert_eq!(denied.code(), ErrorCode::ConfigInvalid);
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[tokio::test]
async fn invalid_root_configuration_closes_readiness_and_write_routes() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    std::fs::write(&fixture.config().deployment_roots_file, b"not-json").unwrap();
    let app = build_router(fixture.config().clone(), Some(db));

    let ready = app
        .clone()
        .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);

    let write = app
        .oneshot(
            Request::post("/api/v1/inbox-directories")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"root_id":"incoming","relative_path":"."}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(write.status(), StatusCode::SERVICE_UNAVAILABLE);
}

fn one_root(id: &str, path: &Path) -> Value {
    json!({"roots":[{
        "id":id,
        "label":"Root",
        "container_path":path,
        "access":"read-only"
    }]})
}

fn write_config(path: &Path, value: Value) {
    std::fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}
