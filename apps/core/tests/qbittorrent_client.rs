mod downloader_support;

use std::time::Duration;

use downloader_support::{ScriptedHttpServer, ScriptedResponse};
use mediaflow_core::connectors::downloader::model::LoadedDownloaderCredentials;
use mediaflow_core::connectors::downloader::port::{
    AddDownloadRequest, DownloadSource, DownloadSourceError, DownloaderEndpoint,
    RemoteDownloadQuery, RemoteDownloadStatus,
};
use mediaflow_core::connectors::downloader::qbittorrent::{
    QbittorrentClient, QbittorrentClientOptions,
};
use mediaflow_core::platform::secrets::SecretBytes;

const PASSWORD: &str = "qbit-password-must-not-leak";
const SOURCE: &[u8] = b"magnet:?xt=urn:btih:SOURCE_MUST_NOT_LEAK";

fn credentials() -> LoadedDownloaderCredentials {
    LoadedDownloaderCredentials::new(
        SecretBytes::new(b"qbit-user".to_vec()),
        SecretBytes::new(PASSWORD.as_bytes().to_vec()),
    )
}

fn endpoint<'a>(
    server: &'a ScriptedHttpServer,
    credentials: &'a LoadedDownloaderCredentials,
) -> DownloaderEndpoint<'a> {
    DownloaderEndpoint {
        base_url: server.origin.as_str(),
        credentials,
    }
}

fn client() -> QbittorrentClient {
    QbittorrentClient::for_test_loopback(QbittorrentClientOptions {
        connect_timeout: Duration::from_secs(1),
        request_timeout: Duration::from_secs(2),
        max_response_bytes: 64 * 1024,
        max_concurrency: 4,
    })
    .unwrap()
}

fn response(body: impl AsRef<[u8]>) -> ScriptedResponse {
    ScriptedResponse::new("200 OK", &[], body)
}

fn login_response() -> ScriptedResponse {
    ScriptedResponse::new(
        "200 OK",
        &[(
            "Set-Cookie",
            "SID=test-session-id; HttpOnly; SameSite=Strict",
        )],
        "Ok.",
    )
}

fn current_login_response() -> ScriptedResponse {
    ScriptedResponse::new(
        "204 No Content",
        &[(
            "Set-Cookie",
            "QBT_SID_19080=current-session-id; HttpOnly; SameSite=Lax; Path=/",
        )],
        "",
    )
}

#[tokio::test]
async fn probe_uses_a_scoped_cookie_and_accepts_the_minimum_and_current_web_api_versions() {
    for api_version in ["2.8.3", "2.11.4"] {
        let server = ScriptedHttpServer::spawn(vec![
            login_response(),
            response("v5.1.2"),
            response(api_version),
        ])
        .await;
        let credentials = credentials();

        let capabilities = client()
            .probe(endpoint(&server, &credentials))
            .await
            .unwrap();

        assert_eq!(capabilities.product_version, "5.1.2");
        assert_eq!(capabilities.api_version, api_version);
        assert!(capabilities.manual_add);
        assert!(capabilities.task_monitoring);
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("POST /api/v2/auth/login HTTP/1.1"));
        assert!(requests[0].contains("username=qbit-user"));
        assert!(requests[0].contains("password=qbit-password-must-not-leak"));
        assert!(!requests[0].contains("Cookie:"));
        assert!(requests[1].starts_with("GET /api/v2/app/version HTTP/1.1"));
        assert!(requests[2].starts_with("GET /api/v2/app/webapiVersion HTTP/1.1"));
        assert!(requests[1].contains("cookie: SID=test-session-id"));
        assert!(requests[2].contains("cookie: SID=test-session-id"));
    }

    let current = ScriptedHttpServer::spawn(vec![
        current_login_response(),
        response("v5.2.3"),
        response("2.11.4"),
    ])
    .await;
    let current_credentials = credentials();

    let capabilities = client()
        .probe(endpoint(&current, &current_credentials))
        .await
        .unwrap();

    assert_eq!(capabilities.product_version, "5.2.3");
    let requests = current.requests();
    assert!(requests[1].contains("cookie: QBT_SID_19080=current-session-id"));
    assert!(requests[2].contains("cookie: QBT_SID_19080=current-session-id"));

    let unsupported = ScriptedHttpServer::spawn(vec![
        login_response(),
        response("v4.1.9"),
        response("2.8.2"),
    ])
    .await;
    assert_eq!(
        client().probe(endpoint(&unsupported, &credentials())).await,
        Err(DownloadSourceError::UnsupportedVersion)
    );
}

