mod common;
mod tmdb_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::connectors::model::{CacheStatus, ProviderMediaKind, TmdbSearchRequest};
use mediaflow_core::connectors::tmdb::{
    TmdbCachePolicy, TmdbClient, TmdbClientOptions, TmdbProvider,
};
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::secrets::SecretBytes;
use serde_json::{Value, json};
use tmdb_support::FakeTmdb;

#[tokio::test]
async fn identical_searches_single_flight_then_use_fresh_and_stale_cache() {
    let state = Arc::new(CacheServerState::default());
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/search/movie", get(search))
            .with_state(state.clone()),
    )
    .await;
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
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
    let provider = TmdbProvider::new(
        db.pool().clone(),
        client,
        TmdbCachePolicy {
            search_fresh: Duration::from_millis(40),
            detail_fresh: Duration::from_millis(40),
            negative_fresh: Duration::from_millis(20),
            stale_grace: Duration::from_secs(1),
        },
    );
    let request = TmdbSearchRequest {
        media_kind: ProviderMediaKind::Movie,
        title: "Dune".to_owned(),
        year: Some(2021),
        locale: "en-US".to_owned(),
        region: None,
    };

    let mut tasks = Vec::new();
    for _ in 0..20 {
        let provider = provider.clone();
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            provider
                .search(
                    &SecretBytes::new(b"cache-token-never-log".to_vec()),
                    &request,
                )
                .await
        }));
    }
    for task in tasks {
        let candidates = task.await.unwrap().unwrap();
        assert_eq!(candidates[0].identity.provider_id, 438_631);
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);

    let fresh = provider
        .search(
            &SecretBytes::new(b"cache-token-never-log".to_vec()),
            &request,
        )
        .await
        .unwrap();
    assert_eq!(fresh[0].cache_status, CacheStatus::Fresh);
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);

    tokio::time::sleep(Duration::from_millis(60)).await;
    state.mode.store(1, Ordering::SeqCst);
    let stale_candidates = provider
        .search(
            &SecretBytes::new(b"cache-token-never-log".to_vec()),
            &request,
        )
        .await
        .unwrap();
    assert_eq!(stale_candidates[0].cache_status, CacheStatus::Stale);
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);

    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM connectors_tmdb_cache")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn negative_results_use_the_short_negative_cache_ttl() {
    let state = Arc::new(CacheServerState {
        mode: AtomicU8::new(2),
        ..CacheServerState::default()
    });
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/search/movie", get(search))
            .with_state(state.clone()),
    )
    .await;
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let provider = TmdbProvider::new(
        db.pool().clone(),
        TmdbClient::for_test_loopback(server.origin.clone(), TmdbClientOptions::default()).unwrap(),
        TmdbCachePolicy::default(),
    );
    let request = TmdbSearchRequest {
        media_kind: ProviderMediaKind::Movie,
        title: "No Such Movie".to_owned(),
        year: None,
        locale: "en-US".to_owned(),
        region: None,
    };

    for _ in 0..2 {
        assert!(
            provider
                .search(
                    &SecretBytes::new(b"negative-cache-token".to_vec()),
                    &request
                )
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT outcome FROM connectors_tmdb_cache LIMIT 1")
            .fetch_one(db.pool())
            .await
            .unwrap(),
        "not-found"
    );
}

