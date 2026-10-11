-- M5 第二批：设备配对与设备令牌。移动端以一次性配对码（Web 签发）或管理员凭据完成
-- enrollment，获得长期设备令牌。令牌仅存 SHA-256 摘要（与会话令牌同先例）；设备令牌
-- 按"令牌即写证明"豁免写路由的同源/CSRF（见 platform::request_security）。设备代表
-- 管理员本人（单管理员模型），业务面权限与管理员会话一致；设备管理面仅浏览器会话通道。

CREATE TABLE identity_devices (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    account_id BLOB NOT NULL REFERENCES identity_accounts(id),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 64),
    token_sha256 BLOB NOT NULL UNIQUE CHECK(length(token_sha256) = 32),
    credential_version INTEGER NOT NULL,
    absolute_expires_at_us INTEGER NOT NULL,
    created_at_us INTEGER NOT NULL,
    last_used_at_us INTEGER,
    revoked_at_us INTEGER,
    revoked_reason TEXT CHECK(revoked_reason IN ('logout','administrator','rotation'))
) STRICT;

CREATE TABLE identity_pairing_codes (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    account_id BLOB NOT NULL REFERENCES identity_accounts(id),
    code_sha256 BLOB NOT NULL UNIQUE CHECK(length(code_sha256) = 32),
    created_at_us INTEGER NOT NULL,
    expires_at_us INTEGER NOT NULL,
    used_at_us INTEGER
) STRICT;

CREATE INDEX identity_devices_account_idx ON identity_devices(account_id);

PRAGMA user_version = 37;
