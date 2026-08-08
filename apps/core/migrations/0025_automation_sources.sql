CREATE TABLE automation_sources (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('rss','webhook','download-completion')),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 120),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    downloader_connection_id BLOB CHECK(downloader_connection_id IS NULL OR length(downloader_connection_id)=16),
    inbox_directory_id BLOB CHECK(inbox_directory_id IS NULL OR length(inbox_directory_id)=16),
    endpoint_summary TEXT CHECK(endpoint_summary IS NULL OR length(endpoint_summary) BETWEEN 1 AND 255),
    poll_interval_seconds INTEGER CHECK(poll_interval_seconds IS NULL OR poll_interval_seconds BETWEEN 60 AND 86400),
    allowed_actions_json TEXT,
    secret_fingerprint TEXT CHECK(secret_fingerprint IS NULL OR (length(secret_fingerprint)=8 AND secret_fingerprint NOT GLOB '*[^0-9a-f]*')),
    secret_schema_version INTEGER CHECK(secret_schema_version IS NULL OR secret_schema_version=1),
    secret_nonce BLOB CHECK(secret_nonce IS NULL OR length(secret_nonce)=24),
    secret_ciphertext BLOB CHECK(secret_ciphertext IS NULL OR length(secret_ciphertext)>=16),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    health TEXT NOT NULL CHECK(health IN ('healthy','degraded','unavailable','unauthorized','rate-limited')),
    checked_at_us INTEGER,
    failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
        'automation.source-disabled','automation.signature-invalid','automation.replay',
        'automation.action-invalid','automation.payload-invalid','automation.downstream-conflict',
        'integration.not-configured','integration.unauthorized','integration.rate-limited',
        'integration.unavailable','provider.timeout','provider.response-too-large',
        'provider.invalid-response'
    )),
    projection_version INTEGER NOT NULL CHECK(projection_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(downloader_connection_id) REFERENCES downloader_connections(id) ON DELETE RESTRICT,
    FOREIGN KEY(inbox_directory_id) REFERENCES discovery_inbox_directories(id) ON DELETE RESTRICT,
    CHECK(
        (kind='rss' AND downloader_connection_id IS NOT NULL AND inbox_directory_id IS NULL
         AND endpoint_summary IS NOT NULL AND poll_interval_seconds IS NOT NULL
         AND allowed_actions_json IS NULL AND secret_fingerprint IS NULL
         AND secret_schema_version=1 AND secret_nonce IS NOT NULL AND secret_ciphertext IS NOT NULL)
        OR
        (kind='webhook' AND downloader_connection_id IS NULL AND inbox_directory_id IS NULL
         AND endpoint_summary IS NULL AND poll_interval_seconds IS NULL
         AND allowed_actions_json IS NOT NULL AND secret_fingerprint IS NOT NULL
         AND secret_schema_version=1 AND secret_nonce IS NOT NULL AND secret_ciphertext IS NOT NULL)
        OR
        (kind='download-completion' AND downloader_connection_id IS NOT NULL AND inbox_directory_id IS NOT NULL
         AND endpoint_summary IS NULL AND poll_interval_seconds IS NULL
         AND allowed_actions_json IS NULL AND secret_fingerprint IS NULL
         AND secret_schema_version IS NULL AND secret_nonce IS NULL AND secret_ciphertext IS NULL)
    ),
    CHECK((health='healthy')=(failure_code IS NULL)),
    CHECK(checked_at_us IS NULL OR checked_at_us<=updated_at_us)
) STRICT;

CREATE INDEX automation_sources_list_idx
ON automation_sources(updated_at_us DESC,id DESC);

CREATE INDEX automation_sources_downloader_idx
ON automation_sources(downloader_connection_id,kind,enabled);

CREATE TABLE automation_source_runtime (
    source_id BLOB PRIMARY KEY NOT NULL CHECK(length(source_id)=16),
    next_poll_at_us INTEGER,
    cursor_schema_version INTEGER CHECK(cursor_schema_version IS NULL OR cursor_schema_version=1),
    cursor_nonce BLOB CHECK(cursor_nonce IS NULL OR length(cursor_nonce)=24),
    cursor_ciphertext BLOB CHECK(cursor_ciphertext IS NULL OR length(cursor_ciphertext)>=16),
    lease_token BLOB CHECK(lease_token IS NULL OR length(lease_token)=16),
    lease_expires_at_us INTEGER,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK(attempt_count>=0),
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(source_id) REFERENCES automation_sources(id) ON DELETE CASCADE,
    CHECK((cursor_schema_version IS NULL)=(cursor_nonce IS NULL)),
    CHECK((cursor_nonce IS NULL)=(cursor_ciphertext IS NULL)),
    CHECK((lease_token IS NULL)=(lease_expires_at_us IS NULL))
) STRICT;

CREATE TABLE automation_webhook_nonces (
    source_id BLOB NOT NULL CHECK(length(source_id)=16),
    nonce_sha256 BLOB NOT NULL CHECK(length(nonce_sha256)=32),
    body_sha256 BLOB NOT NULL CHECK(length(body_sha256)=32),
    signed_at_us INTEGER NOT NULL,
    expires_at_us INTEGER NOT NULL CHECK(expires_at_us>=signed_at_us),
    response_event_id BLOB CHECK(response_event_id IS NULL OR length(response_event_id)=16),
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(source_id,nonce_sha256),
    FOREIGN KEY(source_id) REFERENCES automation_sources(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX automation_webhook_nonces_expiry_idx
ON automation_webhook_nonces(expires_at_us);

PRAGMA user_version = 25;
