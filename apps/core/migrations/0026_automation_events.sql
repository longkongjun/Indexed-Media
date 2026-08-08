CREATE TABLE automation_events (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    source_id BLOB NOT NULL CHECK(length(source_id)=16),
    source_display_name TEXT NOT NULL CHECK(length(source_display_name) BETWEEN 1 AND 120),
    source_config_version INTEGER NOT NULL CHECK(source_config_version>=1),
    action TEXT NOT NULL CHECK(action IN ('create-download','reconcile-inbox')),
    dedup_key_sha256 BLOB NOT NULL CHECK(length(dedup_key_sha256)=32),
    request_sha256 BLOB NOT NULL CHECK(length(request_sha256)=32),
    payload_schema_version INTEGER NOT NULL CHECK(payload_schema_version=1),
    payload_nonce BLOB NOT NULL CHECK(length(payload_nonce)=24),
    payload_ciphertext BLOB NOT NULL CHECK(length(payload_ciphertext)>=16),
    status TEXT NOT NULL CHECK(status IN ('pending','running','retry-wait','completed','failed','cancelled')),
    downstream_kind TEXT CHECK(downstream_kind IS NULL OR downstream_kind IN ('download-task','reconcile-request')),
    downstream_id BLOB CHECK(downstream_id IS NULL OR length(downstream_id)=16),
    result_count INTEGER CHECK(result_count IS NULL OR result_count>=0),
    failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
        'automation.source-disabled','automation.signature-invalid','automation.replay',
        'automation.action-invalid','automation.payload-invalid','automation.downstream-conflict',
        'integration.not-configured','integration.unauthorized','integration.rate-limited',
        'integration.unavailable','provider.timeout','provider.response-too-large',
        'provider.invalid-response'
    )),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count BETWEEN 0 AND 10),
    retry_at_us INTEGER,
    lease_token BLOB CHECK(lease_token IS NULL OR length(lease_token)=16),
    lease_expires_at_us INTEGER,
    projection_version INTEGER NOT NULL CHECK(projection_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(source_id,dedup_key_sha256),
    CHECK((downstream_kind IS NULL)=(downstream_id IS NULL)),
    CHECK((lease_token IS NULL)=(lease_expires_at_us IS NULL)),
    CHECK((status='retry-wait')=(retry_at_us IS NOT NULL)),
    CHECK((status IN ('running'))=(lease_token IS NOT NULL))
) STRICT;

CREATE INDEX automation_events_list_idx
ON automation_events(updated_at_us DESC,id DESC);

CREATE INDEX automation_events_claim_idx
ON automation_events(status,retry_at_us,created_at_us,id);

CREATE INDEX automation_events_source_active_idx
ON automation_events(source_id,status,updated_at_us);

CREATE TABLE automation_webhook_rotation_receipts (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    source_id BLOB NOT NULL CHECK(length(source_id)=16),
    idempotency_key_sha256 BLOB NOT NULL CHECK(length(idempotency_key_sha256)=32),
    request_sha256 BLOB NOT NULL CHECK(length(request_sha256)=32),
    source_config_version INTEGER NOT NULL CHECK(source_config_version>=1),
    secret_schema_version INTEGER NOT NULL CHECK(secret_schema_version=1),
    secret_nonce BLOB NOT NULL CHECK(length(secret_nonce)=24),
    secret_ciphertext BLOB NOT NULL CHECK(length(secret_ciphertext)>=16),
    created_at_us INTEGER NOT NULL,
    UNIQUE(source_id,idempotency_key_sha256),
    FOREIGN KEY(source_id) REFERENCES automation_sources(id) ON DELETE CASCADE
) STRICT;

PRAGMA user_version = 26;
