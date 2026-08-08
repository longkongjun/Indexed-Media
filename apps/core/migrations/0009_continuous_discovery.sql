CREATE TABLE discovery_inbox_policies (
    inbox_directory_id BLOB PRIMARY KEY NOT NULL CHECK(length(inbox_directory_id) = 16),
    minimum_age_seconds INTEGER NOT NULL DEFAULT 60
        CHECK(minimum_age_seconds BETWEEN 0 AND 86400),
    stable_observation_interval_seconds INTEGER NOT NULL DEFAULT 30
        CHECK(stable_observation_interval_seconds BETWEEN 1 AND 86400),
    reconcile_interval_seconds INTEGER NOT NULL DEFAULT 900
        CHECK(reconcile_interval_seconds BETWEEN 60 AND 604800),
    watcher_enabled INTEGER NOT NULL DEFAULT 1 CHECK(watcher_enabled IN (0,1)),
    config_version INTEGER NOT NULL DEFAULT 1 CHECK(config_version >= 1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id) ON DELETE CASCADE
) STRICT;

INSERT INTO discovery_inbox_policies
    (inbox_directory_id,minimum_age_seconds,stable_observation_interval_seconds,
     reconcile_interval_seconds,watcher_enabled,config_version,created_at_us,updated_at_us)
SELECT id,60,30,900,1,1,created_at_us,updated_at_us
FROM discovery_inbox_directories;

CREATE TABLE discovery_watch_states (
    inbox_directory_id BLOB PRIMARY KEY NOT NULL CHECK(length(inbox_directory_id) = 16),
    health TEXT NOT NULL DEFAULT 'pending'
        CHECK(health IN ('pending','healthy','degraded','unavailable','disabled')),
    watcher_active INTEGER NOT NULL DEFAULT 0 CHECK(watcher_active IN (0,1)),
    last_event_at_us INTEGER,
    last_error_code TEXT,
    last_reconcile_started_at_us INTEGER,
    last_reconcile_finished_at_us INTEGER,
    next_reconcile_at_us INTEGER NOT NULL,
    active_reconcile_task_id BLOB
        CHECK(active_reconcile_task_id IS NULL OR length(active_reconcile_task_id) = 16),
    last_reconcile_task_id BLOB
        CHECK(last_reconcile_task_id IS NULL OR length(last_reconcile_task_id) = 16),
    requested_reason TEXT
        CHECK(requested_reason IS NULL OR requested_reason IN ('startup','periodic','watch_recovery')),
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id) ON DELETE CASCADE,
    FOREIGN KEY(active_reconcile_task_id) REFERENCES tasks_scan_tasks(id),
    FOREIGN KEY(last_reconcile_task_id) REFERENCES tasks_scan_tasks(id)
) STRICT;

INSERT INTO discovery_watch_states
    (inbox_directory_id,health,watcher_active,next_reconcile_at_us,version,created_at_us,updated_at_us)
SELECT id,'pending',0,updated_at_us,1,created_at_us,updated_at_us
FROM discovery_inbox_directories;

ALTER TABLE tasks_scan_tasks ADD COLUMN reason TEXT NOT NULL DEFAULT 'manual'
    CHECK(reason IN ('manual','startup','periodic','watch_recovery'));

CREATE INDEX tasks_scan_tasks_reconcile_idx
    ON tasks_scan_tasks(reason, status, inbox_directory_id, created_at_us, id)
    WHERE reason != 'manual';

CREATE UNIQUE INDEX tasks_scan_tasks_one_active_reconcile
    ON tasks_scan_tasks(inbox_directory_id)
    WHERE reason != 'manual' AND status IN ('queued','running');

CREATE TABLE discovery_tracked_files (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    inbox_directory_id BLOB NOT NULL CHECK(length(inbox_directory_id) = 16),
    relative_path_bytes BLOB NOT NULL,
    relative_path_display TEXT NOT NULL,
    current_revision_id BLOB CHECK(current_revision_id IS NULL OR length(current_revision_id) = 16),
    last_observed_at_us INTEGER NOT NULL,
    missing_at_us INTEGER,
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(inbox_directory_id, relative_path_bytes),
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX discovery_tracked_files_inbox_seen_idx
    ON discovery_tracked_files(inbox_directory_id, last_observed_at_us, id);

CREATE TABLE discovery_file_revisions (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    tracked_file_id BLOB NOT NULL CHECK(length(tracked_file_id) = 16),
    identity_snapshot BLOB NOT NULL,
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 0),
    modified_at_ns INTEGER NOT NULL,
    policy_version INTEGER NOT NULL CHECK(policy_version >= 1),
    minimum_age_seconds INTEGER NOT NULL CHECK(minimum_age_seconds BETWEEN 0 AND 86400),
    stable_observation_interval_seconds INTEGER NOT NULL
        CHECK(stable_observation_interval_seconds BETWEEN 1 AND 86400),
    created_at_us INTEGER NOT NULL,
    UNIQUE(tracked_file_id, identity_snapshot, size_bytes, modified_at_ns),
    FOREIGN KEY(tracked_file_id) REFERENCES discovery_tracked_files(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX discovery_file_revisions_file_created_idx
    ON discovery_file_revisions(tracked_file_id, created_at_us, id);

CREATE TABLE discovery_file_revision_states (
    revision_id BLOB PRIMARY KEY NOT NULL CHECK(length(revision_id) = 16),
    status TEXT NOT NULL
        CHECK(status IN ('observing','stable','missing','superseded','skipped-auxiliary')),
    matching_observations INTEGER NOT NULL DEFAULT 1 CHECK(matching_observations >= 1),
    first_observed_at_us INTEGER NOT NULL,
    last_counted_observed_at_us INTEGER NOT NULL,
    last_observed_at_us INTEGER NOT NULL,
    next_check_at_us INTEGER,
    stable_at_us INTEGER,
    missing_at_us INTEGER,
    last_observation_source TEXT NOT NULL CHECK(last_observation_source IN ('scan','watcher','reconcile')),
    skip_reason TEXT CHECK(skip_reason IS NULL OR skip_reason IN ('sample','trailer','extra')),
    version INTEGER NOT NULL DEFAULT 1 CHECK(version >= 1),
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(revision_id) REFERENCES discovery_file_revisions(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX discovery_revision_states_due_idx
    ON discovery_file_revision_states(status, next_check_at_us, revision_id)
    WHERE status = 'observing';

CREATE TABLE discovery_processing_requests (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    revision_id BLOB NOT NULL UNIQUE CHECK(length(revision_id) = 16),
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','claimed','completed')),
    created_at_us INTEGER NOT NULL,
    claimed_at_us INTEGER,
    completed_at_us INTEGER,
    FOREIGN KEY(revision_id) REFERENCES discovery_file_revisions(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX discovery_processing_requests_pending_idx
    ON discovery_processing_requests(status, created_at_us, id)
    WHERE status = 'pending';
