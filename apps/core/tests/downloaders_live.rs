use mediaflow_core::connectors::downloader::model::LoadedDownloaderCredentials;
use mediaflow_core::connectors::downloader::port::{
    AddDownloadRequest, DownloadSource, DownloaderEndpoint, RemoteDownloadQuery,
};
use mediaflow_core::connectors::downloader::qbittorrent::QbittorrentClient;
use mediaflow_core::connectors::downloader::transmission::TransmissionClient;
use mediaflow_core::platform::secrets::SecretBytes;

#[tokio::test]
#[ignore = "requires an explicit real qBittorrent and safe test source environment"]
async fn real_qbittorrent_accepts_and_exposes_only_the_created_test_task() {
    let source = SecretBytes::new(required("MEDIAFLOW_DOWNLOAD_TEST_SOURCE").into_bytes());
    verify(
        &QbittorrentClient::production().expect("build qBittorrent client"),
        &required("MEDIAFLOW_QBITTORRENT_BASE_URL"),
        credentials(
            "MEDIAFLOW_QBITTORRENT_USERNAME",
            "MEDIAFLOW_QBITTORRENT_PASSWORD",
        ),
        &source,
        "qbit",
    )
    .await;
}

#[tokio::test]
#[ignore = "requires an explicit real Transmission and safe test source environment"]
async fn real_transmission_accepts_and_exposes_only_the_created_test_task() {
    let source = SecretBytes::new(required("MEDIAFLOW_DOWNLOAD_TEST_SOURCE").into_bytes());
    verify(
        &TransmissionClient::production().expect("build Transmission client"),
        &required("MEDIAFLOW_TRANSMISSION_BASE_URL"),
        credentials(
            "MEDIAFLOW_TRANSMISSION_USERNAME",
            "MEDIAFLOW_TRANSMISSION_PASSWORD",
        ),
        &source,
        "transmission",
    )
    .await;
}

async fn verify(
    client: &dyn DownloadSource,
    base_url: &str,
    credentials: LoadedDownloaderCredentials,
    source: &SecretBytes,
    product: &str,
) {
    let endpoint = DownloaderEndpoint {
        base_url,
        credentials: &credentials,
    };
    let capabilities = client.probe(endpoint).await.expect("probe real downloader");
    assert!(capabilities.manual_add && capabilities.task_monitoring);
    let correlation_tag = format!("mediaflow-live-{product}-{}", uuid::Uuid::now_v7());
    let remote = client
        .add(
            endpoint,
            AddDownloadRequest {
                source,
                correlation_tag: &correlation_tag,
            },
        )
        .await
        .expect("add isolated live download");
    assert!(matches!(remote.remote_id.len(), 40 | 64));
    assert!(
        remote
            .remote_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    let snapshots = client
        .fetch(
            endpoint,
            RemoteDownloadQuery {
                remote_ids: vec![remote.remote_id.clone()],
                correlation_tag: None,
            },
        )
        .await
        .expect("observe created live download");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].remote_id, remote.remote_id);
}

fn credentials(username: &str, password: &str) -> LoadedDownloaderCredentials {
    LoadedDownloaderCredentials::new(
        SecretBytes::new(required(username).into_bytes()),
        SecretBytes::new(required(password).into_bytes()),
    )
}

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required live-test environment variable {name}"))
}
