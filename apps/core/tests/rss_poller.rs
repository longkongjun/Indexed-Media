#![allow(clippy::too_many_lines)]

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::get;
use mediaflow_core::automation::rss::client::{
    FeedClient, FeedClientError, FeedClientOptions, FeedHttpClient, FeedResponse, RssCursor,
};
use mediaflow_core::automation::rss::poller::{
    AutomationEventDraft, FeedPollCommit, FeedPollCommitPort, FeedPollSource, FeedPoller,
};
use mediaflow_core::platform::secrets::SecretBytes;
use uuid::Uuid;

const RSS: &[u8] = include_bytes!("fixtures/rss/rss20.xml");

#[derive(Default)]
struct FakeCommitPort {
    accepted: Mutex<BTreeSet<(Uuid, [u8; 32])>>,
    cursor: Mutex<Option<RssCursor>>,
    fail_next: Mutex<bool>,
}

#[async_trait]
impl FeedPollCommitPort for FakeCommitPort {
    async fn commit(
        &self,
        source_id: Uuid,
        _expected_config_version: i64,
        commit: FeedPollCommit,
    ) -> Result<usize, FeedClientError> {
        if std::mem::take(&mut *self.fail_next.lock().unwrap()) {
            return Err(FeedClientError::Unavailable);
        }
        let mut accepted = self.accepted.lock().unwrap();
        let before = accepted.len();
        for draft in commit.drafts {
            assert!(!format!("{draft:?}").contains("token=private"));
            accepted.insert((source_id, draft.dedup_key));
        }
        *self.cursor.lock().unwrap() = commit.cursor;
        Ok(accepted.len() - before)
    }

    async fn commit_failure(
        &self,
        _source_id: Uuid,
        _expected_config_version: i64,
        _failure: FeedClientError,
    ) -> Result<(), FeedClientError> {
        Ok(())
    }
}

struct StaticClient;

#[async_trait]
impl FeedClient for StaticClient {
    async fn fetch(
        &self,
        _feed_url: &SecretBytes,
        cursor: Option<&RssCursor>,
    ) -> Result<FeedResponse, FeedClientError> {
        Ok(FeedResponse::Modified {
            body: RSS.to_vec(),
            cursor: Some(
                RssCursor::new(
                    Some("\"feed-v2\"".to_owned()),
                    Some("Fri, 24 Jul 2026 01:02:03 GMT".to_owned()),
                )
                .unwrap(),
            ),
            previous_cursor_present: cursor.is_some(),
        })
    }
}

#[tokio::test]
async fn repeated_poll_deduplicates_drafts_and_failed_commit_does_not_advance_cursor() {
    let commits = Arc::new(FakeCommitPort::default());
    let poller = FeedPoller::new(Arc::new(StaticClient), commits.clone());
    let source_id = Uuid::now_v7();
    let claimed_source = source(source_id);

    *commits.fail_next.lock().unwrap() = true;
    let failed = poller.poll(source(source_id)).await.unwrap_err();
    assert_eq!(failed, FeedClientError::Unavailable);
    assert!(commits.cursor.lock().unwrap().is_none());
    assert!(commits.accepted.lock().unwrap().is_empty());

    let first = poller.poll(claimed_source).await.unwrap();
    assert_eq!(first.accepted_event_count, 2);
    let second = poller.poll(source(source_id)).await.unwrap();
    assert_eq!(second.accepted_event_count, 0);
    assert_eq!(commits.accepted.lock().unwrap().len(), 2);
    assert_eq!(
        commits.cursor.lock().unwrap().as_ref().unwrap().etag(),
        Some("\"feed-v2\"")
    );
}

