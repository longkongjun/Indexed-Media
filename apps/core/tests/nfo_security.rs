use std::ffi::OsStr;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::{DirectoryIdentity, FsBoundaryError, RelativePath, RootId};
use mediaflow_core::identification::model::MediaKind;
use mediaflow_core::identification::nfo::{
    NfoLoadError, NfoLocator, NfoParseError, NfoParser, NfoSource,
};
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use serde_json::json;
use sha2::{Digest, Sha256};

const DOCTYPE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/malicious/doctype.nfo"
));
const XINCLUDE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/malicious/xinclude.nfo"
));
const EPISODE_V22_01: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/valid/episode-v22-01.nfo"
));
const EPISODE_V22_02: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/valid/episode-v22-02.nfo"
));

#[test]
fn rejects_active_xml_constructs_and_entity_references() {
    let parser = NfoParser::default();

    assert_eq!(parser.parse(DOCTYPE), Err(NfoParseError::UnsafeXml));
    assert_eq!(parser.parse(XINCLUDE), Err(NfoParseError::UnsafeXml));
    assert_eq!(
        parser.parse(b"<?probe unsafe?><movie><title>x</title></movie>"),
        Err(NfoParseError::UnsafeXml)
    );
    assert_eq!(
        parser.parse(b"<movie><title>&unknown;</title></movie>"),
        Err(NfoParseError::UnsafeXml)
    );
}

#[test]
fn rejects_document_depth_field_and_file_size_over_boundaries() {
    let parser = NfoParser::default();
    let deep = format!(
        "<movie>{}<title>x</title>{}</movie>",
        "<unknown>".repeat(32),
        "</unknown>".repeat(32)
    );
    let large_field = format!("<movie><title>{}</title></movie>", "x".repeat(65_537));
    let large_file = vec![b'x'; 1_048_577];

    assert_eq!(parser.parse(deep.as_bytes()), Err(NfoParseError::TooDeep));
    assert_eq!(
        parser.parse(large_field.as_bytes()),
        Err(NfoParseError::FieldTooLarge)
    );
    assert_eq!(parser.parse(&large_file), Err(NfoParseError::TooLarge));
}

#[test]
fn capability_lookup_prefers_same_name_movie_nfo_without_modifying_either_file() {
    let fixture = CapabilityFixture::new(".");
    std::fs::write(fixture.root().join("Film.mkv"), b"media").unwrap();
    std::fs::write(
        fixture.root().join("Film.nfo"),
        b"<movie><title>Same name</title></movie>",
    )
    .unwrap();
    std::fs::write(
        fixture.root().join("movie.nfo"),
        b"<movie><title>Fallback</title></movie>",
    )
    .unwrap();
    let media_before = Sha256::digest(std::fs::read(fixture.root().join("Film.mkv")).unwrap());
    let nfo_before = Sha256::digest(std::fs::read(fixture.root().join("Film.nfo")).unwrap());

    let located = NfoLocator::new(&fixture.fs)
        .read_for_media(
            fixture.directory.capability(),
            OsStr::new("Film.mkv"),
            MediaKind::Movie,
            None,
        )
        .unwrap();

    assert_eq!(located.len(), 1);
    assert_eq!(located[0].source, NfoSource::MovieSameName);
    assert!(
        std::str::from_utf8(&located[0].bytes)
            .unwrap()
            .contains("Same name")
    );
    let media_after = Sha256::digest(std::fs::read(fixture.root().join("Film.mkv")).unwrap());
    let nfo_after = Sha256::digest(std::fs::read(fixture.root().join("Film.nfo")).unwrap());
    assert_eq!(media_before, media_after);
    assert_eq!(nfo_before, nfo_after);
}

#[test]
fn episode_lookup_combines_tvshow_and_same_name_episode_nfo() {
    let fixture = CapabilityFixture::new("Show/Season 01");
    std::fs::write(
        fixture.root().join("Show/tvshow.nfo"),
        b"<tvshow><title>Show</title></tvshow>",
    )
    .unwrap();
    std::fs::write(
        fixture.root().join("Show/Season 01/Show.S01E01.nfo"),
        b"<episodedetails><title>Pilot</title></episodedetails>",
    )
    .unwrap();
    let show = fixture.preflight("Show");

    let located = NfoLocator::new(&fixture.fs)
        .read_for_media(
            fixture.directory.capability(),
            OsStr::new("Show.S01E01.mkv"),
            MediaKind::Episode,
            Some(show.capability()),
        )
        .unwrap();

    assert_eq!(located.len(), 2);
    assert_eq!(located[0].source, NfoSource::TvShow);
    assert_eq!(located[1].source, NfoSource::EpisodeSameName);
}

