mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::TestConfigDir;
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use tower::ServiceExt;

#[tokio::test]
async fn posters_require_authentication_before_resource_or_provider_lookup() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let app = build_router(fixture.config().clone(), Some(db));
    let response = app
        .oneshot(
            Request::get("/api/v1/media-items/00000000-0000-7000-8000-0000000000ff/artwork")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Json;
use axum::body::to_bytes;
use axum::extract::Path as ExtractPath;
use axum::http::{HeaderMap, header};
use axum::response::IntoResponse;
use axum::routing::get;
use base64::Engine as _;
use mediaflow_core::catalog::artwork::ArtworkService;
use mediaflow_core::catalog::model::{
    LocalStatus, MediaItemKind, MetadataFieldState, MetadataSource, MetadataSourceType, NfoStatus,
    VerifiedFileAsset, VerifiedLocalResult, VerifiedMediaTree, VerifiedMediaVersion,
    VerifiedMetadataValue,
};
use mediaflow_core::catalog::store::CatalogStore;
use mediaflow_core::connectors::model::{
    ProviderError, ProviderMediaKind, SecretString, TmdbConfigCommand,
};
use mediaflow_core::connectors::service::ConnectorService;
use mediaflow_core::connectors::tmdb::{TmdbClient, TmdbClientOptions};
use mediaflow_core::identity::IdentityService;
use mediaflow_core::identity::model::DeviceAccessMode;
use mediaflow_core::platform::outbox::OutboxNotifier;
use mediaflow_core::platform::secrets::SecretBytes;
use mediaflow_core::shared::error::ErrorCode;
use uuid::Uuid;

const TOKEN: &str = "isolated-artwork-provider-token";

fn png() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=").unwrap()
}

struct Upstream {
    client: TmdbClient,
    metadata_calls: Arc<AtomicUsize>,
    image_calls: Arc<AtomicUsize>,
    redirect_calls: Arc<AtomicUsize>,
    handle: tokio::task::JoinHandle<()>,
}

fn image_response(name: &str, address: std::net::SocketAddr) -> axum::response::Response {
    match name {
        "redirect.png" => (
            StatusCode::FOUND,
            [(header::LOCATION, format!("http://{address}/capture"))],
        )
            .into_response(),
        "wrong.png" => ([(header::CONTENT_TYPE, "text/html")], png()).into_response(),
        "huge.png" => (
            [(header::CONTENT_TYPE, "image/png")],
            vec![0_u8; 2 * 1024 * 1024 + 1],
        )
            .into_response(),
        "missing.png" => StatusCode::NOT_FOUND.into_response(),
        "invalid.png" => (
            [(header::CONTENT_TYPE, "image/png")],
            b"not an image".to_vec(),
        )
            .into_response(),
        _ => ([(header::CONTENT_TYPE, "image/png")], png()).into_response(),
    }
}

impl Upstream {
    async fn start() -> Self {
        let metadata_calls = Arc::new(AtomicUsize::new(0));
        let image_calls = Arc::new(AtomicUsize::new(0));
        let redirect_calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route(
                "/3/movie/{id}",
                get({
                    let calls = metadata_calls.clone();
                    move |ExtractPath(id): ExtractPath<i64>, headers: HeaderMap| {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            assert!(
                                headers.get(header::AUTHORIZATION).is_some_and(
                                    |value| value == format!("Bearer {TOKEN}").as_str()
                                ),
                                "metadata credential must reach only the metadata origin"
                            );
                            let path = match id {
                                2 => Some("http://127.0.0.1/private.png".to_owned()),
                                3 => Some("/../private.png".to_owned()),
                                4 => Some("/redirect.png".to_owned()),
                                5 => Some("/wrong.png".to_owned()),
                                6 => Some("/huge.png".to_owned()),
                                7 => Some("/missing.png".to_owned()),
                                8 => None,
                                9 => Some("/invalid.png".to_owned()),
                                _ => Some(format!("poster-{id}.png")),
                            }
                            .map(|value| {
                                if value.starts_with("poster-") {
                                    format!("/{value}")
                                } else {
                                    value
                                }
                            });
                            Json(serde_json::json!({"id":id,"title":"Fixture","poster_path":path}))
                        }
                    }
                }),
            )
            .route(
                "/t/p/w500/{name}",
                get({
                    let calls = image_calls.clone();
                    move |ExtractPath(name): ExtractPath<String>, headers: HeaderMap| {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            assert!(
                                headers.get(header::AUTHORIZATION).is_none(),
                                "image origin must never receive metadata credentials"
                            );
                            image_response(&name, address)
                        }
                    }
                }),
            )
            .route(
                "/capture",
                get({
                    let calls = redirect_calls.clone();
                    move || {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            StatusCode::OK
                        }
                    }
                }),
            );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = TmdbClient::for_test_loopback(
            format!("http://{address}/3/").parse().unwrap(),
            TmdbClientOptions {
                connect_timeout: Duration::from_secs(1),
                request_timeout: Duration::from_secs(2),
                max_response_bytes: 2 * 1024 * 1024,
                max_concurrency: 4,
            },
        )
        .unwrap();
        Self {
            client,
            metadata_calls,
            image_calls,
            redirect_calls,
            handle,
        }
    }
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

