CREATE TABLE discovery_inbox_directories (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    root_id TEXT NOT NULL,
    relative_path_bytes BLOB NOT NULL,
    relative_path_display TEXT NOT NULL,
    root_identity BLOB NOT NULL,
    directory_identity BLOB NOT NULL,
    health TEXT NOT NULL CHECK(health IN ('available', 'unavailable')),
    last_checked_at_us INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(root_id, relative_path_bytes)
) STRICT;

CREATE INDEX discovery_inbox_root_path_idx
    ON discovery_inbox_directories(root_id, relative_path_bytes, id);

PRAGMA user_version = 3;
