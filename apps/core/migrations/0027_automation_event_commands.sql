CREATE TABLE automation_event_command_receipts (
    event_id BLOB NOT NULL CHECK(length(event_id)=16),
    operation TEXT NOT NULL CHECK(operation IN ('retry','cancel')),
    idempotency_key_sha256 BLOB NOT NULL CHECK(length(idempotency_key_sha256)=32),
    request_sha256 BLOB NOT NULL CHECK(length(request_sha256)=32),
    resulting_projection_version INTEGER NOT NULL CHECK(resulting_projection_version>=1),
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(event_id,operation,idempotency_key_sha256),
    UNIQUE(event_id,idempotency_key_sha256),
    FOREIGN KEY(event_id) REFERENCES automation_events(id) ON DELETE CASCADE
) STRICT;

PRAGMA user_version = 27;