#[tokio::test]
async fn fixed_origin_poster_fetch_rejects_unsafe_paths_redirects_types_and_sizes() {
    let upstream = Upstream::start().await;
    let secret = SecretBytes::new(TOKEN.as_bytes().to_vec());
    let image = upstream
        .client
        .poster(&secret, ProviderMediaKind::Movie, 1, "zh-CN")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(image.content_type, "image/png");
    assert_eq!(image.bytes, png());
    for id in [2, 3, 4, 5, 9] {
        let result = upstream
            .client
            .poster(&secret, ProviderMediaKind::Movie, id, "zh-CN")
            .await;
        assert!(
            matches!(result, Err(ProviderError::InvalidResponse)),
            "unsafe poster must fail closed for fixture {id}"
        );
    }
    assert!(matches!(
        upstream
            .client
            .poster(&secret, ProviderMediaKind::Movie, 6, "zh-CN")
            .await,
        Err(ProviderError::ResponseTooLarge)
    ));
    for id in [7, 8] {
        assert!(
            upstream
                .client
                .poster(&secret, ProviderMediaKind::Movie, id, "zh-CN")
                .await
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(upstream.redirect_calls.load(Ordering::SeqCst), 0);
    assert_eq!(upstream.metadata_calls.load(Ordering::SeqCst), 9);
}

#[tokio::test]
async fn authenticated_poster_is_real_binary_cached_across_instances_and_account_bound() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let media = seed_catalog_identity(db.pool(), account).await;
    let upstream = Upstream::start().await;
    ConnectorService::new(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(upstream.client.clone()),
    )
    .put_tmdb(
        TmdbConfigCommand {
            api_read_access_token: SecretString::new(TOKEN.to_owned()),
            locale: "zh-CN".to_owned(),
            region: None,
        },
        0,
    )
    .await
    .unwrap();
    let identity = IdentityService::new(db.pool().clone(), fixture.config().config_dir.clone());
    let (code, _) = identity.issue_pairing_code(account).await.unwrap();
    let enrolled = identity
        .enroll_with_pairing_code_access(&code, "TV", "isolated", DeviceAccessMode::MediaReadOnly)
        .await
        .unwrap();
    let app = mediaflow_core::catalog::routes::router_with_artwork_client(
        fixture.config().clone(),
        &db,
        OutboxNotifier::new(),
        Some(upstream.client.clone()),
    );
    let request = || {
        Request::get(format!("/api/v1/media-items/{media}/artwork"))
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", enrolled.raw_token),
            )
            .body(Body::empty())
            .unwrap()
    };
    let response = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, max-age=3600"
    );
    let etag = response.headers()[header::ETAG].clone();
    assert_eq!(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
        png()
    );
    let restarted = mediaflow_core::catalog::routes::router_with_artwork_client(
        fixture.config().clone(),
        &db,
        OutboxNotifier::new(),
        Some(upstream.client.clone()),
    );
    let response = restarted.oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::ETAG], etag);
    assert_eq!(upstream.metadata_calls.load(Ordering::SeqCst), 1);
    assert_eq!(upstream.image_calls.load(Ordering::SeqCst), 1);
    let service = ArtworkService::new(
        db.pool().clone(),
        fixture.config().config_dir.clone(),
        upstream.client.clone(),
    );
    let denied = service.poster(Uuid::now_v7(), media).await;
    assert!(matches!(denied,Err(error) if error.code()==ErrorCode::NotFound));
    assert_eq!(upstream.metadata_calls.load(Ordering::SeqCst), 1);
    let catalog = CatalogStore::new(db.pool().clone())
        .detail(account, media)
        .await
        .unwrap();
    assert!(
        catalog.item.artwork_ref.is_none(),
        "derived cache must not claim verified local artwork assets"
    );
    identity
        .revoke_device(enrolled.device.id, "administrator")
        .await
        .unwrap();
    assert_eq!(
        app.oneshot(request()).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_single_cache_entry(&fixture.config().config_dir);
}

