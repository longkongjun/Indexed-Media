CREATE TABLE download_tasks (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    connection_id BLOB NOT NULL CHECK(length(connection_id)=16),
    connection_display_name TEXT NOT NULL CHECK(length(connection_display_name) BETWEEN 1 AND 120),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 512),
    source_sha256 BLOB NOT NULL CHECK(length(source_sha256)=32),
    source_schema_version INTEGER NOT NULL CHECK(source_schema_version=1),
    source_nonce BLOB NOT NULL CHECK(length(source_nonce)=24),
    source_ciphertext BLOB NOT NULL CHECK(length(source_ciphertext)>=16),
    idempotency_key_sha256 BLOB NOT NULL UNIQUE CHECK(length(idempotency_key_sha256)=32),
    request_sha256 BLOB NOT NULL CHECK(length(request_sha256)=32),
    status TEXT NOT NULL CHECK(status IN (
        'queued','submitting','monitoring','retry-wait','completed','failed'
    )),
    remote_id TEXT CHECK(
        remote_id IS NULL OR
        (length(remote_id) IN (40,64) AND remote_id NOT GLOB '*[^0-9a-f]*')
    ),
    remote_status TEXT CHECK(remote_status IS NULL OR remote_status IN (
        'queued','downloading','paused','completed','failed','unknown'
    )),
    progress_basis_points INTEGER NOT NULL CHECK(progress_basis_points BETWEEN 0 AND 10000),
    failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
        'integration.not-configured','integration.unauthorized','integration.rate-limited',
        'integration.unavailable','integration.unsupported-version','provider.timeout',
        'provider.response-too-large','provider.invalid-response',
        'download.correlation-ambiguous','download.remote-missing'
    )),
    retry_at_us INTEGER,
    lease_owner TEXT CHECK(lease_owner IS NULL OR length(lease_owner) BETWEEN 1 AND 128),
    lease_expires_at_us INTEGER,
    attempt_count INTEGER NOT NULL CHECK(attempt_count>=0),
    linked INTEGER NOT NULL CHECK(linked IN (0,1)),
    projection_version INTEGER NOT NULL CHECK(projection_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    CHECK(linked=(remote_id IS NOT NULL)),
    CHECK(remote_status IS NULL OR linked=1),
    CHECK((lease_owner IS NULL)=(lease_expires_at_us IS NULL)),
    CHECK(status NOT IN ('monitoring','completed') OR linked=1),
    CHECK(status!='completed' OR remote_status='completed'),
    CHECK(status!='failed' OR failure_code IS NOT NULL)
) STRICT;

CREATE UNIQUE INDEX download_tasks_remote_ref_unique
ON download_tasks(connection_id,remote_id)
WHERE remote_id IS NOT NULL;

CREATE INDEX download_tasks_list_idx
ON download_tasks(updated_at_us DESC,id DESC);

CREATE INDEX download_tasks_connection_active_idx
ON download_tasks(connection_id,status,id);

CREATE INDEX download_tasks_claim_idx
ON download_tasks(status,retry_at_us,lease_expires_at_us,created_at_us,id);
