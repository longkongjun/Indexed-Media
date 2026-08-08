CREATE TABLE downloader_connections (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('qbittorrent','transmission')),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 120),
    base_url TEXT NOT NULL CHECK(length(base_url) BETWEEN 1 AND 2048),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    secret_schema_version INTEGER NOT NULL CHECK(secret_schema_version=1),
    secret_nonce BLOB NOT NULL CHECK(length(secret_nonce)=24),
    secret_ciphertext BLOB NOT NULL CHECK(length(secret_ciphertext)>=16),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    health TEXT NOT NULL CHECK(health IN (
        'healthy','degraded','unavailable','unauthorized','rate-limited'
    )),
    failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN (
        'integration.unauthorized','integration.rate-limited','integration.unavailable',
        'integration.unsupported-version','provider.timeout','provider.response-too-large',
        'provider.invalid-response'
    )),
    checked_at_us INTEGER,
    manual_add INTEGER CHECK(manual_add IS NULL OR manual_add IN (0,1)),
    task_monitoring INTEGER CHECK(task_monitoring IS NULL OR task_monitoring IN (0,1)),
    product_version TEXT CHECK(product_version IS NULL OR length(product_version) BETWEEN 1 AND 64),
    api_version TEXT CHECK(api_version IS NULL OR length(api_version) BETWEEN 1 AND 64),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    CHECK(
        (manual_add IS NULL AND task_monitoring IS NULL AND product_version IS NULL AND api_version IS NULL)
        OR
        (manual_add IS NOT NULL AND task_monitoring IS NOT NULL AND product_version IS NOT NULL AND api_version IS NOT NULL)
    ),
    CHECK((health='healthy')=(failure_code IS NULL)),
    CHECK(checked_at_us IS NULL OR checked_at_us<=updated_at_us)
) STRICT;

CREATE INDEX downloader_connections_list_idx
ON downloader_connections(updated_at_us DESC,id DESC);
