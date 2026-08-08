CREATE TABLE tasks_idempotency_bindings (
    account_id BLOB NOT NULL CHECK(length(account_id) = 16),
    key_sha256 BLOB NOT NULL CHECK(length(key_sha256) = 32),
    action TEXT NOT NULL,
    target_id BLOB NOT NULL CHECK(length(target_id) = 16),
    result_task_id BLOB NOT NULL CHECK(length(result_task_id) = 16),
    created_at_us INTEGER NOT NULL,
    PRIMARY KEY(account_id, key_sha256),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(result_task_id) REFERENCES tasks_scan_tasks(id)
) WITHOUT ROWID, STRICT;

-- v4/v5 曾允许同一摘要被不同动作和目标复用。迁移保留 `tasks_idempotency_keys` 中的每份历史结果，
-- 并按 `account_id`、`key_sha256` 分组选择一个确定性绑定写入新表。未获选的历史结果仍留在旧表；
-- 后续按其动作或目标复用相同摘要时，运行时会因绑定不匹配返回 `request.conflict`。
INSERT INTO tasks_idempotency_bindings
    (account_id,key_sha256,action,target_id,result_task_id,created_at_us)
SELECT account_id,key_sha256,action,target_id,result_task_id,created_at_us
FROM (
    SELECT account_id,key_sha256,action,target_id,result_task_id,created_at_us,
           ROW_NUMBER() OVER (
               PARTITION BY account_id,key_sha256
               ORDER BY created_at_us ASC,action ASC,target_id ASC,result_task_id ASC
           ) AS binding_rank
    FROM tasks_idempotency_keys
)
WHERE binding_rank = 1;

ALTER TABLE discovery_scan_file_observations
    ADD COLUMN first_observed_seq INTEGER NOT NULL DEFAULT 0 CHECK(first_observed_seq >= 0);

UPDATE discovery_scan_file_observations AS observation
SET first_observed_seq = (
    SELECT COUNT(*)
    FROM discovery_scan_file_observations AS earlier
    WHERE earlier.scan_batch_id = observation.scan_batch_id
      AND earlier.discovered_file_id <= observation.discovered_file_id
);

CREATE INDEX discovery_scan_observations_snapshot_idx
    ON discovery_scan_file_observations(scan_batch_id, first_observed_seq, discovered_file_id);

ALTER TABLE tasks_scan_errors
    ADD COLUMN first_seen_seq INTEGER NOT NULL DEFAULT 0 CHECK(first_seen_seq >= 0);

UPDATE tasks_scan_errors AS scan_error
SET first_seen_seq = (
    SELECT COUNT(*)
    FROM tasks_scan_errors AS earlier
    WHERE earlier.attempt_id = scan_error.attempt_id
      AND (earlier.first_seen_at_us < scan_error.first_seen_at_us
           OR (earlier.first_seen_at_us = scan_error.first_seen_at_us
               AND earlier.id <= scan_error.id))
);

CREATE INDEX tasks_scan_errors_snapshot_idx
    ON tasks_scan_errors(attempt_id, first_seen_seq, first_seen_at_us, id);

CREATE TABLE tasks_scan_task_order_history (
    revision INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id BLOB NOT NULL CHECK(length(task_id) = 16),
    account_id BLOB NOT NULL CHECK(length(account_id) = 16),
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(task_id) REFERENCES tasks_scan_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

INSERT INTO tasks_scan_task_order_history (task_id,account_id,updated_at_us)
SELECT id,account_id,updated_at_us
FROM tasks_scan_tasks
ORDER BY created_at_us ASC,id ASC;

CREATE INDEX tasks_scan_task_order_snapshot_idx
    ON tasks_scan_task_order_history(account_id,revision,task_id,updated_at_us);
CREATE INDEX tasks_scan_task_order_task_revision_idx
    ON tasks_scan_task_order_history(task_id,revision DESC);

CREATE TRIGGER tasks_scan_task_order_after_insert
AFTER INSERT ON tasks_scan_tasks
BEGIN
    INSERT INTO tasks_scan_task_order_history (task_id,account_id,updated_at_us)
    VALUES (NEW.id,NEW.account_id,NEW.updated_at_us);
END;

CREATE TRIGGER tasks_scan_task_order_after_update
AFTER UPDATE OF updated_at_us ON tasks_scan_tasks
BEGIN
    INSERT INTO tasks_scan_task_order_history (task_id,account_id,updated_at_us)
    VALUES (NEW.id,NEW.account_id,NEW.updated_at_us);
END;

PRAGMA user_version = 6;