#[tokio::test]
async fn http_client_sends_conditionals_rejects_redirect_and_bounds_status_body_and_timeout() {
    let captured = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
    let destination_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let destination = TestServer::spawn(Router::new().route(
        "/capture",
        get({
            let calls = destination_calls.clone();
            move || {
                let calls = calls.clone();
                async move {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    StatusCode::OK
                }
            }
        }),
    ))
    .await;
    let redirect = destination.origin.join("capture").unwrap().to_string();
    let server = TestServer::spawn(
        Router::new()
            .route(
                "/feed",
                get({
                    let captured = captured.clone();
                    move |headers: HeaderMap| {
                        let captured = captured.clone();
                        async move {
                            captured.lock().unwrap().push(headers);
                            (
                                StatusCode::OK,
                                [
                                    (header::ETAG, "\"feed-v2\""),
                                    (header::LAST_MODIFIED, "Fri, 24 Jul 2026 01:02:03 GMT"),
                                ],
                                RSS,
                            )
                        }
                    }
                }),
            )
            .route("/not-modified", get(|| async { StatusCode::NOT_MODIFIED }))
            .route(
                "/redirect",
                get(move || {
                    let redirect = redirect.clone();
                    async move { (StatusCode::FOUND, [(header::LOCATION, redirect)]) }
                }),
            )
            .route("/rate", get(|| async { StatusCode::TOO_MANY_REQUESTS }))
            .route("/unauthorized", get(|| async { StatusCode::UNAUTHORIZED }))
            .route(
                "/server-error",
                get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
            )
            .route("/large", get(|| async { vec![b'x'; 2 * 1024 * 1024 + 1] }))
            .route(
                "/stalled",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    Body::from(RSS)
                }),
            ),
    )
    .await;
    let client = FeedHttpClient::for_test_loopback(FeedClientOptions {
        connect_timeout: Duration::from_millis(100),
        request_timeout: Duration::from_millis(100),
        max_response_bytes: 2 * 1024 * 1024,
        max_concurrency: 2,
    })
    .unwrap();
    let cursor = RssCursor::new(
        Some("\"feed-v1\"".to_owned()),
        Some("Thu, 23 Jul 2026 01:02:03 GMT".to_owned()),
    )
    .unwrap();

    let response = client
        .fetch(&secret_url(&server, "feed"), Some(&cursor))
        .await
        .unwrap();
    let FeedResponse::Modified {
        body,
        cursor: returned_cursor,
        ..
    } = response
    else {
        panic!("modified response")
    };
    assert_eq!(body, RSS);
    assert_eq!(returned_cursor.unwrap().etag(), Some("\"feed-v2\""));
    let (if_none_match, if_modified_since) = {
        let captured = captured.lock().unwrap();
        (
            captured[0][header::IF_NONE_MATCH]
                .to_str()
                .unwrap()
                .to_owned(),
            captured[0][header::IF_MODIFIED_SINCE]
                .to_str()
                .unwrap()
                .to_owned(),
        )
    };
    assert_eq!(if_none_match, "\"feed-v1\"");
    assert_eq!(if_modified_since, "Thu, 23 Jul 2026 01:02:03 GMT");

    assert!(matches!(
        client
            .fetch(&secret_url(&server, "not-modified"), Some(&cursor))
            .await,
        Ok(FeedResponse::NotModified { .. })
    ));
    assert_eq!(
        client.fetch(&secret_url(&server, "redirect"), None).await,
        Err(FeedClientError::InvalidResponse)
    );
    assert_eq!(
        destination_calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        client.fetch(&secret_url(&server, "rate"), None).await,
        Err(FeedClientError::RateLimited)
    );
    assert_eq!(
        client
            .fetch(&secret_url(&server, "unauthorized"), None)
            .await,
        Err(FeedClientError::Unauthorized)
    );
    assert_eq!(
        client
            .fetch(&secret_url(&server, "server-error"), None)
            .await,
        Err(FeedClientError::Unavailable)
    );
    assert_eq!(
        client.fetch(&secret_url(&server, "large"), None).await,
        Err(FeedClientError::ResponseTooLarge)
    );
    assert_eq!(
        client.fetch(&secret_url(&server, "stalled"), None).await,
        Err(FeedClientError::Timeout)
    );
}

fn source(id: Uuid) -> FeedPollSource {
    FeedPollSource {
        id,
        config_version: 1,
        downloader_connection_id: Uuid::now_v7(),
        feed_url: SecretBytes::new(b"https://feed.example.test/private?token=private".to_vec()),
        cursor: None,
    }
}

fn secret_url(server: &TestServer, path: &str) -> SecretBytes {
    SecretBytes::new(server.origin.join(path).unwrap().to_string().into_bytes())
}

struct TestServer {
    origin: url::Url,
    task: tokio::task::JoinHandle<()>,
}

impl TestServer {
    async fn spawn(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            origin: url::Url::parse(&format!("http://{address}/")).unwrap(),
            task,
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn _assert_secret_draft(_: AutomationEventDraft) {}
