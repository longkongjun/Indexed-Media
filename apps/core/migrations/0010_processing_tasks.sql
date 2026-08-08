CREATE TABLE tasks_processing_tasks (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    discovered_file_id BLOB NOT NULL CHECK(length(discovered_file_id)=16),
    file_revision_id BLOB NOT NULL CHECK(length(file_revision_id)=16),
    inbox_directory_id BLOB NOT NULL CHECK(length(inbox_directory_id)=16),
    current_attempt_id BLOB NOT NULL CHECK(length(current_attempt_id)=16),
    status TEXT NOT NULL CHECK(status IN ('queued','running','waiting-confirmation','paused','cancelled')),
    stage TEXT NOT NULL CHECK(stage IN ('identification','planning','file-operation','nfo','completion')),
    checkpoint TEXT NOT NULL CHECK(checkpoint IN ('pending','identification-complete','waiting-confirmation','dependency-blocked','skipped-auxiliary','cancelled')),
    reason TEXT CHECK(reason IS NULL OR reason IN (
        'identification.ambiguous','identification.confirmed-external-id',
        'identification.confirmed-title-year','identification.multiple-strong-candidates',
        'identification.no-candidate','identification.probable-title',
        'identification.provider-unavailable','identification.provider-unauthorized',
        'identification.revision-changed','auxiliary.sample','auxiliary.trailer',
        'auxiliary.extra','watcher.unavailable','reconcile.failed',
        'integration.unconfigured','integration.healthy','integration.unauthorized',
        'integration.rate-limited','integration.unavailable'
    )),
    recovering INTEGER NOT NULL DEFAULT 0 CHECK(recovering IN (0,1)),
    attempt_count INTEGER NOT NULL DEFAULT 1 CHECK(attempt_count>=1),
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)),
    lease_owner TEXT,
    lease_expires_at_us INTEGER,
    next_retry_at_us INTEGER,
    config_snapshot_json TEXT NOT NULL CHECK(json_valid(config_snapshot_json) AND length(config_snapshot_json)<=16384),
    version INTEGER NOT NULL DEFAULT 1 CHECK(version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(discovered_file_id,file_revision_id),
    CHECK((status='running') = (lease_owner IS NOT NULL AND lease_expires_at_us IS NOT NULL)),
    CHECK(cancel_requested=0 OR status='running'),
    CHECK(next_retry_at_us IS NULL OR status='paused'),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(discovered_file_id) REFERENCES discovery_tracked_files(id),
    FOREIGN KEY(file_revision_id) REFERENCES discovery_file_revisions(id),
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id)
) STRICT;

CREATE INDEX tasks_processing_claim_idx
ON tasks_processing_tasks(status,stage,created_at_us,id);

CREATE INDEX tasks_processing_due_retry_idx
ON tasks_processing_tasks(status,stage,next_retry_at_us,id)
WHERE status='paused' AND next_retry_at_us IS NOT NULL;

CREATE TABLE tasks_processing_attempts (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    reason TEXT NOT NULL CHECK(reason IN (
        'initial','manual-retry','dependency-recovery','lease-recovery','revision-replaced'
    )),
    ordinal INTEGER NOT NULL CHECK(ordinal>=1),
    status TEXT NOT NULL CHECK(status IN ('queued','running','succeeded','failed','cancelled')),
    stage TEXT NOT NULL CHECK(stage IN ('identification','planning','file-operation','nfo','completion')),
    lease_owner TEXT,
    failure_code TEXT,
    started_at_us INTEGER,
    finished_at_us INTEGER,
    created_at_us INTEGER NOT NULL,
    UNIQUE(task_id,ordinal),
    CHECK((status='running') = (lease_owner IS NOT NULL)),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id) ON DELETE CASCADE
) STRICT;

CREATE UNIQUE INDEX tasks_processing_one_active_attempt
ON tasks_processing_attempts(task_id)
WHERE status IN ('queued','running');

CREATE TABLE tasks_processing_idempotency_bindings (
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    key_sha256 BLOB NOT NULL CHECK(length(key_sha256)=32),
    action TEXT NOT NULL CHECK(action IN ('processing.retry','processing.cancel')),
    target_id BLOB NOT NULL CHECK(length(target_id)=16),
    result_task_id BLOB NOT NULL CHECK(length(result_task_id)=16),
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(account_id,key_sha256),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(result_task_id) REFERENCES tasks_processing_tasks(id)
) WITHOUT ROWID, STRICT;

CREATE TABLE tasks_processing_task_order_history (
    revision INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

CREATE INDEX tasks_processing_order_snapshot_idx
ON tasks_processing_task_order_history(account_id,revision,task_id,updated_at_us);

CREATE INDEX tasks_processing_order_task_revision_idx
ON tasks_processing_task_order_history(task_id,revision DESC);

CREATE TRIGGER tasks_processing_order_after_insert
AFTER INSERT ON tasks_processing_tasks
BEGIN
    INSERT INTO tasks_processing_task_order_history(task_id,account_id,updated_at_us)
    VALUES (NEW.id,NEW.account_id,NEW.updated_at_us);
END;

CREATE TRIGGER tasks_processing_order_after_update
AFTER UPDATE OF updated_at_us ON tasks_processing_tasks
BEGIN
    INSERT INTO tasks_processing_task_order_history(task_id,account_id,updated_at_us)
    VALUES (NEW.id,NEW.account_id,NEW.updated_at_us);
END;
