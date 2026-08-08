CREATE TABLE catalog_media_items (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    library_id BLOB NOT NULL CHECK(length(library_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('movie','series','generic-video')),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 512),
    title_normalized TEXT NOT NULL CHECK(length(title_normalized) BETWEEN 1 AND 1024),
    year INTEGER CHECK(year IS NULL OR year BETWEEN 1870 AND 9999),
    local_status TEXT NOT NULL CHECK(local_status IN ('complete','partial')),
    nfo_status TEXT NOT NULL CHECK(nfo_status IN ('not-requested','complete','partial','failed')),
    projection_version INTEGER NOT NULL CHECK(projection_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

CREATE INDEX catalog_media_list_idx
ON catalog_media_items(account_id,updated_at_us DESC,id DESC);
CREATE INDEX catalog_media_type_list_idx
ON catalog_media_items(account_id,kind,updated_at_us DESC,id DESC);
CREATE INDEX catalog_media_library_list_idx
ON catalog_media_items(account_id,library_id,updated_at_us DESC,id DESC);
CREATE INDEX catalog_media_status_list_idx
ON catalog_media_items(account_id,local_status,updated_at_us DESC,id DESC);
CREATE INDEX catalog_media_title_idx
ON catalog_media_items(account_id,title_normalized,updated_at_us DESC,id DESC);

CREATE TABLE catalog_media_nodes (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    parent_id BLOB CHECK(parent_id IS NULL OR length(parent_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('season','episode','generic-video-item')),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 512),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99999),
    UNIQUE(id,media_item_id),
    UNIQUE(media_item_id,parent_id,kind,ordinal,id),
    FOREIGN KEY(media_item_id) REFERENCES catalog_media_items(id) ON DELETE CASCADE,
    FOREIGN KEY(parent_id,media_item_id) REFERENCES catalog_media_nodes(id,media_item_id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX catalog_nodes_item_order_idx
ON catalog_media_nodes(media_item_id,parent_id,kind,ordinal,id);

CREATE TABLE catalog_media_versions (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    owner_node_id BLOB CHECK(owner_node_id IS NULL OR length(owner_node_id)=16),
    label TEXT CHECK(label IS NULL OR length(label)<=128),
    UNIQUE(id,media_item_id),
    FOREIGN KEY(media_item_id) REFERENCES catalog_media_items(id) ON DELETE CASCADE,
    FOREIGN KEY(owner_node_id,media_item_id) REFERENCES catalog_media_nodes(id,media_item_id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX catalog_versions_owner_idx
ON catalog_media_versions(media_item_id,owner_node_id,id);

CREATE TABLE catalog_file_assets (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    file_revision_id BLOB NOT NULL CHECK(length(file_revision_id)=16),
    source_relative_path TEXT NOT NULL CHECK(length(source_relative_path) BETWEEN 1 AND 4096),
    current_relative_path TEXT NOT NULL CHECK(length(current_relative_path) BETWEEN 1 AND 4096),
    size_bytes INTEGER NOT NULL CHECK(size_bytes>=0),
    UNIQUE(id,media_item_id),
    UNIQUE(account_id,file_revision_id),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(media_item_id) REFERENCES catalog_media_items(id) ON DELETE CASCADE,
    FOREIGN KEY(file_revision_id) REFERENCES discovery_file_revisions(id)
) STRICT;

CREATE INDEX catalog_file_assets_item_idx
ON catalog_file_assets(media_item_id,id);

CREATE TABLE catalog_version_files (
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    version_id BLOB NOT NULL CHECK(length(version_id)=16),
    file_asset_id BLOB NOT NULL CHECK(length(file_asset_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 31),
    PRIMARY KEY(version_id,file_asset_id),
    UNIQUE(version_id,ordinal),
    FOREIGN KEY(version_id,media_item_id) REFERENCES catalog_media_versions(id,media_item_id)
        ON DELETE CASCADE,
    FOREIGN KEY(file_asset_id,media_item_id) REFERENCES catalog_file_assets(id,media_item_id)
        ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

CREATE TABLE catalog_metadata_values (
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    field TEXT NOT NULL CHECK(length(field) BETWEEN 1 AND 64),
    value TEXT CHECK(value IS NULL OR length(value)<=4096),
    state TEXT NOT NULL CHECK(state IN ('present','missing')),
    source_type TEXT CHECK(source_type IS NULL OR source_type IN ('tmdb','nfo','manual','system')),
    source_id TEXT CHECK(source_id IS NULL OR length(source_id)<=128),
    source_version TEXT CHECK(source_version IS NULL OR length(source_version)<=64),
    PRIMARY KEY(media_item_id,field),
    CHECK((state='missing')=(value IS NULL)),
    CHECK((state='missing')=(source_type IS NULL)),
    CHECK(source_type IS NOT NULL OR (source_id IS NULL AND source_version IS NULL)),
    FOREIGN KEY(media_item_id) REFERENCES catalog_media_items(id) ON DELETE CASCADE
) WITHOUT ROWID, STRICT;

CREATE TABLE catalog_artwork_refs (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('poster','backdrop')),
    state TEXT NOT NULL CHECK(state IN ('available','missing')),
    local_relative_path TEXT CHECK(
        local_relative_path IS NULL OR length(local_relative_path) BETWEEN 1 AND 4096
    ),
    UNIQUE(media_item_id,kind),
    CHECK((state='available')=(local_relative_path IS NOT NULL)),
    FOREIGN KEY(media_item_id) REFERENCES catalog_media_items(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE catalog_applied_local_results (
    result_id BLOB PRIMARY KEY NOT NULL CHECK(length(result_id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    request_sha256 BLOB NOT NULL CHECK(length(request_sha256)=32),
    applied_at_us INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(media_item_id) REFERENCES catalog_media_items(id)
) STRICT;

CREATE INDEX catalog_results_media_idx
ON catalog_applied_local_results(media_item_id,applied_at_us,result_id);
CREATE INDEX catalog_results_task_idx
ON catalog_applied_local_results(account_id,task_id,result_id);

CREATE TABLE catalog_media_order_history (
    revision INTEGER PRIMARY KEY AUTOINCREMENT,
    media_item_id BLOB NOT NULL CHECK(length(media_item_id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    library_id BLOB NOT NULL CHECK(length(library_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('movie','series','generic-video')),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 512),
    title_normalized TEXT NOT NULL CHECK(length(title_normalized) BETWEEN 1 AND 1024),
    year INTEGER CHECK(year IS NULL OR year BETWEEN 1870 AND 9999),
    local_status TEXT NOT NULL CHECK(local_status IN ('complete','partial')),
    artwork_id BLOB CHECK(artwork_id IS NULL OR length(artwork_id)=16),
    artwork_kind TEXT CHECK(artwork_kind IS NULL OR artwork_kind IN ('poster','backdrop')),
    artwork_state TEXT CHECK(artwork_state IS NULL OR artwork_state IN ('available','missing')),
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(media_item_id) REFERENCES catalog_media_items(id) ON DELETE CASCADE,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

CREATE INDEX catalog_order_snapshot_idx
ON catalog_media_order_history(account_id,revision,kind,library_id,local_status,updated_at_us,media_item_id);
CREATE INDEX catalog_order_item_revision_idx
ON catalog_media_order_history(media_item_id,revision DESC);

CREATE TRIGGER catalog_order_after_insert
AFTER INSERT ON catalog_media_items
BEGIN
    INSERT INTO catalog_media_order_history
    (media_item_id,account_id,library_id,kind,title,title_normalized,year,local_status,
     artwork_id,artwork_kind,artwork_state,updated_at_us)
    SELECT NEW.id,NEW.account_id,NEW.library_id,NEW.kind,NEW.title,NEW.title_normalized,
           NEW.year,NEW.local_status,art.id,art.kind,art.state,NEW.updated_at_us
    FROM (SELECT 1) seed
    LEFT JOIN catalog_artwork_refs art
      ON art.media_item_id=NEW.id AND art.kind='poster';
END;

CREATE TRIGGER catalog_order_after_update
AFTER UPDATE OF updated_at_us ON catalog_media_items
BEGIN
    INSERT INTO catalog_media_order_history
    (media_item_id,account_id,library_id,kind,title,title_normalized,year,local_status,
     artwork_id,artwork_kind,artwork_state,updated_at_us)
    SELECT NEW.id,NEW.account_id,NEW.library_id,NEW.kind,NEW.title,NEW.title_normalized,
           NEW.year,NEW.local_status,art.id,art.kind,art.state,NEW.updated_at_us
    FROM (SELECT 1) seed
    LEFT JOIN catalog_artwork_refs art
      ON art.media_item_id=NEW.id AND art.kind='poster';
END;
