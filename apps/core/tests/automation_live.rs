#![cfg(unix)]

use std::ffi::OsStr;
use std::io::Write as _;
use std::path::PathBuf;

use mediaflow_core::automation::rss::client::{FeedClient, FeedHttpClient, FeedResponse};
use mediaflow_core::automation::rss::parser::FeedParser;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::enhancer::model::{
    BaseIdentityHint, EnhancementInput, EnhancerConfigInput,
};
use mediaflow_core::connectors::enhancer::ollama::OllamaClient;
use mediaflow_core::connectors::enhancer::port::IdentificationEnhancer;
use mediaflow_core::discovery::capability::{CapabilityFs, DeploymentRootSet};
use mediaflow_core::discovery::model::{RelativePath, RootId};
use mediaflow_core::platform::capability_fs::OsCapabilityFs;
use mediaflow_core::platform::secrets::SecretBytes;
use serde_json::json;

const OWNERSHIP_MARKER: &[u8] = b"mediaflow-source-automation-live-owned\n";

#[tokio::test]
#[ignore = "requires an explicit real RSS, local Ollama, and isolated capability root"]
async fn real_feed_ollama_and_capability_root_obey_the_production_boundaries() {
    let feed_url = SecretBytes::new(required("MEDIAFLOW_AUTOMATION_LIVE_RSS_URL").into_bytes());
    let response = FeedHttpClient::production()
        .expect("build production RSS client")
        .fetch(&feed_url, None)
        .await
        .expect("fetch the explicitly configured live feed");
    let FeedResponse::Modified { body, .. } = response else {
        panic!("the initial live RSS request must return a bounded feed body")
    };
    let feed = FeedParser::default()
        .parse(&body)
        .expect("parse the live RSS/Atom body with production bounds");
    assert!(
        !feed.items.is_empty(),
        "the live feed must contain at least one supported download item"
    );

    let enhancer = EnhancerConfigInput {
        enabled: true,
        base_url: required("MEDIAFLOW_AUTOMATION_LIVE_OLLAMA_BASE_URL"),
        model: required("MEDIAFLOW_AUTOMATION_LIVE_OLLAMA_MODEL"),
        timeout_ms: 30_000,
    }
    .validate()
    .expect("validate the explicitly configured local Ollama endpoint");
    let ollama = OllamaClient::production().expect("build production Ollama client");
    let probe = ollama
        .probe(enhancer.endpoint())
        .await
        .expect("probe the live Ollama endpoint");
    assert!(
        probe.model_available,
        "the configured model must be installed"
    );
    let input = EnhancementInput::new(
        "Arrival.2016.mkv".to_owned(),
        vec!["Movies".to_owned()],
        BaseIdentityHint::movie("Arrival".to_owned(), Some(2016)),
    )
    .expect("build bounded live enhancement input");
    ollama
        .enhance(enhancer.endpoint(), &input)
        .await
        .expect("the live model must return strict bounded hints");

    let root = canonical_live_root();
    let mut declarations_file = tempfile::NamedTempFile::new().expect("create root declaration");
    serde_json::to_writer(
        &mut declarations_file,
        &json!({
            "roots": [{
                "id": "automation-live",
                "label": "Automation live acceptance",
                "container_path": root,
                "access": "read-write"
            }]
        }),
    )
    .expect("write root declaration");
    declarations_file.flush().expect("flush root declaration");
    let roots = DeploymentRootSet::load(declarations_file.path(), RunMode::Development)
        .expect("load the isolated capability root");
    let filesystem = OsCapabilityFs::open(roots.declarations(), RunMode::Development)
        .expect("open the isolated capability root");
    let root_id = RootId::parse("automation-live").expect("valid root id");
    let root_capability = filesystem
        .preflight_directory(&root_id, &RelativePath::parse(".").unwrap())
        .expect("preflight the capability root")
        .into_capability();
    assert_eq!(
        filesystem
            .read_bounded_relative_file(&root_capability, OsStr::new(".mediaflow-live-owned"), 128,)
            .expect("read the ownership marker through the capability"),
        OWNERSHIP_MARKER
    );
    let inbox = filesystem
        .preflight_directory(&root_id, &RelativePath::parse("incoming").unwrap())
        .expect("preflight the isolated inbox")
        .into_capability();
    let entries = filesystem
        .read_directory(&inbox)
        .expect("enumerate live inbox");
    let media = entries
        .iter()
        .find(|entry| entry.name() == OsStr::new("Arrival.2016.mkv"))
        .expect("find the live media fixture");
    assert!(
        filesystem
            .metadata_no_follow(&inbox, media.name())
            .expect("inspect the fixture without following links")
            .is_file()
    );
}

fn canonical_live_root() -> PathBuf {
    std::fs::canonicalize(required("MEDIAFLOW_AUTOMATION_LIVE_TEST_ROOT"))
        .expect("live script must provide an existing isolated test root")
}

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required live-test environment variable {name}"))
}
