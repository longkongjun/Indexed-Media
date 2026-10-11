-- M3-C4：Jellyfin 独立连接（ServiceHealthProvider 边界）。
-- 配置与服务健康独立于 MediaFlow 资源库；base_url 非敏感明文，API 凭据认证加密。
-- 本表不承载媒体库映射，也不被任何任务表引用——Jellyfin 不可用不得影响本地任务。

CREATE TABLE connectors_jellyfin (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    base_url TEXT NOT NULL CHECK(base_url LIKE 'https://%' OR base_url LIKE 'http://%'),
    display_name TEXT CHECK(display_name IS NULL OR length(display_name) BETWEEN 1 AND 120),
    secret_schema_version INTEGER CHECK(secret_schema_version IS NULL OR secret_schema_version=1),
    secret_nonce BLOB CHECK(secret_nonce IS NULL OR length(secret_nonce)=24),
    secret_ciphertext BLOB,
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    health TEXT NOT NULL CHECK(health IN ('unconfigured','healthy','degraded','unavailable','unauthorized')),
    failure_code TEXT CHECK(failure_code IS NULL OR failure_code IN ('integration.not-configured','integration.unauthorized','integration.unavailable','provider.timeout','provider.invalid-response')),
    server_version TEXT CHECK(server_version IS NULL OR length(server_version) BETWEEN 1 AND 64),
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

PRAGMA user_version = 33;
