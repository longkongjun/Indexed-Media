CREATE TABLE organization_plan_nfo_inputs (
    plan_id BLOB PRIMARY KEY NOT NULL CHECK(length(plan_id)=16),
    input_json TEXT NOT NULL CHECK(json_valid(input_json) AND length(input_json)<=2097152),
    FOREIGN KEY(plan_id) REFERENCES organization_plans(id)
) STRICT;

CREATE TRIGGER organization_plan_nfo_inputs_immutable
BEFORE UPDATE ON organization_plan_nfo_inputs
BEGIN SELECT RAISE(ABORT, 'organization plan NFO input is immutable'); END;

CREATE TABLE organization_file_operation_journals (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    plan_id BLOB NOT NULL CHECK(length(plan_id)=16),
    plan_version INTEGER NOT NULL CHECK(plan_version>=1),
    operation_id BLOB NOT NULL CHECK(length(operation_id)=16),
    parent_operation_id BLOB CHECK(parent_operation_id IS NULL OR length(parent_operation_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('copy','move','hardlink','source-removal','nfo')),
    status TEXT NOT NULL CHECK(status IN
        ('prepared','executing','applied','verified','compensated','manual-review')),
    source_root_id TEXT,
    source_relative_path TEXT,
    expected_source_identity BLOB,
    expected_source_size INTEGER CHECK(expected_source_size IS NULL OR expected_source_size>=0),
    expected_source_modified_at_ns INTEGER,
    destination_root_id TEXT NOT NULL,
    destination_relative_path TEXT NOT NULL,
    applied_target_identity BLOB,
    applied_size INTEGER CHECK(applied_size IS NULL OR applied_size>=0),
    applied_sha256 BLOB CHECK(applied_sha256 IS NULL OR length(applied_sha256)=32),
    nfo_preexisting INTEGER CHECK(nfo_preexisting IS NULL OR nfo_preexisting IN (0,1)),
    nfo_outcome TEXT CHECK(nfo_outcome IS NULL OR nfo_outcome IN ('preserved','generated')),
    reason TEXT,
    projection_version INTEGER NOT NULL CHECK(projection_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(plan_id,operation_id),
    CHECK((source_root_id IS NULL)=(source_relative_path IS NULL)),
    CHECK((source_root_id IS NULL)=(expected_source_identity IS NULL)),
    CHECK((source_root_id IS NULL)=(expected_source_size IS NULL)),
    CHECK((source_root_id IS NULL)=(expected_source_modified_at_ns IS NULL)),
    CHECK((applied_target_identity IS NULL)=(applied_size IS NULL)),
    CHECK((applied_target_identity IS NULL)=(applied_sha256 IS NULL)),
    CHECK(kind='nfo' OR (nfo_preexisting IS NULL AND nfo_outcome IS NULL)),
    CHECK(kind!='nfo' OR nfo_preexisting IS NOT NULL),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(plan_id) REFERENCES organization_plans(id)
) STRICT;

CREATE INDEX organization_journals_task_idx
ON organization_file_operation_journals(account_id,task_id,created_at_us,id);

CREATE INDEX organization_journals_recovery_idx
ON organization_file_operation_journals(status,updated_at_us,id)
WHERE status IN ('prepared','executing','applied','manual-review');

CREATE TABLE organization_local_results (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    plan_id BLOB NOT NULL UNIQUE CHECK(length(plan_id)=16),
    plan_version INTEGER NOT NULL CHECK(plan_version>=1),
    file_journal_id BLOB NOT NULL CHECK(length(file_journal_id)=16),
    status TEXT NOT NULL CHECK(status IN
        ('partial-success','completed','compensated','manual-review')),
    nfo_status TEXT NOT NULL CHECK(nfo_status IN
        ('not-requested','preserved','generated','failed')),
    catalog_media_item_id BLOB CHECK(catalog_media_item_id IS NULL OR length(catalog_media_item_id)=16),
    remaining_actions_json TEXT NOT NULL CHECK(json_valid(remaining_actions_json)),
    version INTEGER NOT NULL CHECK(version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(plan_id) REFERENCES organization_plans(id),
    FOREIGN KEY(file_journal_id) REFERENCES organization_file_operation_journals(id)
) STRICT;

CREATE INDEX organization_local_results_task_idx
ON organization_local_results(account_id,task_id,updated_at_us,id);

CREATE TABLE organization_rollback_receipts (
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    idempotency_key_sha256 BLOB NOT NULL CHECK(length(idempotency_key_sha256)=32),
    request_digest BLOB NOT NULL CHECK(length(request_digest)=32),
    result_id BLOB NOT NULL CHECK(length(result_id)=16),
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(account_id,idempotency_key_sha256),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(result_id) REFERENCES organization_local_results(id)
) WITHOUT ROWID, STRICT;

PRAGMA user_version = 23;