#[tokio::test]
async fn authentication_and_http_statuses_map_to_stable_redacted_failures() {
    for (status, expected) in [
        ("403 Forbidden", DownloadSourceError::Unauthorized),
        ("429 Too Many Requests", DownloadSourceError::RateLimited),
        (
            "405 Method Not Allowed",
            DownloadSourceError::InvalidResponse,
        ),
        ("503 Service Unavailable", DownloadSourceError::Unavailable),
    ] {
        let server = ScriptedHttpServer::spawn(vec![ScriptedResponse::new(status, &[], "")]).await;
        let credentials = credentials();
        let error = client()
            .probe(endpoint(&server, &credentials))
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        let rendered = format!("{error:?} {:?} {credentials:?}", client());
        assert!(!rendered.contains(PASSWORD));
    }
}

#[tokio::test]
async fn add_recovers_a_lost_response_by_unique_tag_without_exposing_the_source() {
    let hash = "0123456789abcdef0123456789abcdef01234567";
    let server = ScriptedHttpServer::spawn(vec![
        login_response(),
        response("Ok."),
        ScriptedResponse::empty_connection(),
        response(format!(
            r#"[{{"hash":"{hash}","state":"downloading","progress":0.5}}]"#
        )),
    ])
    .await;
    let credentials = credentials();
    let source = SecretBytes::new(SOURCE.to_vec());
    let request = AddDownloadRequest {
        source: &source,
        correlation_tag: "mf-019-test",
    };

    let remote = client()
        .add(endpoint(&server, &credentials), request)
        .await
        .unwrap();

    assert_eq!(remote.remote_id, hash);
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    assert!(requests[1].starts_with("POST /api/v2/torrents/createTags HTTP/1.1"));
    assert!(requests[1].contains("tags=mf-019-test"));
    assert!(requests[2].starts_with("POST /api/v2/torrents/add HTTP/1.1"));
    assert!(requests[2].contains("urls=magnet%3A%3Fxt%3Durn%3Abtih%3ASOURCE_MUST_NOT_LEAK"));
    assert!(requests[2].contains("tags=mf-019-test"));
    assert!(requests[3].starts_with("GET /api/v2/torrents/info?tag=mf-019-test HTTP/1.1"));
    let rendered = format!("{:?} {:?}", client(), source);
    assert!(!rendered.contains("SOURCE_MUST_NOT_LEAK"));
}

#[tokio::test]
async fn current_add_ack_is_validated_before_correlation_lookup() {
    let hash = "2222222222222222222222222222222222222222";
    let server = ScriptedHttpServer::spawn(vec![
        current_login_response(),
        response(""),
        response(format!(
            r#"{{"added_torrent_ids":["{hash}"],"failure_count":0,"pending_count":0,"success_count":1}}"#
        )),
        response(format!(
            r#"[{{"hash":"{hash}","state":"stoppedDL","progress":0}}]"#
        )),
    ])
    .await;
    let credentials = credentials();
    let source = SecretBytes::new(SOURCE.to_vec());

    let remote = client()
        .add(
            endpoint(&server, &credentials),
            AddDownloadRequest {
                source: &source,
                correlation_tag: "mediaflow-current-add",
            },
        )
        .await
        .unwrap();

    assert_eq!(remote.remote_id, hash);
}

