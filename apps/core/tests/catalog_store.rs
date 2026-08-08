#![allow(clippy::too_many_lines)]

mod common;

use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::catalog::model::{
    ArtworkKind, ArtworkState, LocalStatus, MediaItemFilter, MediaItemKind, MediaNodeKind,
    MetadataFieldState, MetadataSource, MetadataSourceType, NfoStatus, VerifiedArtworkRef,
    VerifiedFileAsset, VerifiedLocalResult, VerifiedMediaNode, VerifiedMediaTree,
    VerifiedMediaVersion, VerifiedMetadataValue,
};
use mediaflow_core::catalog::store::CatalogStore;
use mediaflow_core::platform::migrations::migrate_with_backup;
use mediaflow_core::platform::outbox::OutboxReader;
use mediaflow_core::shared::error::ErrorCode;
use mediaflow_core::shared::page::PageRequest;
use mediaflow_core::tasks::events::{CatalogMediaChange, TaskEventEnvelope};
use mediaflow_core::tasks::processing::model::ProcessingLease;
use uuid::Uuid;

#[tokio::test]
async fn movie_result_is_atomic_traceable_idempotent_and_account_scoped() {
    let (fixture, store, account, inbox, lease) = setup(b"incoming/Dune.2021.mkv").await;
    let result = movie_result(&lease, Uuid::now_v7());
    let media_id = store
        .apply_verified_local_result(account, &result, 100_000_000)
        .await
        .unwrap();
    assert_eq!(media_id, result.media.id);

    let page = store
        .list(
            account,
            &MediaItemFilter::default(),
            &PageRequest::new(None, Some(20)).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].title, "Dune");
    assert_eq!(page.items[0].year, Some(2021));
    assert_eq!(
        page.items[0].artwork_ref.as_ref().unwrap().state,
        ArtworkState::Missing
    );

    let detail = store.detail(account, media_id).await.unwrap();
    assert_eq!(detail.versions.len(), 1);
    assert_eq!(detail.versions[0].files.len(), 1);
    assert_eq!(
        detail.versions[0].files[0].source_relative_path,
        "incoming/Dune.2021.mkv"
    );
    assert_eq!(detail.metadata[0].state, MetadataFieldState::Missing);
    assert_eq!(detail.nfo_status, NfoStatus::Failed);
    assert_eq!(detail.related_task_ids, vec![lease.task.id]);

    let events = OutboxReader::new(fixture.pool().clone())
        .after(0, 200)
        .await
        .unwrap();
    let catalog_event = events
        .iter()
        .find(|event| matches!(event, TaskEventEnvelope::CatalogMediaChanged { .. }))
        .unwrap();
    assert!(matches!(
        catalog_event,
        TaskEventEnvelope::CatalogMediaChanged {
            task_id: None,
            payload,
            ..
        } if payload.media_item_id == media_id
            && payload.projection_version == 1
            && payload.change == CatalogMediaChange::Created
    ));
    let raw_payload = sqlx::query_scalar::<_, String>(
        "SELECT payload_json FROM platform_outbox_events
         WHERE event_type='catalog.media-changed' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(fixture.pool())
    .await
    .unwrap();
    assert!(!raw_payload.contains("incoming/Dune"));
    assert!(!raw_payload.contains("Movies/Dune"));

    let outbox_before = table_count(fixture.pool(), "platform_outbox_events").await;
    assert_eq!(
        store
            .apply_verified_local_result(account, &result, 101_000_000)
            .await
            .unwrap(),
        media_id
    );
    assert_eq!(
        table_count(fixture.pool(), "platform_outbox_events").await,
        outbox_before
    );
    assert_eq!(table_count(fixture.pool(), "catalog_file_assets").await, 1);
    assert_eq!(
        table_count(fixture.pool(), "catalog_applied_local_results").await,
        1
    );

    let mut conflicting = result.clone();
    conflicting.media.title = "Different".to_owned();
    assert_eq!(
        store
            .apply_verified_local_result(account, &conflicting, 102_000_000)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::RequestConflict
    );

    let other_account = Uuid::now_v7();
    assert_eq!(
        store
            .detail(other_account, media_id)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::NotFound
    );

    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM catalog_applied_local_results result
             JOIN tasks_processing_tasks task ON task.id=result.task_id
             WHERE result.account_id=task.account_id AND task.inbox_directory_id=?",
        )
        .bind(inbox.as_bytes().as_slice())
        .fetch_one(fixture.pool())
        .await
        .unwrap(),
        1
    );
}