#[test]
fn episode_lookup_supports_kodi_v22_separate_multi_episode_nfos() {
    let fixture = CapabilityFixture::new("Show/Season 01");
    std::fs::write(
        fixture
            .root()
            .join("Show/Season 01/Show.S01E01E02-S01E01.nfo"),
        EPISODE_V22_01,
    )
    .unwrap();
    std::fs::write(
        fixture
            .root()
            .join("Show/Season 01/Show.S01E01E02-S01E02.nfo"),
        EPISODE_V22_02,
    )
    .unwrap();

    let located = NfoLocator::new(&fixture.fs)
        .read_for_episode_references(
            fixture.directory.capability(),
            OsStr::new("Show.S01E01E02.mkv"),
            None,
            &[(1, 1), (1, 2)],
        )
        .unwrap();

    assert_eq!(located.len(), 2);
    assert!(
        located
            .iter()
            .all(|nfo| nfo.source == NfoSource::EpisodeSeparate)
    );
    assert_eq!(
        NfoParser::default()
            .parse(&located[1].bytes)
            .unwrap()
            .documents[0]
            .episode,
        Some(2)
    );
}

#[cfg(unix)]
#[test]
fn lookup_rejects_symlinks_and_preserves_non_utf8_media_stems() {
    use std::os::unix::fs::symlink;

    let fixture = CapabilityFixture::new(".");
    let outside = fixture.temp.path().join("outside.nfo");
    std::fs::write(&outside, b"<movie><title>outside</title></movie>").unwrap();
    symlink(&outside, fixture.root().join("Linked.nfo")).unwrap();
    std::fs::write(
        fixture.root().join("movie.nfo"),
        b"<movie><title>must not fallback</title></movie>",
    )
    .unwrap();

    let error = NfoLocator::new(&fixture.fs)
        .read_for_media(
            fixture.directory.capability(),
            OsStr::new("Linked.mkv"),
            MediaKind::Movie,
            None,
        )
        .unwrap_err();
    assert_eq!(
        error,
        NfoLoadError::Boundary(FsBoundaryError::SymlinkForbidden)
    );

    std::fs::remove_file(fixture.root().join("Linked.nfo")).unwrap();
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let raw_media = std::ffi::OsString::from_vec(vec![
            b'n', b'o', b'n', 0xff, b'u', b'8', b'.', b'm', b'k', b'v',
        ]);
        let raw_nfo = std::ffi::OsString::from_vec(vec![
            b'n', b'o', b'n', 0xff, b'u', b'8', b'.', b'n', b'f', b'o',
        ]);
        std::fs::write(
            fixture.root().join(&raw_nfo),
            b"<movie><title>raw</title></movie>",
        )
        .unwrap();

        let located = NfoLocator::new(&fixture.fs)
            .read_for_media(
                fixture.directory.capability(),
                OsStr::from_bytes(raw_media.as_bytes()),
                MediaKind::Movie,
                None,
            )
            .unwrap();
        assert_eq!(located[0].source, NfoSource::MovieSameName);
    }
}

#[test]
fn lookup_rejects_oversized_nfo_and_invalid_media_components() {
    let fixture = CapabilityFixture::new(".");
    std::fs::write(fixture.root().join("Huge.nfo"), vec![b'x'; 1_048_577]).unwrap();
    let locator = NfoLocator::new(&fixture.fs);

    assert_eq!(
        locator
            .read_for_media(
                fixture.directory.capability(),
                OsStr::new("Huge.mkv"),
                MediaKind::Movie,
                None,
            )
            .unwrap_err(),
        NfoLoadError::TooLarge
    );
    assert_eq!(
        locator
            .read_for_media(
                fixture.directory.capability(),
                OsStr::new("../escape.mkv"),
                MediaKind::Movie,
                None,
            )
            .unwrap_err(),
        NfoLoadError::Boundary(FsBoundaryError::PathInvalid)
    );
}

struct CapabilityFixture {
    temp: tempfile::TempDir,
    library_root: std::path::PathBuf,
    fs: OsCapabilityFs,
    directory: DirectoryIdentity,
}

impl CapabilityFixture {
    fn new(relative: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let library_root = temp.path().join("library");
        std::fs::create_dir_all(&library_root).unwrap();
        if relative != "." {
            std::fs::create_dir_all(library_root.join(relative)).unwrap();
        }
        let library_root = std::fs::canonicalize(library_root).unwrap();
        let config = temp.path().join("deployment-roots.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({"roots":[{
                "id":"incoming",
                "label":"Incoming",
                "container_path":library_root,
                "access":"read-only"
            }]}))
            .unwrap(),
        )
        .unwrap();
        let roots = DeploymentRootSet::load(&config, RunMode::Development).unwrap();
        let fs = OsCapabilityFs::open(roots.declarations(), RunMode::Development).unwrap();
        let root_id = RootId::parse("incoming").unwrap();
        let directory = fs
            .preflight_directory(&root_id, &RelativePath::parse(relative).unwrap())
            .unwrap();
        Self {
            temp,
            library_root,
            fs,
            directory,
        }
    }

    fn root(&self) -> &std::path::Path {
        &self.library_root
    }

    fn preflight(&self, relative: &str) -> DirectoryIdentity {
        self.fs
            .preflight_directory(
                &RootId::parse("incoming").unwrap(),
                &RelativePath::parse(relative).unwrap(),
            )
            .unwrap()
    }
}