#[tokio::test]
async fn cached_detail_is_available_during_a_temporary_provider_outage() {
    let state = Arc::new(CacheServerState::default());
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/movie/{id}", get(details))
            .with_state(state.clone()),
    )
    .await;
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let provider = TmdbProvider::new(
        db.pool().clone(),
        TmdbClient::for_test_loopback(server.origin.clone(), TmdbClientOptions::default()).unwrap(),
        TmdbCachePolicy {
            detail_fresh: Duration::from_millis(20),
            stale_grace: Duration::from_secs(1),
            ..TmdbCachePolicy::default()
        },
    );

    let first = provider
        .details(
            &SecretBytes::new(b"detail-cache-token".to_vec()),
            ProviderMediaKind::Movie,
            129,
            "zh-CN",
        )
        .await
        .unwrap();
    assert_eq!(first.summaries.len(), 2);
    assert_eq!(state.calls.load(Ordering::SeqCst), 3);

    tokio::time::sleep(Duration::from_millis(30)).await;
    state.mode.store(1, Ordering::SeqCst);
    let stale_detail = provider
        .details(
            &SecretBytes::new(b"detail-cache-token".to_vec()),
            ProviderMediaKind::Movie,
            129,
            "zh-CN",
        )
        .await
        .unwrap();
    assert_eq!(stale_detail.cache_status, CacheStatus::Stale);
    assert_eq!(stale_detail.identity.provider_id, 129);
    assert_eq!(state.calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn cancelling_the_single_flight_owner_does_not_leave_a_stuck_query_key() {
    let state = Arc::new(CacheServerState {
        mode: AtomicU8::new(3),
        ..CacheServerState::default()
    });
    let server = FakeTmdb::spawn(
        Router::new()
            .route("/3/search/movie", get(search))
            .with_state(state.clone()),
    )
    .await;
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let provider = TmdbProvider::new(
        db.pool().clone(),
        TmdbClient::for_test_loopback(server.origin.clone(), TmdbClientOptions::default()).unwrap(),
        TmdbCachePolicy::default(),
    );
    let request = TmdbSearchRequest {
        media_kind: ProviderMediaKind::Movie,
        title: "Cancelled".to_owned(),
        year: None,
        locale: "en-US".to_owned(),
        region: None,
    };
    let owner = {
        let provider = provider.clone();
        let request = request.clone();
        tokio::spawn(async move {
            provider
                .search(
                    &SecretBytes::new(b"cancelled-flight-token".to_vec()),
                    &request,
                )
                .await
        })
    };
    while state.calls.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    owner.abort();
    let _ = owner.await;
    state.mode.store(0, Ordering::SeqCst);

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        provider.search(
            &SecretBytes::new(b"cancelled-flight-token".to_vec()),
            &request,
        ),
    )
    .await
    .expect("single-flight key released")
    .unwrap();
    assert_eq!(result[0].identity.provider_id, 438_631);
}

#[derive(Default)]
struct CacheServerState {
    calls: AtomicUsize,
    mode: AtomicU8,
}

async fn search(State(state): State<Arc<CacheServerState>>) -> Result<Json<Value>, StatusCode> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    if state.mode.load(Ordering::SeqCst) == 3 {
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    match state.mode.load(Ordering::SeqCst) {
        0 => Ok(Json(json!({
            "page":1,
            "total_pages":1,
            "results":[{
                "id":438_631,
                "title":"Dune",
                "original_title":"Dune",
                "original_language":"en",
                "release_date":"2021-10-22",
                "overview":""
            }]
        }))),
        1 => Err(StatusCode::SERVICE_UNAVAILABLE),
        _ => Ok(Json(json!({"page":1,"total_pages":1,"results":[]}))),
    }
}

#[derive(serde::Deserialize)]
struct DetailQuery {
    language: String,
}

async fn details(
    State(state): State<Arc<CacheServerState>>,
    Path(id): Path<i64>,
    Query(query): Query<DetailQuery>,
) -> Result<Json<Value>, StatusCode> {
    state.calls.fetch_add(1, Ordering::SeqCst);
    if state.mode.load(Ordering::SeqCst) == 1 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    if id != 129 {
        return Err(StatusCode::NOT_FOUND);
    }
    let (title, language, overview) = match query.language.as_str() {
        "zh-CN" => ("千与千寻", "ja", ""),
        "ja-JP" => ("千と千尋の神隠し", "ja", "少女が神々の世界へ迷い込む物語。"),
        "en-US" => ("Spirited Away", "ja", "A young girl enters a spirit world."),
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    Ok(Json(json!({
        "id":129,
        "title":title,
        "original_title":"千と千尋の神隠し",
        "original_language":language,
        "release_date":"2001-07-20",
        "overview":overview
    })))
}
