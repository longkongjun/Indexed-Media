-- A reservation is never automatically reclaimed: after a crash, domain facts/journal
-- must establish whether work completed before any new execution may be authorized.
CREATE TABLE capability_request_claims (
    request_id BLOB PRIMARY KEY NOT NULL,
    server_principal TEXT NOT NULL,
    capability TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_fingerprint BLOB NOT NULL CHECK(length(request_fingerprint) = 32),
    state TEXT NOT NULL CHECK(state IN ('pending','completed')),
    created_at_us INTEGER NOT NULL,
    UNIQUE(server_principal, capability, idempotency_key)
);
-- Historical calls lack the original envelope and cannot be safely backfilled.
-- They remain immutable audit records and block request/idempotency reuse.
PRAGMA user_version = 40;
