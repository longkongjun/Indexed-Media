CREATE TABLE organization_config_snapshots (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    target_id BLOB NOT NULL CHECK(length(target_id)=16),
    target_config_version INTEGER NOT NULL CHECK(target_config_version>=1),
    kind TEXT NOT NULL CHECK(kind IN ('movie','series','generic-video')),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 100),
    root_id TEXT NOT NULL CHECK(length(root_id) BETWEEN 1 AND 64),
    relative_path_display TEXT NOT NULL CHECK(length(relative_path_display) BETWEEN 1 AND 4096),
    operation TEXT NOT NULL CHECK(operation IN ('move','copy','hardlink')),
    naming_pattern TEXT NOT NULL CHECK(naming_pattern IN ('movie','series','generic-numbered')),
    nfo_policy TEXT NOT NULL CHECK(nfo_policy IN ('preserve-only','generate-missing')),
    automatic INTEGER NOT NULL CHECK(automatic IN (0,1)),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    created_at_us INTEGER NOT NULL
) STRICT;

CREATE TABLE organization_config_snapshot_rules (
    snapshot_id BLOB NOT NULL CHECK(length(snapshot_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 99),
    media_kind TEXT NOT NULL CHECK(media_kind IN ('movie','series','generic-video')),
    inbox_directory_id BLOB CHECK(inbox_directory_id IS NULL OR length(inbox_directory_id)=16),
    explicit_tag TEXT CHECK(explicit_tag IS NULL OR length(explicit_tag) BETWEEN 1 AND 100),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    PRIMARY KEY(snapshot_id,ordinal),
    FOREIGN KEY(snapshot_id) REFERENCES organization_config_snapshots(id)
) WITHOUT ROWID, STRICT;

CREATE TABLE organization_config_provenance (
    snapshot_id BLOB NOT NULL CHECK(length(snapshot_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 63),
    field_name TEXT NOT NULL CHECK(length(field_name) BETWEEN 1 AND 64),
    source_kind TEXT NOT NULL CHECK(source_kind IN ('target','profile','rule','manual-decision')),
    source_id BLOB CHECK(source_id IS NULL OR length(source_id)=16),
    source_version INTEGER NOT NULL CHECK(source_version>=1),
    PRIMARY KEY(snapshot_id,ordinal),
    FOREIGN KEY(snapshot_id) REFERENCES organization_config_snapshots(id)
) WITHOUT ROWID, STRICT;

CREATE TABLE organization_plans (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    file_revision_id BLOB NOT NULL CHECK(length(file_revision_id)=16),
    selected_identity_id BLOB CHECK(selected_identity_id IS NULL OR length(selected_identity_id)=16),
    version INTEGER NOT NULL CHECK(version>=1),
    target_id BLOB NOT NULL CHECK(length(target_id)=16),
    snapshot_id BLOB NOT NULL UNIQUE CHECK(length(snapshot_id)=16),
    source_root_id TEXT NOT NULL CHECK(length(source_root_id) BETWEEN 1 AND 64),
    source_relative_path TEXT NOT NULL CHECK(length(source_relative_path) BETWEEN 1 AND 4096),
    destination_root_id TEXT NOT NULL CHECK(length(destination_root_id) BETWEEN 1 AND 64),
    destination_relative_path TEXT NOT NULL CHECK(length(destination_relative_path) BETWEEN 1 AND 4096),
    operation TEXT NOT NULL CHECK(operation IN ('move','copy','hardlink')),
    naming TEXT NOT NULL CHECK(length(naming) BETWEEN 1 AND 512),
    authorization TEXT NOT NULL CHECK(authorization IN ('automatic','one-time','paused')),
    risk_codes_json TEXT NOT NULL CHECK(json_valid(risk_codes_json) AND length(risk_codes_json)<=4096),
    config_fingerprint BLOB NOT NULL CHECK(length(config_fingerprint)=32),
    created_at_us INTEGER NOT NULL,
    UNIQUE(task_id,version),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(file_revision_id) REFERENCES discovery_file_revisions(id),
    FOREIGN KEY(snapshot_id) REFERENCES organization_config_snapshots(id)
) STRICT;

CREATE INDEX organization_plans_current_idx
ON organization_plans(account_id,task_id,version DESC,id DESC);

CREATE TABLE organization_plan_operations (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    plan_id BLOB NOT NULL CHECK(length(plan_id)=16),
    ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 63),
    kind TEXT NOT NULL CHECK(kind IN ('file','ensure-missing-nfo')),
    destination_root_id TEXT NOT NULL CHECK(length(destination_root_id) BETWEEN 1 AND 64),
    destination_relative_path TEXT NOT NULL CHECK(length(destination_relative_path) BETWEEN 1 AND 4096),
    UNIQUE(plan_id,ordinal),
    FOREIGN KEY(plan_id) REFERENCES organization_plans(id)
) STRICT;

CREATE TABLE organization_plan_recalculation_receipts (
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    idempotency_key_sha256 BLOB NOT NULL CHECK(length(idempotency_key_sha256)=32),
    request_digest BLOB NOT NULL CHECK(length(request_digest)=32),
    result_plan_id BLOB NOT NULL CHECK(length(result_plan_id)=16),
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(account_id,idempotency_key_sha256),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(result_plan_id) REFERENCES organization_plans(id)
) WITHOUT ROWID, STRICT;

CREATE TABLE organization_plan_authorization_receipts (
    plan_id BLOB PRIMARY KEY NOT NULL CHECK(length(plan_id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    plan_version INTEGER NOT NULL CHECK(plan_version>=1),
    idempotency_key_sha256 BLOB NOT NULL CHECK(length(idempotency_key_sha256)=32),
    created_at_us INTEGER NOT NULL,
    UNIQUE(account_id,idempotency_key_sha256),
    FOREIGN KEY(plan_id) REFERENCES organization_plans(id),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id)
) STRICT;

CREATE TRIGGER organization_config_snapshots_immutable
BEFORE UPDATE ON organization_config_snapshots
BEGIN SELECT RAISE(ABORT, 'organization config snapshot is immutable'); END;

CREATE TRIGGER organization_config_snapshot_rules_immutable
BEFORE UPDATE ON organization_config_snapshot_rules
BEGIN SELECT RAISE(ABORT, 'organization config snapshot rule is immutable'); END;

CREATE TRIGGER organization_config_provenance_immutable
BEFORE UPDATE ON organization_config_provenance
BEGIN SELECT RAISE(ABORT, 'organization config provenance is immutable'); END;

CREATE TRIGGER organization_plans_immutable
BEFORE UPDATE ON organization_plans
BEGIN SELECT RAISE(ABORT, 'organization plan is immutable'); END;

CREATE TRIGGER organization_plan_operations_immutable
BEFORE UPDATE ON organization_plan_operations
BEGIN SELECT RAISE(ABORT, 'organization plan operation is immutable'); END;

PRAGMA user_version = 22;
