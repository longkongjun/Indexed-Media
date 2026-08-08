mod tmdb_support;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use mediaflow_core::connectors::model::{ProviderError, ProviderMediaKind, TmdbSearchRequest};
use mediaflow_core::connectors::tmdb::{TmdbClient, TmdbClientOptions};
use mediaflow_core::platform::secrets::SecretBytes;
use serde_json::json;
use tmdb_support::{FakeTmdb, RequestProbe};

const TOKEN: &[u8] = b"test-bearer-token-that-must-not-leak";

#[tokio::test]
async fn movie_search_sends_bearer_only_to_the_configured_origin_and_maps_bounded_results() {
    let probe = Arc::new(RequestProbe::default());
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/search/movie", get(search_movie))
            .with_state(probe.clone()),
    )
    .await;
    let client = test_client(server.origin.clone());

    let candidates = client
        .search(
            &SecretBytes::new(TOKEN.to_vec()),
            &TmdbSearchRequest {
                media_kind: ProviderMediaKind::Movie,
                title: "千与千寻".to_owned(),
                year: Some(2001),
                locale: "zh-CN".to_owned(),
                region: Some("CN".to_owned()),
            },
        )
        .await
        .unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].identity.provider_id, 129);
    assert_eq!(candidates[0].titles[0].value, "千与千寻");
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.authorization.lock().unwrap().as_slice(),
        ["Bearer test-bearer-token-that-must-not-leak"]
    );
    let path = &probe.paths.lock().unwrap()[0];
    assert!(path.starts_with("/3/search/movie?"));
    assert!(path.contains("language=zh-CN"));
    assert!(path.contains("region=CN"));
    assert!(!format!("{client:?}").contains("test-bearer-token"));
}

