use std::ffi::OsStr;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::{FsBoundaryError, RelativePath, RootId};
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use serde_json::json;

#[test]
fn relative_paths_normalize_components_and_reject_every_escape_form_before_io() {
    let valid = [
        (".", "."),
        ("movies/ready", "movies/ready"),
        ("movies//./ready/", "movies/ready"),
    ];
    for (input, expected) in valid {
        let path = RelativePath::parse(input).unwrap();
        assert_eq!(path.as_str(), expected);
    }
    for invalid in [
        "",
        "/absolute",
        "../outside",
        "inside/../outside",
        "C:\\outside",
        "C:/outside",
        "\\\\server\\share",
        "//server/share",
        "inside\\..\\outside",
        "inside\\child",
        "nul\0suffix",
    ] {
        assert_eq!(
            RelativePath::parse(invalid).unwrap_err(),
            FsBoundaryError::PathInvalid,
            "{invalid:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn real_intermediate_and_terminal_symlinks_never_reach_external_sentinel() {
    use std::os::unix::fs::symlink;

    let fixture = FsFixture::new();
    let outside = fixture.temp.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"outside-secret").unwrap();
    symlink(&outside, fixture.root.join("middle-link")).unwrap();
    symlink(&outside, fixture.root.join("terminal-link")).unwrap();

    for input in ["middle-link/sentinel", "terminal-link"] {
        let error = fixture
            .fs
            .preflight_directory(&fixture.root_id, &RelativePath::parse(input).unwrap())
            .unwrap_err();
        assert_eq!(error, FsBoundaryError::SymlinkForbidden);
        assert!(!format!("{error:?}").contains(outside.to_str().unwrap()));
    }

    let root_identity = fixture
        .fs
        .preflight_directory(&fixture.root_id, &RelativePath::parse(".").unwrap())
        .unwrap();
    let capability = root_identity.capability();
    std::fs::write(fixture.root.join("literal\\name"), b"inside-root").unwrap();
    assert!(
        fixture
            .fs
            .metadata_no_follow(capability, OsStr::new("literal\\name"))
            .unwrap()
            .is_file(),
        "a raw Unix child backslash is data, not a request-path separator"
    );
    let metadata = fixture
        .fs
        .metadata_no_follow(capability, OsStr::new("terminal-link"))
        .unwrap();
    assert!(metadata.is_symlink());
    assert_eq!(
        fixture
            .fs
            .read_directory(&root_identity.into_capability())
            .unwrap()
            .iter()
            .filter(|entry| entry.name() == OsStr::new("sentinel"))
            .count(),
        0
    );
}

#[cfg(unix)]
#[test]
fn configured_root_with_intermediate_symlink_is_rejected_by_capability_open() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let real_parent = temp.path().join("real-parent");
    let root = real_parent.join("root");
    let alias_parent = temp.path().join("alias-parent");
    std::fs::create_dir_all(&root).unwrap();
    symlink(&real_parent, &alias_parent).unwrap();
    let config = temp.path().join("deployment-roots.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({"roots":[{
            "id":"incoming",
            "label":"Incoming",
            "container_path":alias_parent.join("root"),
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();

    let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
    assert!(OsCapabilityFs::open(roots.declarations(), RunMode::Development).is_err());
}

#[cfg(unix)]
#[test]
fn root_replacement_permission_changes_and_non_utf8_names_are_fail_closed_or_lossless() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = FsFixture::new();
    let available = fixture.root.join("available");
    std::fs::create_dir(&available).unwrap();
    let identity = fixture
        .fs
        .preflight_directory(&fixture.root_id, &RelativePath::parse("available").unwrap())
        .unwrap();
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let raw_name = std::ffi::OsString::from_vec(vec![b'n', b'o', b'n', 0xff, b'u', b'8']);
        std::fs::write(available.join(&raw_name), b"raw bytes").unwrap();
        let entries = fixture.fs.read_directory(identity.capability()).unwrap();
        let raw_entry = entries
            .iter()
            .find(|entry| entry.name().as_bytes() == raw_name.as_bytes())
            .expect("non-UTF-8 entry retained");
        assert_eq!(raw_entry.name().as_bytes(), raw_name.as_bytes());
        let metadata = fixture
            .fs
            .metadata_no_follow(identity.capability(), raw_entry.name())
            .unwrap();
        assert!(metadata.is_file());
    }
    #[cfg(not(target_os = "linux"))]
    eprintln!("non-UTF-8 directory entry assertion skipped; Linux rerun required");

    for mode in [0o400, 0o100, 0o000] {
        std::fs::set_permissions(&available, std::fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(
            fixture
                .fs
                .preflight_directory(&fixture.root_id, &RelativePath::parse("available").unwrap())
                .unwrap_err(),
            FsBoundaryError::Unavailable,
            "mode {mode:o} must not satisfy both effective read and search access"
        );
    }
    std::fs::set_permissions(&available, std::fs::Permissions::from_mode(0o700)).unwrap();
    fixture
        .fs
        .preflight_directory(&fixture.root_id, &RelativePath::parse("available").unwrap())
        .unwrap();

    let old_root = fixture.temp.path().join("old-root");
    std::fs::rename(&fixture.root, &old_root).unwrap();
    std::fs::create_dir(&fixture.root).unwrap();
    std::fs::write(fixture.root.join("replacement-sentinel"), b"must-not-read").unwrap();
    assert_eq!(
        fixture
            .fs
            .read_directory(identity.capability())
            .unwrap_err(),
        FsBoundaryError::RootChanged
    );
}

