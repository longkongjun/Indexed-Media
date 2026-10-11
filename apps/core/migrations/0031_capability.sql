-- AI 能力协议（media-capability.v1）的持久化事实。
-- 调用记录支撑幂等重放（同 request_id 返回缓存结果）；
-- 事件表按 sequence 单调递增，供 AI 断线后按 cursor 补发。

CREATE TABLE IF NOT EXISTS capability_calls (
    request_id      BLOB PRIMARY KEY,
    idempotency_key TEXT NOT NULL,
    capability      TEXT NOT NULL,
    actor_kind      TEXT NOT NULL,
    actor_id        TEXT NOT NULL,
    status          TEXT NOT NULL,
    result_json     TEXT,
    error_json      TEXT,
    created_at_us   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_capability_calls_idem
    ON capability_calls(idempotency_key, capability);

CREATE TABLE IF NOT EXISTS capability_events (
    sequence       INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id       BLOB NOT NULL UNIQUE,
    event_type     TEXT NOT NULL,
    occurred_at_us INTEGER NOT NULL,
    task_id        BLOB,
    request_id     BLOB,
    operation_id   TEXT,
    state          TEXT,
    progress_json  TEXT,
    evidence_json  TEXT,
    error_json     TEXT
);

CREATE INDEX IF NOT EXISTS idx_capability_events_task
    ON capability_events(task_id, sequence);

PRAGMA user_version = 31;