#[tokio::test]
async fn series_hierarchy_reuses_one_physical_file_for_a_multi_episode_version() {
    let (fixture, store, account, _inbox, lease) = setup(b"shows/Example.S01E01-E02.mkv").await;
    let file_id = Uuid::now_v7();
    let season_id = Uuid::now_v7();
    let episode_one = Uuid::now_v7();
    let episode_two = Uuid::now_v7();
    let result = VerifiedLocalResult {
        result_id: Uuid::now_v7(),
        task_id: lease.task.id,
        library_id: Uuid::now_v7(),
        media: VerifiedMediaTree {
            id: Uuid::now_v7(),
            kind: MediaItemKind::Series,
            title: "Example".to_owned(),
            year: None,
            local_status: LocalStatus::Complete,
            metadata: vec![],
            artwork_refs: vec![],
            nodes: vec![
                VerifiedMediaNode {
                    id: season_id,
                    parent_id: None,
                    kind: MediaNodeKind::Season,
                    title: "Season 1".to_owned(),
                    ordinal: 1,
                },
                VerifiedMediaNode {
                    id: episode_one,
                    parent_id: Some(season_id),
                    kind: MediaNodeKind::Episode,
                    title: "Episode 1".to_owned(),
                    ordinal: 1,
                },
                VerifiedMediaNode {
                    id: episode_two,
                    parent_id: Some(season_id),
                    kind: MediaNodeKind::Episode,
                    title: "Episode 2".to_owned(),
                    ordinal: 2,
                },
            ],
            versions: vec![
                VerifiedMediaVersion {
                    id: Uuid::now_v7(),
                    owner_node_id: Some(episode_one),
                    label: None,
                    file_asset_ids: vec![file_id],
                },
                VerifiedMediaVersion {
                    id: Uuid::now_v7(),
                    owner_node_id: Some(episode_two),
                    label: None,
                    file_asset_ids: vec![file_id],
                },
            ],
        },
        file_assets: vec![VerifiedFileAsset {
            id: file_id,
            file_revision_id: lease.task.file_revision_id,
            source_relative_path: lease.task.relative_path.clone(),
            current_relative_path: "TV/Example/Season 01/Example S01E01-E02.mkv".to_owned(),
            size_bytes: 4096,
        }],
        nfo_status: NfoStatus::Complete,
    };
    let media_id = store
        .apply_verified_local_result(account, &result, 110_000_000)
        .await
        .unwrap();
    let detail = store.detail(account, media_id).await.unwrap();
    assert_eq!(detail.children.len(), 3);
    assert!(detail.versions.is_empty());
    assert_eq!(
        detail
            .children
            .iter()
            .find(|child| child.id == episode_one)
            .unwrap()
            .parent_id,
        Some(season_id)
    );
    assert_eq!(
        detail
            .children
            .iter()
            .filter(|child| child.kind == MediaNodeKind::Episode)
            .map(|child| child.versions[0].files[0].id)
            .collect::<Vec<_>>(),
        vec![file_id, file_id]
    );
    assert_eq!(table_count(fixture.pool(), "catalog_file_assets").await, 1);
    assert_eq!(
        table_count(fixture.pool(), "catalog_version_files").await,
        2
    );
}

