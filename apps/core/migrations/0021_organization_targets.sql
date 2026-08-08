CREATE TABLE organization_targets (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('movie','series','generic-video')),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 100),
    root_id TEXT NOT NULL CHECK(length(root_id) BETWEEN 1 AND 64),
    relative_path_bytes BLOB NOT NULL CHECK(length(relative_path_bytes) BETWEEN 1 AND 4096),
    relative_path_display TEXT NOT NULL CHECK(length(relative_path_display) BETWEEN 1 AND 4096),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(account_id,root_id,relative_path_bytes),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

CREATE INDEX organization_targets_list_idx
ON organization_targets(account_id,updated_at_us DESC,id DESC);

CREATE TABLE organization_profiles (
    target_id BLOB PRIMARY KEY NOT NULL CHECK(length(target_id)=16),
    operation TEXT NOT NULL CHECK(operation IN ('move','copy','hardlink')),
    naming_pattern TEXT NOT NULL CHECK(naming_pattern IN ('movie','series','generic-numbered')),
    nfo_policy TEXT NOT NULL CHECK(nfo_policy IN ('preserve-only','generate-missing')),
    automatic INTEGER NOT NULL CHECK(automatic IN (0,1)),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(target_id) REFERENCES organization_targets(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE organization_rules (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    target_id BLOB NOT NULL CHECK(length(target_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99),
    media_kind TEXT NOT NULL CHECK(media_kind IN ('movie','series','generic-video')),
    inbox_directory_id BLOB CHECK(inbox_directory_id IS NULL OR length(inbox_directory_id)=16),
    explicit_tag TEXT CHECK(explicit_tag IS NULL OR length(explicit_tag) BETWEEN 1 AND 100),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(target_id,ordinal),
    FOREIGN KEY(target_id) REFERENCES organization_targets(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX organization_rules_target_idx
ON organization_rules(target_id,ordinal);

PRAGMA user_version = 21;
