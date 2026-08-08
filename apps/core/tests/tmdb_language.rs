mod tmdb_support;

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use mediaflow_core::connectors::model::{FieldLanguageSource, ProviderMediaKind};
use mediaflow_core::connectors::tmdb::{TmdbClient, TmdbClientOptions};
use mediaflow_core::platform::secrets::SecretBytes;
use serde::Deserialize;
use serde_json::Value;
use tmdb_support::{FakeTmdb, RequestProbe};

const ZH: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/tmdb/detail-movie-zh-CN.json"
));
const JA: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/tmdb/detail-movie-ja-JP.json"
));
const EN: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/tmdb/detail-movie-en-US.json"
));

#[tokio::test]
async fn details_merge_preferred_original_and_english_fields_with_explicit_sources() {
    let probe = Arc::new(RequestProbe::default());
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/movie/{id}", get(movie_details))
            .with_state(probe.clone()),
    )
    .await;
    let client = TmdbClient::for_test_loopback(
        server.origin.clone(),
        TmdbClientOptions {
            connect_timeout: Duration::from_secs(1),
            request_timeout: Duration::from_secs(2),
            max_response_bytes: 2 * 1024 * 1024,
            max_concurrency: 4,
        },
    )
    .unwrap();

    let candidate = client
        .details_with_language_fallback(
            &SecretBytes::new(b"language-token-not-for-output".to_vec()),
            ProviderMediaKind::Movie,
            129,
            "zh-CN",
        )
        .await
        .unwrap();

    assert_eq!(candidate.titles.len(), 3);
    assert_eq!(candidate.titles[0].value, "千与千寻");
    assert_eq!(candidate.titles[0].language, "zh-CN");
    assert_eq!(candidate.titles[0].source, FieldLanguageSource::Preferred);
    assert_eq!(candidate.titles[1].value, "千と千尋の神隠し");
    assert_eq!(candidate.titles[1].language, "ja-JP");
    assert_eq!(candidate.titles[1].source, FieldLanguageSource::Original);
    assert_eq!(candidate.titles[2].value, "Spirited Away");
    assert_eq!(candidate.titles[2].source, FieldLanguageSource::English);
    assert_eq!(candidate.summaries.len(), 2);
    assert_eq!(candidate.summaries[0].source, FieldLanguageSource::Original);
    assert_eq!(candidate.summaries[1].source, FieldLanguageSource::English);
    assert_eq!(probe.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[derive(Deserialize)]
struct LanguageQuery {
    language: String,
}

async fn movie_details(
    State(probe): State<Arc<RequestProbe>>,
    Path(id): Path<i64>,
    Query(query): Query<LanguageQuery>,
) -> Result<Json<Value>, StatusCode> {
    let _active = probe.enter();
    if id != 129 {
        return Err(StatusCode::NOT_FOUND);
    }
    let fixture = match query.language.as_str() {
        "zh-CN" => ZH,
        "ja-JP" => JA,
        "en-US" => EN,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    Ok(Json(serde_json::from_str(fixture).unwrap()))
}
