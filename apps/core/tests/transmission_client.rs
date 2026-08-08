mod downloader_support;

use std::time::Duration;

use downloader_support::{ScriptedHttpServer, ScriptedResponse};
use mediaflow_core::connectors::downloader::model::LoadedDownloaderCredentials;
use mediaflow_core::connectors::downloader::port::{
    AddDownloadRequest, DownloadSource, DownloadSourceError, DownloaderEndpoint,
    RemoteDownloadQuery, RemoteDownloadStatus,
};
use mediaflow_core::connectors::downloader::transmission::{
    TransmissionClient, TransmissionClientOptions,
};
use mediaflow_core::platform::secrets::SecretBytes;

const PASSWORD: &str = "transmission-password-must-not-leak";
const SOURCE: &[u8] = b"https://tracker.invalid/private/SOURCE_MUST_NOT_LEAK.torrent";

fn credentials() -> LoadedDownloaderCredentials {
    LoadedDownloaderCredentials::new(
        SecretBytes::new(b"transmission-user".to_vec()),
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

fn client() -> TransmissionClient {
    TransmissionClient::for_test_loopback(TransmissionClientOptions {
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

fn conflict(version: &str) -> ScriptedResponse {
    if version.starts_with('6') {
        ScriptedResponse::new(
            "409 Conflict",
            &[
                ("X-Transmission-Session-Id", "test-session-id"),
                ("X-Transmission-Rpc-Version", version),
            ],
            "",
        )
    } else {
        ScriptedResponse::new(
            "409 Conflict",
            &[("X-Transmission-Session-Id", "test-session-id")],
            "",
        )
    }
}

fn legacy_session() -> ScriptedResponse {
    legacy_session_version("5.3.0")
}

fn legacy_session_version(version: &str) -> ScriptedResponse {
    response(format!(
        r#"{{"arguments":{{"rpc-version-semver":"{version}","version":"4.0.6"}},"result":"success"}}"#
    ))
}

fn json_session() -> ScriptedResponse {
    response(
        r#"{"jsonrpc":"2.0","id":1,"result":{"rpc_version_semver":"6.1.0","version":"4.2.0"}}"#,
    )
}

#[tokio::test]
async fn probe_negotiates_legacy_53_and_json_rpc_6_with_session_id_retry_and_basic_auth() {
    for (version, session, expected_method, expected_product, expected_api) in [
        (
            "5.3.0",
            legacy_session(),
            "\"method\":\"session-get\"",
            "4.0.6",
            "5.3.0",
        ),
        (
            "6.1.0",
            json_session(),
            "\"method\":\"session_get\"",
            "4.2.0",
            "6.1.0",
        ),
    ] {
        let server = ScriptedHttpServer::spawn(vec![conflict(version), session]).await;
        let credentials = credentials();

        let capabilities = client()
            .probe(endpoint(&server, &credentials))
            .await
            .unwrap();

        assert_eq!(capabilities.product_version, expected_product);
        assert_eq!(capabilities.api_version, expected_api);
        assert!(capabilities.manual_add);
        assert!(capabilities.task_monitoring);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("POST /transmission/rpc HTTP/1.1"));
        assert!(requests[0].contains("authorization: Basic "));
        assert!(!requests[0].contains(PASSWORD));
        assert!(requests[0].contains("\"method\":\"session-get\""));
        assert!(requests[1].contains("x-transmission-session-id: test-session-id"));
        assert!(requests[1].contains(expected_method));
        assert!(requests[1].contains("version"));
        assert!(requests[1].contains("rpc"));
        if version.starts_with('6') {
            assert!(requests[1].contains("\"jsonrpc\":\"2.0\""));
        }
    }
}

#[tokio::test]
async fn legacy_added_and_json_rpc_duplicate_both_return_the_stable_hash() {
    let cases = [
        (
            "5.3.0",
            legacy_session(),
            response(
                r#"{"arguments":{"torrent-added":{"hashString":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},"result":"success"}"#,
            ),
            "\"method\":\"torrent-add\"",
        ),
        (
            "6.1.0",
            json_session(),
            response(
                r#"{"jsonrpc":"2.0","id":2,"result":{"torrent_duplicate":{"hash_string":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}"#,
            ),
            "\"method\":\"torrent_add\"",
        ),
    ];
    for (version, session, add_response, expected_method) in cases {
        let server =
            ScriptedHttpServer::spawn(vec![conflict(version), session, add_response]).await;
        let credentials = credentials();
        let source = SecretBytes::new(SOURCE.to_vec());

        let remote = client()
            .add(
                endpoint(&server, &credentials),
                AddDownloadRequest {
                    source: &source,
                    correlation_tag: "mf-transmission",
                },
            )
            .await
            .unwrap();

        assert!(matches!(remote.remote_id.as_bytes()[0], b'a' | b'b'));
        assert_eq!(remote.remote_id.len(), 40);
        let requests = server.requests();
        assert!(requests[2].contains(expected_method));
        assert!(requests[2].contains("SOURCE_MUST_NOT_LEAK.torrent"));
        let rendered = format!("{:?} {:?}", client(), source);
        assert!(!rendered.contains("SOURCE_MUST_NOT_LEAK"));
    }
}

#[tokio::test]
async fn both_codecs_map_common_fields_and_unknown_states_without_guessing() {
    let hash_a = "1111111111111111111111111111111111111111";
    let hash_b = "2222222222222222222222222222222222222222";
    let cases = [
        (
            "5.3.0",
            legacy_session(),
            response(format!(
                r#"{{"arguments":{{"torrents":[
                  {{"hashString":"{hash_a}","status":4,"percentDone":0.5,"error":0}},
                  {{"hashString":"{hash_b}","status":99,"percentDone":0.25,"error":0}}
                ]}},"result":"success"}}"#
            )),
            "\"method\":\"torrent-get\"",
        ),
        (
            "6.1.0",
            json_session(),
            response(format!(
                r#"{{"jsonrpc":"2.0","id":2,"result":{{"torrents":[
                  {{"hash_string":"{hash_a}","status":4,"percent_done":0.5,"error":0}},
                  {{"hash_string":"{hash_b}","status":6,"percent_done":1.0,"error":0}}
                ]}}}}"#
            )),
            "\"method\":\"torrent_get\"",
        ),
    ];
    for (version, session, fetch_response, expected_method) in cases {
        let server =
            ScriptedHttpServer::spawn(vec![conflict(version), session, fetch_response]).await;
        let credentials = credentials();

        let snapshots = client()
            .fetch(
                endpoint(&server, &credentials),
                RemoteDownloadQuery {
                    remote_ids: vec![hash_a.to_owned(), hash_b.to_owned()],
                    correlation_tag: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(snapshots[0].status, RemoteDownloadStatus::Downloading);
        assert_eq!(snapshots[0].progress_basis_points, 5000);
        assert!(matches!(
            snapshots[1].status,
            RemoteDownloadStatus::Unknown | RemoteDownloadStatus::Completed
        ));
        assert!(server.requests()[2].contains(expected_method));
    }
}

#[tokio::test]
async fn unsupported_versions_and_http_failures_are_stable_and_redacted() {
    let unsupported =
        ScriptedHttpServer::spawn(vec![conflict("5.2.0"), legacy_session_version("5.2.0")]).await;
    let credentials = credentials();
    assert_eq!(
        client().probe(endpoint(&unsupported, &credentials)).await,
        Err(DownloadSourceError::UnsupportedVersion)
    );

    for (status, expected) in [
        ("401 Unauthorized", DownloadSourceError::Unauthorized),
        ("429 Too Many Requests", DownloadSourceError::RateLimited),
        ("503 Service Unavailable", DownloadSourceError::Unavailable),
    ] {
        let server = ScriptedHttpServer::spawn(vec![ScriptedResponse::new(status, &[], "")]).await;
        let error = client()
            .probe(endpoint(&server, &credentials))
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {:?} {credentials:?}", client()).contains(PASSWORD));
    }
}

#[tokio::test]
async fn redirects_oversized_and_stalled_responses_fail_closed() {
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
        conflict("5.3.0"),
        ScriptedResponse::new("200 OK", &[("Content-Length", "65537")], "short"),
    ])
    .await;
    assert_eq!(
        client().probe(endpoint(&oversized, &credentials)).await,
        Err(DownloadSourceError::ResponseTooLarge)
    );

    let stalled = ScriptedHttpServer::spawn(vec![
        ScriptedResponse::new("200 OK", &[], "{}").delayed(Duration::from_millis(250)),
    ])
    .await;
    let short_client = TransmissionClient::for_test_loopback(TransmissionClientOptions {
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
