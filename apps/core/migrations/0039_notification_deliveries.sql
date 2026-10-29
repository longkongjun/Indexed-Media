-- Forward-only durable optional delivery. Existing notifications are not backfilled.
CREATE TABLE notification_deliveries (
    id TEXT PRIMARY KEY NOT NULL,
    notification_id BLOB NOT NULL CHECK(length(notification_id)=16),
    webhook_id BLOB NOT NULL CHECK(length(webhook_id)=16),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    status TEXT NOT NULL CHECK(status IN ('pending','sending','retry-wait','succeeded','failed','cancelled')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count BETWEEN 0 AND 10),
    retry_at_us INTEGER,
    lease_token BLOB CHECK(lease_token IS NULL OR length(lease_token)=16),
    lease_expires_at_us INTEGER,
    failure_code TEXT,
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(notification_id,webhook_id,config_version),
    FOREIGN KEY(notification_id) REFERENCES notifications(id)
) STRICT;
CREATE INDEX notification_deliveries_due_idx
    ON notification_deliveries(status,retry_at_us,lease_expires_at_us,created_at_us);
PRAGMA user_version = 39;