fn assert_single_cache_entry(config_dir: &std::path::Path) {
    let cache = config_dir.join("artwork-cache");
    let entries = std::fs::read_dir(cache)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0]
            .path()
            .extension()
            .is_some_and(|extension| extension == "image")
    );
}

#[tokio::test]
async fn poster_cache_rejects_directory_symlinks_and_hardlinked_files_before_reading() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let media = seed_catalog_identity(db.pool(), account).await;
    let upstream = Upstream::start().await;
    ConnectorService::new(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(upstream.client.clone()),
    )
    .put_tmdb(
        TmdbConfigCommand {
            api_read_access_token: SecretString::new(TOKEN.to_owned()),
            locale: "zh-CN".to_owned(),
            region: None,
        },
        0,
    )
    .await
    .unwrap();
    let service = ArtworkService::new(
        db.pool().clone(),
        fixture.config().config_dir.clone(),
        upstream.client.clone(),
    );
    assert!(service.poster(account, media).await.is_ok());
    let cache = fixture.config().config_dir.join("artwork-cache");
    let entry = std::fs::read_dir(&cache)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let outside = tempfile::tempdir().unwrap();
    let sentinel = outside.path().join("private-poster.png");
    std::fs::write(&sentinel, png()).unwrap();
    std::fs::remove_file(&entry).unwrap();
    std::fs::hard_link(&sentinel, &entry).unwrap();
    assert!(service.poster(account, media).await.is_err());
    std::fs::remove_file(&entry).unwrap();
    std::fs::remove_dir(&cache).unwrap();
    std::os::unix::fs::symlink(outside.path(), &cache).unwrap();
    assert!(service.poster(account, media).await.is_err());
    assert_eq!(std::fs::read(&sentinel).unwrap(), png());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 1);
    assert_eq!(upstream.metadata_calls.load(Ordering::SeqCst), 1);
}

async fn seed_catalog_identity(pool: &sqlx::SqlitePool, account: Uuid) -> Uuid {
    let inbox = common::seed_inbox(pool).await;
    let lease = common::seed_processing_lease(
        pool,
        inbox,
        b"incoming/Movie.2021.mkv",
        vec![1],
        "artwork-fixture",
    )
    .await;
    let attempt = Uuid::now_v7();
    let candidate = Uuid::now_v7();
    sqlx::query("INSERT INTO identification_attempts(id,task_id,processing_attempt_id,file_revision_id,ordinal,parser_version,provider_version,rule_version,status,started_at_us,finished_at_us) VALUES (?,?,?,?,1,'1','1',1,'decided',1,2)")
        .bind(attempt.as_bytes().as_slice()).bind(lease.task.id.as_bytes().as_slice()).bind(lease.attempt_id.as_bytes().as_slice()).bind(lease.task.file_revision_id.as_bytes().as_slice()).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO identification_candidates(id,attempt_id,ordinal,provider,media_type,provider_id,locale,ranking_score,provider_version,source_hash,created_at_us) VALUES (?,?,1,'tmdb','movie',1,'zh-CN',100,1,?,1)")
        .bind(candidate.as_bytes().as_slice()).bind(attempt.as_bytes().as_slice()).bind(vec![0_u8;32]).execute(pool).await.unwrap();
    let media = Uuid::now_v7();
    let file = Uuid::now_v7();
    CatalogStore::new(pool.clone())
        .apply_verified_local_result(
            account,
            &VerifiedLocalResult {
                result_id: Uuid::now_v7(),
                task_id: lease.task.id,
                library_id: Uuid::now_v7(),
                media: VerifiedMediaTree {
                    id: media,
                    kind: MediaItemKind::Movie,
                    title: "Fixture movie".to_owned(),
                    year: Some(2021),
                    local_status: LocalStatus::Partial,
                    metadata: vec![VerifiedMetadataValue {
                        field: "title".to_owned(),
                        value: Some("Fixture movie".to_owned()),
                        state: MetadataFieldState::Present,
                        source: Some(MetadataSource {
                            kind: MetadataSourceType::Tmdb,
                            id: Some(candidate.to_string()),
                            version: Some("1".to_owned()),
                        }),
                    }],
                    artwork_refs: vec![],
                    nodes: vec![],
                    versions: vec![VerifiedMediaVersion {
                        id: Uuid::now_v7(),
                        owner_node_id: None,
                        label: None,
                        file_asset_ids: vec![file],
                    }],
                },
                file_assets: vec![VerifiedFileAsset {
                    id: file,
                    file_revision_id: lease.task.file_revision_id,
                    source_relative_path: lease.task.relative_path,
                    current_relative_path: "Movies/Fixture.mkv".to_owned(),
                    size_bytes: 1,
                }],
                nfo_status: NfoStatus::Partial,
            },
            1_000_000_000,
        )
        .await
        .unwrap();
    media
}

