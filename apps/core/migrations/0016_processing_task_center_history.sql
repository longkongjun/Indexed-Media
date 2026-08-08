DROP TRIGGER tasks_processing_order_after_insert;
DROP TRIGGER tasks_processing_order_after_update;

CREATE TABLE tasks_processing_task_order_history_next (
    revision INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    updated_at_us INTEGER NOT NULL,
    status TEXT NOT NULL CHECK(status IN (
        'queued','running','waiting-confirmation','paused','cancelled',
        'partial-success','completed','failed'
    )),
    stage TEXT NOT NULL CHECK(stage IN (
        'identification','planning','file-operation','nfo','completion'
    )),
    checkpoint TEXT NOT NULL CHECK(checkpoint IN (
        'pending','identification-complete','waiting-confirmation','dependency-blocked',
        'skipped-auxiliary','cancelled'
    )),
    decision_checkpoint TEXT CHECK(decision_checkpoint IS NULL OR decision_checkpoint IN (
        'manual-decision-pending','rematch-pending','generic-video-selected','planning-requested'
    )),
    current_task_decision_id BLOB CHECK(
        current_task_decision_id IS NULL OR length(current_task_decision_id)=16
    ),
    reason TEXT CHECK(reason IS NULL OR length(reason) BETWEEN 1 AND 128),
    recovering INTEGER NOT NULL CHECK(recovering IN (0,1)),
    attempt_count INTEGER NOT NULL CHECK(attempt_count>=1),
    next_retry_at_us INTEGER,
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id) ON DELETE CASCADE,
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id)
) STRICT;

INSERT INTO tasks_processing_task_order_history_next
(revision,task_id,account_id,updated_at_us,status,stage,checkpoint,decision_checkpoint,
 current_task_decision_id,reason,recovering,attempt_count,next_retry_at_us)
SELECT history.revision,history.task_id,history.account_id,history.updated_at_us,
       task.status,task.stage,task.checkpoint,task.decision_checkpoint,
       task.current_task_decision_id,task.reason,task.recovering,task.attempt_count,
       task.next_retry_at_us
FROM tasks_processing_task_order_history history
JOIN tasks_processing_tasks task ON task.id=history.task_id;

DROP TABLE tasks_processing_task_order_history;
ALTER TABLE tasks_processing_task_order_history_next
RENAME TO tasks_processing_task_order_history;

CREATE INDEX tasks_processing_order_snapshot_idx
ON tasks_processing_task_order_history(account_id,revision,task_id,updated_at_us);

CREATE INDEX tasks_processing_order_task_revision_idx
ON tasks_processing_task_order_history(task_id,revision DESC);

CREATE INDEX tasks_processing_center_snapshot_idx
ON tasks_processing_task_order_history(account_id,revision,status,stage,updated_at_us,task_id);

CREATE TRIGGER tasks_processing_order_after_insert
AFTER INSERT ON tasks_processing_tasks
BEGIN
    INSERT INTO tasks_processing_task_order_history
    (task_id,account_id,updated_at_us,status,stage,checkpoint,decision_checkpoint,
     current_task_decision_id,reason,recovering,attempt_count,next_retry_at_us)
    VALUES
    (NEW.id,NEW.account_id,NEW.updated_at_us,NEW.status,NEW.stage,NEW.checkpoint,
     NEW.decision_checkpoint,NEW.current_task_decision_id,NEW.reason,NEW.recovering,
     NEW.attempt_count,NEW.next_retry_at_us);
END;

CREATE TRIGGER tasks_processing_order_after_update
AFTER UPDATE OF updated_at_us ON tasks_processing_tasks
BEGIN
    INSERT INTO tasks_processing_task_order_history
    (task_id,account_id,updated_at_us,status,stage,checkpoint,decision_checkpoint,
     current_task_decision_id,reason,recovering,attempt_count,next_retry_at_us)
    VALUES
    (NEW.id,NEW.account_id,NEW.updated_at_us,NEW.status,NEW.stage,NEW.checkpoint,
     NEW.decision_checkpoint,NEW.current_task_decision_id,NEW.reason,NEW.recovering,
     NEW.attempt_count,NEW.next_retry_at_us);
END;
