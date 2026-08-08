CREATE TABLE platform_outbox_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_type TEXT NOT NULL,
    schema_version TEXT NOT NULL,
    aggregate_id BLOB NOT NULL CHECK(length(aggregate_id) = 16),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    committed_at_us INTEGER NOT NULL,
    delivery_attempts INTEGER NOT NULL DEFAULT 0 CHECK(delivery_attempts >= 0),
    last_delivery_error TEXT
) STRICT;

CREATE INDEX platform_outbox_events_aggregate_idx
    ON platform_outbox_events(aggregate_id, id);

PRAGMA user_version = 5;
