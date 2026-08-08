CREATE TABLE connectors_integrations (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    kind TEXT NOT NULL UNIQUE CHECK(kind='tmdb'),
    locale TEXT NOT NULL CHECK(length(locale)=5),
    region TEXT CHECK(region IS NULL OR length(region)=2),
    secret_schema_version INTEGER,
    secret_nonce BLOB CHECK(secret_nonce IS NULL OR length(secret_nonce)=24),
    secret_ciphertext BLOB,
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    health TEXT NOT NULL CHECK(health IN ('unconfigured','healthy','degraded','unavailable','unauthorized','rate-limited')),
    failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN ('integration.not-configured','integration.unauthorized','integration.rate-limited','integration.unavailable','provider.timeout','provider.response-too-large','provider.invalid-response')),
    checked_at_us INTEGER,
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    CHECK(
        (secret_schema_version IS NULL AND secret_nonce IS NULL AND secret_ciphertext IS NULL)
        OR
        (secret_schema_version=1 AND secret_nonce IS NOT NULL AND length(secret_ciphertext)>=16)
    ),
    CHECK((health='unconfigured') = (secret_ciphertext IS NULL))
) STRICT;

CREATE TABLE connectors_tmdb_cache (
    query_key BLOB PRIMARY KEY NOT NULL CHECK(length(query_key)=32),
    provider_schema_version INTEGER NOT NULL CHECK(provider_schema_version>=1),
    outcome TEXT NOT NULL CHECK(outcome IN ('found','not-found')),
    response_json BLOB CHECK(response_json IS NULL OR length(response_json)<=2097152),
    fresh_until_us INTEGER NOT NULL,
    stale_until_us INTEGER NOT NULL,
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    CHECK((outcome='found' AND response_json IS NOT NULL) OR (outcome='not-found' AND response_json IS NULL))
) STRICT;

CREATE INDEX connectors_tmdb_cache_expiry
ON connectors_tmdb_cache(stale_until_us,query_key);