#[tokio::test]
async fn available_libraries_expose_only_account_owned_ids_and_safe_names() {
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let media = seed_catalog_identity(db.pool(), account).await;
    let catalog = CatalogStore::new(db.pool().clone());
    let expected_library = catalog
        .detail(account, media)
        .await
        .unwrap()
        .item
        .library_id;
    let listed = catalog.libraries(account).await.unwrap();
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, expected_library);
    assert_eq!(listed.items[0].name, "电影库");
    assert!(
        catalog
            .libraries(Uuid::now_v7())
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let identity = IdentityService::new(db.pool().clone(), fixture.config().config_dir.clone());
    let (code, _) = identity
        .issue_pairing_code_with_access(account, DeviceAccessMode::MediaReadOnly)
        .await
        .unwrap();
    let device = identity
        .enroll_with_pairing_code_access(&code, "TV", "libraries", DeviceAccessMode::MediaReadOnly)
        .await
        .unwrap();
    let app = build_router(fixture.config().clone(), Some(db));
    let response = app
        .oneshot(
            Request::get("/api/v1/media-libraries")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", device.raw_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 16_384).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let item = body["items"][0].as_object().unwrap();
    assert_eq!(
        item.len(),
        2,
        "library read projection must not include paths or policy"
    );
    assert_eq!(item["id"], expected_library.to_string());
    assert_eq!(item["name"], "电影库");
}

#[tokio::test]
async fn full_cache_returns_downloaded_poster_without_persisting_or_removing_entries() {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
    let fixture = TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let media = seed_catalog_identity(db.pool(), account).await;
    let upstream = Upstream::start().await;
    ConnectorService::new(
        db.pool().clone(),
        &fixture.config().config_dir,
        Arc::new(upstream.client.clone()),
    )
    .put_tmdb(
        TmdbConfigCommand {
            api_read_access_token: SecretString::new(TOKEN.to_owned()),
            locale: "zh-CN".to_owned(),
            region: None,
        },
        0,
    )
    .await
    .unwrap();
    let cache = fixture.config().config_dir.join("artwork-cache");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&cache)
        .unwrap();
    for index in 0..512 {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(cache.join(format!("retained-{index}.image")))
            .unwrap();
    }
    let service = ArtworkService::new(
        db.pool().clone(),
        fixture.config().config_dir.clone(),
        upstream.client.clone(),
    );
    for _ in 0..2 {
        let image = service.poster(account, media).await.unwrap();
        assert_eq!(
            image.bytes,
            png(),
            "a full cache must not fail the downloaded response"
        );
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 512);
    }
    assert_eq!(
        upstream.image_calls.load(Ordering::SeqCst),
        2,
        "over-budget image must not be persisted"
    );
    assert_eq!(
        std::fs::metadata(cache.join("retained-0.image"))
            .unwrap()
            .len(),
        0
    );
}