#[cfg(unix)]
#[test]
fn directory_capabilities_are_bound_to_the_issuing_filesystem_instance() {
    let fixture = FsFixture::new();
    std::fs::write(fixture.root.join("sentinel"), b"inside").unwrap();
    let config = fixture.temp.path().join("second-roots.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({"roots":[{
            "id":"incoming",
            "label":"Incoming",
            "container_path":fixture.root,
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
    let declarations = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
    let second = OsCapabilityFs::open(declarations.declarations(), RunMode::Development).unwrap();
    let capability = fixture
        .fs
        .preflight_directory(&fixture.root_id, &RelativePath::parse(".").unwrap())
        .unwrap()
        .into_capability();

    assert_eq!(
        second.read_directory(&capability).unwrap_err(),
        FsBoundaryError::PathInvalid
    );
    assert_eq!(
        second
            .metadata_no_follow(&capability, OsStr::new("sentinel"))
            .unwrap_err(),
        FsBoundaryError::PathInvalid
    );
}

#[cfg(target_os = "linux")]
#[test]
fn production_root_requires_mount_boundary_and_detects_bind_replacement() {
    let temp = tempfile::tempdir().unwrap();
    let ordinary = temp.path().join("ordinary-directory");
    std::fs::create_dir(&ordinary).unwrap();
    let ordinary = std::fs::canonicalize(ordinary).unwrap();
    let config = temp.path().join("ordinary.json");
    write_root_config(&config, &ordinary);
    let declarations = DeploymentRootSet::load(&config, RunMode::Production).unwrap();
    assert!(OsCapabilityFs::open(declarations.declarations(), RunMode::Production).is_err());

    let Some(mount_root) = std::env::var_os("MEDIAFLOW_TEST_PRODUCTION_MOUNT_ROOT") else {
        eprintln!("bind-mount replacement branch requires the Linux mount harness");
        return;
    };
    let mount_root = std::path::PathBuf::from(mount_root);
    write_root_config(&config, &mount_root);
    let declarations = DeploymentRootSet::load(&config, RunMode::Production).unwrap();
    let fs = OsCapabilityFs::open(declarations.declarations(), RunMode::Production).unwrap();
    let root_id = RootId::parse("incoming").unwrap();
    fs.preflight_directory(&root_id, &RelativePath::parse(".").unwrap())
        .unwrap();

    let helper = std::env::var_os("MEDIAFLOW_TEST_BIND_REPLACE_HELPER")
        .expect("mount harness must provide a bind replacement helper executable");
    let status = std::process::Command::new(helper)
        .arg(&mount_root)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(
        fs.preflight_directory(&root_id, &RelativePath::parse(".").unwrap())
            .unwrap_err(),
        FsBoundaryError::RootChanged
    );
}

#[cfg(target_os = "linux")]
#[test]
fn linux_uid_gid_acl_harness_checks_effective_read_and_search_access() {
    let Some(root) = std::env::var_os("MEDIAFLOW_TEST_PERMISSION_ROOT") else {
        eprintln!("UID/GID/ACL branches require the Linux permission harness");
        return;
    };
    let expected = std::env::var("MEDIAFLOW_TEST_EXPECT_ACCESS")
        .expect("permission harness must set available or unavailable");
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("permissions.json");
    write_root_config(&config, std::path::Path::new(&root));
    let declarations = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
    let opened = OsCapabilityFs::open(declarations.declarations(), RunMode::Development);
    match expected.as_str() {
        "available" => {
            let fs = opened.unwrap();
            fs.preflight_directory(
                &RootId::parse("incoming").unwrap(),
                &RelativePath::parse(".").unwrap(),
            )
            .unwrap();
        }
        "unavailable" => assert!(opened.is_err()),
        _ => panic!("MEDIAFLOW_TEST_EXPECT_ACCESS must be available or unavailable"),
    }
}

#[cfg(target_os = "linux")]
fn write_root_config(path: &std::path::Path, root: &std::path::Path) {
    std::fs::write(
        path,
        serde_json::to_vec(&json!({"roots":[{
            "id":"incoming",
            "label":"Incoming",
            "container_path":root,
            "access":"read-only"
        }]}))
        .unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
struct FsFixture {
    temp: tempfile::TempDir,
    root: std::path::PathBuf,
    root_id: RootId,
    fs: OsCapabilityFs,
}

#[cfg(unix)]
impl FsFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let config = temp.path().join("deployment-roots.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({"roots":[{
                "id":"incoming",
                "label":"Incoming",
                "container_path":root,
                "access":"read-only"
            }]}))
            .unwrap(),
        )
        .unwrap();
        let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
        let fs = OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap();
        Self {
            temp,
            root,
            root_id: RootId::parse("incoming").unwrap(),
            fs,
        }
    }
}