#[tokio::test]
async fn fetch_maps_only_requested_hashes_to_bounded_product_neutral_snapshots() {
    let hashes = [
        "1111111111111111111111111111111111111111",
        "2222222222222222222222222222222222222222",
        "3333333333333333333333333333333333333333",
        "4444444444444444444444444444444444444444",
        "5555555555555555555555555555555555555555",
    ];
    let body = format!(
        r#"[
          {{"hash":"{}","state":"queuedDL","progress":0.0}},
          {{"hash":"{}","state":"downloading","progress":0.5}},
          {{"hash":"{}","state":"pausedDL","progress":0.75}},
          {{"hash":"{}","state":"uploading","progress":1.0}},
          {{"hash":"{}","state":"futureState","progress":0.25}}
        ]"#,
        hashes[0], hashes[1], hashes[2], hashes[3], hashes[4]
    );
    let server = ScriptedHttpServer::spawn(vec![login_response(), response(body)]).await;
    let credentials = credentials();

    let snapshots = client()
        .fetch(
            endpoint(&server, &credentials),
            RemoteDownloadQuery {
                remote_ids: hashes.iter().map(ToString::to_string).collect(),
                correlation_tag: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(snapshots.len(), 5);
    assert_eq!(snapshots[0].status, RemoteDownloadStatus::Queued);
    assert_eq!(snapshots[1].status, RemoteDownloadStatus::Downloading);
    assert_eq!(snapshots[1].progress_basis_points, 5000);
    assert_eq!(snapshots[2].status, RemoteDownloadStatus::Paused);
    assert_eq!(snapshots[3].status, RemoteDownloadStatus::Completed);
    assert_eq!(snapshots[4].status, RemoteDownloadStatus::Unknown);
    let requests = server.requests();
    assert!(requests[1].contains("hashes=1111111111111111111111111111111111111111%7C2222"));
}

#[tokio::test]
async fn multiple_tag_matches_fail_closed_and_do_not_guess_a_remote_hash() {
    let body = r#"[
      {"hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","state":"downloading","progress":0.1},
      {"hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","state":"downloading","progress":0.2}
    ]"#;
    let server = ScriptedHttpServer::spawn(vec![
        login_response(),
        response("Ok."),
        response("Ok."),
        response(body),
    ])
    .await;
    let credentials = credentials();
    let source = SecretBytes::new(SOURCE.to_vec());

    assert_eq!(
        client()
            .add(
                endpoint(&server, &credentials),
                AddDownloadRequest {
                    source: &source,
                    correlation_tag: "mf-ambiguous",
                },
            )
            .await,
        Err(DownloadSourceError::CorrelationAmbiguous)
    );
}

#[tokio::test]
async fn redirects_oversized_and_stalled_responses_fail_without_forwarding_credentials() {
    let destination = ScriptedHttpServer::spawn(vec![response("unexpected")]).await;
    let redirect = ScriptedHttpServer::spawn(vec![ScriptedResponse::new(
        "302 Found",
        &[("Location", destination.origin.as_str())],
        "",
    )])
    .await;
    let credentials = credentials();
    assert_eq!(
        client().probe(endpoint(&redirect, &credentials)).await,
        Err(DownloadSourceError::InvalidResponse)
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(destination.requests().is_empty());

    let oversized = ScriptedHttpServer::spawn(vec![
        login_response(),
        ScriptedResponse::new("200 OK", &[("Content-Length", "65537")], "short"),
    ])
    .await;
    assert_eq!(
        client().probe(endpoint(&oversized, &credentials)).await,
        Err(DownloadSourceError::ResponseTooLarge)
    );

    let stalled = ScriptedHttpServer::spawn(vec![
        ScriptedResponse::new("200 OK", &[], "Ok.").delayed(Duration::from_millis(250)),
    ])
    .await;
    let short_client = QbittorrentClient::for_test_loopback(QbittorrentClientOptions {
        connect_timeout: Duration::from_millis(100),
        request_timeout: Duration::from_millis(50),
        max_response_bytes: 1024,
        max_concurrency: 1,
    })
    .unwrap();
    assert_eq!(
        short_client.probe(endpoint(&stalled, &credentials)).await,
        Err(DownloadSourceError::Timeout)
    );
}
