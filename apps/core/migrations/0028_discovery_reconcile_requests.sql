CREATE UNIQUE INDEX automation_sources_one_enabled_completion_mapping
ON automation_sources(downloader_connection_id)
WHERE kind='download-completion' AND enabled=1;

CREATE TABLE download_completion_signals (
    download_task_id BLOB PRIMARY KEY NOT NULL CHECK(length(download_task_id)=16),
    downloader_connection_id BLOB NOT NULL CHECK(length(downloader_connection_id)=16),
    completion_projection_version INTEGER NOT NULL CHECK(completion_projection_version>=1),
    status TEXT NOT NULL CHECK(status IN ('pending','running','completed','failed')),
    outcome TEXT CHECK(outcome IS NULL OR outcome IN ('event-created','no-mapping','dispatch-failed')),
    automation_event_id BLOB CHECK(automation_event_id IS NULL OR length(automation_event_id)=16),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count BETWEEN 0 AND 10),
    lease_token BLOB CHECK(lease_token IS NULL OR length(lease_token)=16),
    lease_expires_at_us INTEGER,
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(download_task_id) REFERENCES download_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY(downloader_connection_id) REFERENCES downloader_connections(id) ON DELETE RESTRICT,
    FOREIGN KEY(automation_event_id) REFERENCES automation_events(id) ON DELETE RESTRICT,
    CHECK((lease_token IS NULL)=(lease_expires_at_us IS NULL)),
    CHECK((status='running')=(lease_token IS NOT NULL)),
    CHECK((status IN ('completed','failed'))=(outcome IS NOT NULL)),
    CHECK((outcome='event-created')=(automation_event_id IS NOT NULL))
) STRICT;

CREATE INDEX download_completion_signals_claim_idx
ON download_completion_signals(status,created_at_us,download_task_id);

CREATE TABLE discovery_reconcile_requests (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    idempotency_key_sha256 BLOB NOT NULL UNIQUE CHECK(length(idempotency_key_sha256)=32),
    request_sha256 BLOB NOT NULL CHECK(length(request_sha256)=32),
    inbox_directory_id BLOB NOT NULL CHECK(length(inbox_directory_id)=16),
    scan_task_id BLOB NOT NULL CHECK(length(scan_task_id)=16),
    status TEXT NOT NULL CHECK(status IN ('accepted','completed','failed')),
    observed_files INTEGER CHECK(observed_files IS NULL OR observed_files>=0),
    errors INTEGER CHECK(errors IS NULL OR errors>=0),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id) ON DELETE RESTRICT,
    FOREIGN KEY(scan_task_id) REFERENCES tasks_scan_tasks(id) ON DELETE RESTRICT,
    CHECK((status='accepted')=(observed_files IS NULL)),
    CHECK((observed_files IS NULL)=(errors IS NULL))
) STRICT;

CREATE INDEX discovery_reconcile_requests_task_idx
ON discovery_reconcile_requests(scan_task_id,status,id);

PRAGMA user_version = 28;
