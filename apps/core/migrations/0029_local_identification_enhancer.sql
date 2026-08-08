CREATE TABLE identification_enhancer (
    singleton_key INTEGER PRIMARY KEY NOT NULL CHECK(singleton_key=1),
    id BLOB NOT NULL UNIQUE CHECK(length(id)=16),
    kind TEXT NOT NULL CHECK(kind='ollama'),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    base_url TEXT NOT NULL CHECK(length(base_url) BETWEEN 1 AND 2048),
    endpoint_summary TEXT NOT NULL CHECK(length(endpoint_summary) BETWEEN 1 AND 255),
    model TEXT NOT NULL CHECK(length(model) BETWEEN 1 AND 128),
    timeout_ms INTEGER NOT NULL CHECK(timeout_ms BETWEEN 100 AND 30000),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    health TEXT NOT NULL CHECK(health IN ('healthy','degraded','unavailable','rate-limited')),
    checked_at_us INTEGER,
    fallback_code TEXT CHECK(fallback_code IS NULL OR fallback_code IN (
        'automation.source-disabled','integration.not-configured','integration.rate-limited',
        'integration.unavailable','provider.timeout','provider.response-too-large',
        'provider.invalid-response'
    )),
    projection_version INTEGER NOT NULL CHECK(projection_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    CHECK((health='healthy')=(fallback_code IS NULL)),
    CHECK(enabled=1 OR (health='degraded' AND fallback_code='automation.source-disabled')),
    CHECK(checked_at_us IS NULL OR checked_at_us<=updated_at_us)
) STRICT;

INSERT INTO identification_enhancer
(singleton_key,id,kind,enabled,base_url,endpoint_summary,model,timeout_ms,config_version,
 health,checked_at_us,fallback_code,projection_version,created_at_us,updated_at_us)
VALUES
(1,x'00000000000000000000000000000001','ollama',0,'http://127.0.0.1:11434',
 'http://127.0.0.1:11434','qwen3:4b',3000,1,'degraded',NULL,
 'automation.source-disabled',1,0,0);

PRAGMA user_version = 29;
