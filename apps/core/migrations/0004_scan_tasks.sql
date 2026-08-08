CREATE TABLE discovery_scan_batches (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    inbox_directory_id BLOB NOT NULL CHECK(length(inbox_directory_id) = 16),
    inbox_version INTEGER NOT NULL CHECK(inbox_version >= 1),
    started_at_us INTEGER,
    finished_at_us INTEGER,
    visited_directories INTEGER NOT NULL DEFAULT 0 CHECK(visited_directories >= 0),
    observed_files INTEGER NOT NULL DEFAULT 0 CHECK(observed_files >= 0),
    skipped_entries INTEGER NOT NULL DEFAULT 0 CHECK(skipped_entries >= 0),
    errors INTEGER NOT NULL DEFAULT 0 CHECK(errors >= 0),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id)
) STRICT;

CREATE TABLE tasks_scan_tasks (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    account_id BLOB NOT NULL CHECK(length(account_id) = 16),
    scan_batch_id BLOB NOT NULL UNIQUE CHECK(length(scan_batch_id) = 16),
    inbox_directory_id BLOB NOT NULL CHECK(length(inbox_directory_id) = 16),
    status TEXT NOT NULL CHECK(status IN ('queued','running','partial-success','completed','failed','cancelled')),
    stage TEXT NOT NULL CHECK(stage IN ('queued','enumerating','finalizing','finished')),
    recovering INTEGER NOT NULL DEFAULT 0 CHECK(recovering IN (0,1)),
    current_attempt_id BLOB NOT NULL CHECK(length(current_attempt_id) = 16),
    visited_directories INTEGER NOT NULL DEFAULT 0 CHECK(visited_directories >= 0),
    observed_files INTEGER NOT NULL DEFAULT 0 CHECK(observed_files >= 0),
    skipped_entries INTEGER NOT NULL DEFAULT 0 CHECK(skipped_entries >= 0),
    errors INTEGER NOT NULL DEFAULT 0 CHECK(errors >= 0),
    cancel_requested INTEGER NOT NULL DEFAULT 0 CHECK(cancel_requested IN (0,1)),
    lease_owner TEXT,
    lease_expires_at_us INTEGER,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(scan_batch_id) REFERENCES discovery_scan_batches(id),
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id)
) STRICT;

CREATE INDEX tasks_scan_tasks_status_updated_idx
    ON tasks_scan_tasks(status, updated_at_us DESC, id DESC);
CREATE INDEX tasks_scan_tasks_updated_idx
    ON tasks_scan_tasks(updated_at_us DESC, id DESC);

CREATE TABLE tasks_scan_attempts (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    task_id BLOB NOT NULL CHECK(length(task_id) = 16),
    reason TEXT NOT NULL CHECK(reason IN ('initial','manual_retry','recovery')),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 1),
    status TEXT NOT NULL CHECK(status IN ('queued','running','partial-success','completed','failed','cancelled')),
    lease_owner TEXT,
    started_at_us INTEGER,
    finished_at_us INTEGER,
    created_at_us INTEGER NOT NULL,
    UNIQUE(task_id, ordinal),
    FOREIGN KEY(task_id) REFERENCES tasks_scan_tasks(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE discovery_files (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    inbox_directory_id BLOB NOT NULL CHECK(length(inbox_directory_id) = 16),
    relative_path_bytes BLOB NOT NULL,
    relative_path_display TEXT NOT NULL,
    identity_snapshot BLOB NOT NULL,
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
    modified_at_ns INTEGER NOT NULL,
    first_seen_batch_id BLOB NOT NULL CHECK(length(first_seen_batch_id) = 16),
    last_seen_batch_id BLOB NOT NULL CHECK(length(last_seen_batch_id) = 16),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(inbox_directory_id, relative_path_bytes),
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id),
    FOREIGN KEY(first_seen_batch_id) REFERENCES discovery_scan_batches(id),
    FOREIGN KEY(last_seen_batch_id) REFERENCES discovery_scan_batches(id)
) STRICT;

CREATE INDEX discovery_files_page_idx
    ON discovery_files(inbox_directory_id, relative_path_bytes ASC, id ASC);

CREATE TABLE discovery_scan_file_observations (
    scan_batch_id BLOB NOT NULL CHECK(length(scan_batch_id) = 16),
    discovered_file_id BLOB NOT NULL CHECK(length(discovered_file_id) = 16),
    last_observed_attempt_id BLOB NOT NULL CHECK(length(last_observed_attempt_id) = 16),
    identity_snapshot BLOB NOT NULL,
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
    modified_at_ns INTEGER NOT NULL,
    observed_at_us INTEGER NOT NULL,
    PRIMARY KEY(scan_batch_id, discovered_file_id),
    FOREIGN KEY(scan_batch_id) REFERENCES discovery_scan_batches(id) ON DELETE CASCADE,
    FOREIGN KEY(discovered_file_id) REFERENCES discovery_files(id),
    FOREIGN KEY(last_observed_attempt_id) REFERENCES tasks_scan_attempts(id)
) WITHOUT ROWID, STRICT;

CREATE TABLE tasks_scan_errors (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    task_id BLOB NOT NULL CHECK(length(task_id) = 16),
    scan_batch_id BLOB NOT NULL CHECK(length(scan_batch_id) = 16),
    attempt_id BLOB NOT NULL CHECK(length(attempt_id) = 16),
    code TEXT NOT NULL,
    scope TEXT NOT NULL CHECK(scope IN ('entry','directory','root')),
    relative_path_bytes BLOB NOT NULL,
    relative_path_display TEXT NOT NULL,
    first_seen_at_us INTEGER NOT NULL,
    last_seen_at_us INTEGER NOT NULL,
    occurrences INTEGER NOT NULL DEFAULT 1 CHECK(occurrences >= 1),
    UNIQUE(attempt_id, code, scope, relative_path_bytes),
    FOREIGN KEY(task_id) REFERENCES tasks_scan_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY(scan_batch_id) REFERENCES discovery_scan_batches(id) ON DELETE CASCADE,
    FOREIGN KEY(attempt_id) REFERENCES tasks_scan_attempts(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX tasks_scan_errors_page_idx
    ON tasks_scan_errors(task_id, attempt_id, first_seen_at_us ASC, id ASC);

CREATE TABLE tasks_idempotency_keys (
    account_id BLOB NOT NULL CHECK(length(account_id) = 16),
    action TEXT NOT NULL,
    target_id BLOB NOT NULL CHECK(length(target_id) = 16),
    key_sha256 BLOB NOT NULL CHECK(length(key_sha256) = 32),
    result_task_id BLOB NOT NULL CHECK(length(result_task_id) = 16),
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(account_id, action, target_id, key_sha256),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(result_task_id) REFERENCES tasks_scan_tasks(id)
) WITHOUT ROWID, STRICT;

PRAGMA user_version = 4;
