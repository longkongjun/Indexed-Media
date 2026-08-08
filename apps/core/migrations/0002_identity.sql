CREATE TABLE identity_accounts (
    singleton_key INTEGER PRIMARY KEY NOT NULL CHECK(singleton_key = 1),
    id BLOB NOT NULL UNIQUE CHECK(length(id) = 16),
    normalized_name TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    password_phc TEXT NOT NULL,
    credential_version INTEGER NOT NULL DEFAULT 1 CHECK(credential_version >= 1),
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active', 'disabled')),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL
) STRICT;

CREATE TABLE identity_sessions (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    account_id BLOB NOT NULL CHECK(length(account_id) = 16),
    token_sha256 BLOB NOT NULL UNIQUE CHECK(length(token_sha256) = 32),
    csrf_sha256 BLOB NOT NULL CHECK(length(csrf_sha256) = 32),
    idle_expires_at_us INTEGER NOT NULL,
    absolute_expires_at_us INTEGER NOT NULL,
    credential_version INTEGER NOT NULL,
    last_used_at_us INTEGER NOT NULL,
    created_at_us INTEGER NOT NULL,
    revoked_at_us INTEGER,
    revoked_reason TEXT,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

CREATE INDEX identity_sessions_account_idx ON identity_sessions(account_id, revoked_at_us);

CREATE TABLE identity_login_throttles (
    candidate_sha256 BLOB NOT NULL CHECK(length(candidate_sha256) = 32),
    source_sha256 BLOB NOT NULL CHECK(length(source_sha256) = 32),
    window_started_at_us INTEGER NOT NULL,
    failure_count INTEGER NOT NULL CHECK(failure_count >= 0),
    retry_after_us INTEGER,
    updated_at_us INTEGER NOT NULL,
    PRIMARY KEY(candidate_sha256, source_sha256)
) STRICT;

PRAGMA user_version = 2;