#[tokio::test]
async fn client_enforces_four_requests_and_classifies_statuses_with_bounded_retry_after() {
    let probe = Arc::new(RequestProbe::default());
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/search/movie", get(slow_search_movie))
            .route(
                "/3/status/unauthorized",
                get(|| async { StatusCode::UNAUTHORIZED }),
            )
            .route(
                "/3/status/forbidden",
                get(|| async { StatusCode::FORBIDDEN }),
            )
            .route("/3/status/rate", get(rate_limited))
            .route(
                "/3/status/server",
                get(|| async { StatusCode::SERVICE_UNAVAILABLE }),
            )
            .route(
                "/3/tv/{id}/season/{season}/episode/{episode}",
                get(episode_details),
            )
            .with_state(probe.clone()),
    )
    .await;
    let client = test_client(server.origin.clone());
    let mut tasks = Vec::new();
    for index in 0..12 {
        let client = client.clone();
        tasks.push(tokio::spawn(async move {
            client
                .search(
                    &SecretBytes::new(TOKEN.to_vec()),
                    &TmdbSearchRequest {
                        media_kind: ProviderMediaKind::Movie,
                        title: format!("movie {index}"),
                        year: None,
                        locale: "en-US".to_owned(),
                        region: None,
                    },
                )
                .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(probe.maximum_active.load(Ordering::SeqCst), 4);

    for path in ["status/unauthorized", "status/forbidden"] {
        assert_eq!(
            client.probe(&SecretBytes::new(TOKEN.to_vec()), path).await,
            Err(ProviderError::CredentialsInvalid)
        );
    }
    let before = chrono::Utc::now().timestamp_micros();
    let rate = client
        .probe(&SecretBytes::new(TOKEN.to_vec()), "status/rate")
        .await
        .unwrap_err();
    let ProviderError::RateLimited { retry_at_us } = rate else {
        panic!("expected rate limit, got {rate:?}");
    };
    assert!((before + 2_000_000..=before + 4_000_000).contains(&retry_at_us));
    assert!(matches!(
        client
            .probe(&SecretBytes::new(TOKEN.to_vec()), "status/server")
            .await,
        Err(ProviderError::TemporarilyUnavailable { .. })
    ));

    let episodes = client
        .verify_episodes(
            &SecretBytes::new(TOKEN.to_vec()),
            100,
            &[(1, 1), (1, 2), (1, 3)],
            "en-US",
        )
        .await
        .unwrap();
    assert_eq!(episodes.len(), 3);
}

#[tokio::test]
async fn redirects_are_not_followed_and_oversized_or_stalled_responses_fail_closed() {
    let destination_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let destination = FakeTmdb::spawn(Router::new().route(
        "/capture",
        get({
            let calls = destination_calls.clone();
            move || async move {
                calls.fetch_add(1, Ordering::SeqCst);
                StatusCode::OK
            }
        }),
    ))
    .await;
    let redirect_target = destination.origin.join("../capture").unwrap().to_string();
    let source = FakeTmdb::spawn(
        Router::new()
            .route(
                "/3/redirect",
                get(move || {
                    let target = redirect_target.clone();
                    async move { (StatusCode::FOUND, [(header::LOCATION, target)]) }
                }),
            )
            .route("/3/large", get(large_response))
            .route("/3/chunked-large", get(chunked_large_response))
            .route("/3/stalled", get(stalled_response)),
    )
    .await;
    let client = TmdbClient::for_test_loopback(
        source.origin.clone(),
        TmdbClientOptions {
            connect_timeout: Duration::from_millis(200),
            request_timeout: Duration::from_millis(100),
            max_response_bytes: 2 * 1024 * 1024,
            max_concurrency: 4,
        },
    )
    .unwrap();

    assert_eq!(
        client
            .probe(&SecretBytes::new(TOKEN.to_vec()), "redirect")
            .await,
        Err(ProviderError::InvalidResponse)
    );
    assert_eq!(destination_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        client
            .probe(&SecretBytes::new(TOKEN.to_vec()), "large")
            .await,
        Err(ProviderError::ResponseTooLarge)
    );
    assert_eq!(
        client
            .probe(&SecretBytes::new(TOKEN.to_vec()), "chunked-large")
            .await,
        Err(ProviderError::ResponseTooLarge)
    );
    assert_eq!(
        client
            .probe(&SecretBytes::new(TOKEN.to_vec()), "stalled")
            .await,
        Err(ProviderError::Timeout)
    );
}

fn test_client(origin: url::Url) -> TmdbClient {
    TmdbClient::for_test_loopback(
        origin,
        TmdbClientOptions {
            connect_timeout: Duration::from_secs(1),
            request_timeout: Duration::from_secs(2),
            max_response_bytes: 2 * 1024 * 1024,
            max_concurrency: 4,
        },
    )
    .unwrap()
}

async fn search_movie(
    State(probe): State<Arc<RequestProbe>>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    capture(&probe, &headers, &uri);
    axum::Json(json!({
        "page":1,
        "total_pages":1,
        "results":[{
            "id":129,
            "title":"千与千寻",
            "original_title":"千と千尋の神隠し",
            "original_language":"ja",
            "release_date":"2001-07-20",
            "overview":""
        }]
    }))
}

async fn slow_search_movie(
    State(probe): State<Arc<RequestProbe>>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    let active = probe.enter();
    capture(&probe, &headers, &uri);
    tokio::time::sleep(Duration::from_millis(40)).await;
    drop(active);
    axum::Json(json!({"page":1,"total_pages":1,"results":[]}))
}

fn capture(probe: &RequestProbe, headers: &HeaderMap, uri: &Uri) {
    probe.calls.fetch_add(1, Ordering::SeqCst);
    probe.authorization.lock().unwrap().push(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    );
    probe.paths.lock().unwrap().push(uri.to_string());
}

async fn rate_limited() -> impl IntoResponse {
    (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "3")])
}

async fn large_response() -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_LENGTH, 2 * 1024 * 1024 + 1)
        .body(Body::from(vec![b'x'; 2 * 1024 * 1024 + 1]))
        .unwrap()
}

async fn stalled_response() -> impl IntoResponse {
    tokio::time::sleep(Duration::from_millis(250)).await;
    axum::Json(json!({"ok":true}))
}

async fn episode_details(Path((_id, season, episode)): Path<(i64, u16, u16)>) -> impl IntoResponse {
    axum::Json(json!({
        "id":1000 + i64::from(episode),
        "season_number":season,
        "episode_number":episode,
        "name":format!("Episode {episode}")
    }))
}

async fn chunked_large_response() -> Response {
    let chunks = (0..3)
        .map(|_| Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(vec![b'x'; 800_000])));
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}
