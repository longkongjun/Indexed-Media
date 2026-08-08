PRAGMA user_version = 1;

CREATE TABLE platform_metadata (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at_us INTEGER NOT NULL
) STRICT;

CREATE TABLE platform_audit_events (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    occurred_at_us INTEGER NOT NULL,
    category TEXT NOT NULL,
    action TEXT NOT NULL,
    outcome TEXT NOT NULL,
    subject_id TEXT,
    safe_details_json TEXT NOT NULL DEFAULT '{}'
) STRICT;

CREATE INDEX platform_audit_events_occurred_at_idx
    ON platform_audit_events (occurred_at_us, id);