#[tokio::test]
async fn generic_group_keeps_deterministic_members_without_provider_identity() {
    let (fixture, store, account, _inbox, lease) = setup(b"clips/group/one.mp4").await;
    let mut result = movie_result(&lease, Uuid::now_v7());
    result.media.kind = MediaItemKind::GenericVideo;
    result.media.title = "Family Clips".to_owned();
    result.media.year = None;
    result.media.metadata = vec![VerifiedMetadataValue {
        field: "grouping".to_owned(),
        value: Some("directory".to_owned()),
        state: MetadataFieldState::Present,
        source: Some(MetadataSource {
            kind: MetadataSourceType::System,
            id: Some("relative-parent".to_owned()),
            version: Some("1".to_owned()),
        }),
    }];
    result.media.nodes = vec![VerifiedMediaNode {
        id: Uuid::now_v7(),
        parent_id: None,
        kind: MediaNodeKind::GenericVideoItem,
        title: "one".to_owned(),
        ordinal: 0,
    }];
    result.media.versions[0].owner_node_id = Some(result.media.nodes[0].id);

    let id = store
        .apply_verified_local_result(account, &result, 120_000_000)
        .await
        .unwrap();
    let detail = store.detail(account, id).await.unwrap();
    assert_eq!(detail.item.kind, MediaItemKind::GenericVideo);
    assert_eq!(detail.children[0].ordinal, 0);
    assert_eq!(
        detail.metadata[0].source_type,
        Some(MetadataSourceType::System)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM catalog_metadata_values WHERE source_type='tmdb'",
        )
        .fetch_one(fixture.pool())
        .await
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn invalid_tree_remote_artwork_and_unowned_task_fail_before_catalog_side_effects() {
    let (fixture, store, account, _inbox, lease) = setup(b"invalid/movie.mkv").await;
    let mut remote = movie_result(&lease, Uuid::now_v7());
    remote.media.artwork_refs[0] = VerifiedArtworkRef {
        id: Uuid::now_v7(),
        kind: ArtworkKind::Poster,
        state: ArtworkState::Available,
        local_relative_path: Some("https://image.tmdb.org/poster.jpg".to_owned()),
    };
    assert_eq!(
        store
            .apply_verified_local_result(account, &remote, 130_000_000)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ValidationFailed
    );

    let mut malformed = movie_result(&lease, Uuid::now_v7());
    malformed.media.metadata[0].state = MetadataFieldState::Present;
    assert_eq!(
        store
            .apply_verified_local_result(account, &malformed, 131_000_000)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::ValidationFailed
    );

    let other_account = Uuid::now_v7();
    let unowned = movie_result(&lease, Uuid::now_v7());
    assert_eq!(
        store
            .apply_verified_local_result(other_account, &unowned, 132_000_000)
            .await
            .unwrap_err()
            .code(),
        ErrorCode::NotFound
    );
    assert_eq!(table_count(fixture.pool(), "catalog_media_items").await, 0);
    assert_eq!(
        table_count(fixture.pool(), "catalog_applied_local_results").await,
        0
    );
    assert!(!include_str!("../src/catalog/store.rs").contains("identification_"));
}

async fn setup(path: &[u8]) -> (TestDb, CatalogStore, Uuid, Uuid, ProcessingLease) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = common::seed_account(db.pool()).await;
    let inbox = common::seed_inbox(db.pool()).await;
    let lease =
        common::seed_processing_lease(db.pool(), inbox, path, vec![44], "catalog-test").await;
    let store = CatalogStore::new(db.pool().clone());
    (
        TestDb {
            _fixture: fixture,
            db,
        },
        store,
        account,
        inbox,
        lease,
    )
}

fn movie_result(lease: &ProcessingLease, library_id: Uuid) -> VerifiedLocalResult {
    let file_id = Uuid::now_v7();
    VerifiedLocalResult {
        result_id: Uuid::now_v7(),
        task_id: lease.task.id,
        library_id,
        media: VerifiedMediaTree {
            id: Uuid::now_v7(),
            kind: MediaItemKind::Movie,
            title: "Dune".to_owned(),
            year: Some(2021),
            local_status: LocalStatus::Partial,
            metadata: vec![VerifiedMetadataValue {
                field: "overview".to_owned(),
                value: None,
                state: MetadataFieldState::Missing,
                source: None,
            }],
            artwork_refs: vec![VerifiedArtworkRef {
                id: Uuid::now_v7(),
                kind: ArtworkKind::Poster,
                state: ArtworkState::Missing,
                local_relative_path: None,
            }],
            nodes: vec![],
            versions: vec![VerifiedMediaVersion {
                id: Uuid::now_v7(),
                owner_node_id: None,
                label: Some("4K".to_owned()),
                file_asset_ids: vec![file_id],
            }],
        },
        file_assets: vec![VerifiedFileAsset {
            id: file_id,
            file_revision_id: lease.task.file_revision_id,
            source_relative_path: lease.task.relative_path.clone(),
            current_relative_path: "Movies/Dune (2021)/Dune (2021).mkv".to_owned(),
            size_bytes: 1024,
        }],
        nfo_status: NfoStatus::Failed,
    }
}

struct TestDb {
    _fixture: common::TestConfigDir,
    db: mediaflow_core::platform::db::Db,
}

impl TestDb {
    fn pool(&self) -> &sqlx::SqlitePool {
        self.db.pool()
    }
}

async fn table_count(pool: &sqlx::SqlitePool, table: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(pool)
        .await
        .unwrap()
}
