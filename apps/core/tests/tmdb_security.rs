mod tmdb_support;

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::get;
use axum::{Json, Router};
use mediaflow_core::connectors::model::{
    ExternalIdSource, ProviderError, ProviderMediaKind, TmdbExternalIdRequest,
};
use mediaflow_core::connectors::tmdb::{TmdbClient, TmdbClientBuildError, TmdbClientOptions};
use mediaflow_core::platform::secrets::SecretBytes;
use serde::Deserialize;
use serde_json::json;
use tmdb_support::{FakeTmdb, RequestProbe};

#[test]
fn test_constructor_accepts_only_plain_loopback_origins_and_production_origin_is_fixed() {
    assert_eq!(
        TmdbClient::for_test_loopback(
            url::Url::parse("https://api.themoviedb.org/3/").unwrap(),
            TmdbClientOptions::default(),
        )
        .unwrap_err(),
        TmdbClientBuildError::TestOriginForbidden
    );
    assert_eq!(
        TmdbClient::for_test_loopback(
            url::Url::parse("http://example.test/3/").unwrap(),
            TmdbClientOptions::default(),
        )
        .unwrap_err(),
        TmdbClientBuildError::TestOriginForbidden
    );
    assert_eq!(
        TmdbClient::production().unwrap().origin(),
        "https://api.themoviedb.org/3/"
    );
}

#[tokio::test]
async fn external_id_lookup_uses_find_before_any_search_and_never_exposes_token_in_failures() {
    let probe = Arc::new(RequestProbe::default());
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/find/{external_id}", get(find))
            .with_state(probe.clone()),
    )
    .await;
    let client =
        TmdbClient::for_test_loopback(server.origin.clone(), TmdbClientOptions::default()).unwrap();
    let token = SecretBytes::new(b"external-id-token-sentinel".to_vec());

    let candidates = client
        .find_external(
            &token,
            &TmdbExternalIdRequest {
                source: ExternalIdSource::Imdb,
                external_id: "tt1160419".to_owned(),
                locale: "zh-CN".to_owned(),
            },
        )
        .await
        .unwrap();

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].identity.media_kind, ProviderMediaKind::Movie);
    assert_eq!(candidates[0].identity.provider_id, 438_631);
    {
        let paths = probe.paths.lock().unwrap();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].starts_with("/3/find/tt1160419?"));
        assert!(paths[0].contains("external_source=imdb_id"));
    }

    let invalid = client
        .find_external(
            &token,
            &TmdbExternalIdRequest {
                source: ExternalIdSource::Imdb,
                external_id: "../escape".to_owned(),
                locale: "zh-CN".to_owned(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(invalid, ProviderError::InvalidResponse);
    assert!(!format!("{invalid:?} {client:?} {token:?}").contains("token-sentinel"));
    assert_eq!(probe.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[derive(Deserialize)]
struct FindQuery {
    external_source: String,
    language: String,
}

async fn find(
    State(probe): State<Arc<RequestProbe>>,
    Path(external_id): Path<String>,
    Query(query): Query<FindQuery>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> Result<Json<serde_json::Value>, StatusCode> {
    probe
        .calls
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    probe.paths.lock().unwrap().push(uri.to_string());
    probe.authorization.lock().unwrap().push(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    );
    if external_id != "tt1160419" || query.external_source != "imdb_id" || query.language != "zh-CN"
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(Json(json!({
        "movie_results":[{
            "id":438_631,
            "title":"沙丘",
            "original_title":"Dune",
            "original_language":"en",
            "release_date":"2021-10-22",
            "overview":""
        }],
        "tv_results":[]
    })))
}
