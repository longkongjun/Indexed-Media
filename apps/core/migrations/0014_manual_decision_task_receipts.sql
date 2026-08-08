CREATE TABLE tasks_processing_manual_decision_receipts (
    decision_id BLOB PRIMARY KEY NOT NULL CHECK(length(decision_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    checkpoint TEXT NOT NULL CHECK(checkpoint IN (
        'manual-decision-pending','rematch-pending','generic-video-selected','planning-requested'
    )),
    applied_at_us INTEGER NOT NULL,
    FOREIGN KEY(decision_id) REFERENCES identification_task_decisions(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id) ON DELETE CASCADE
) STRICT;

CREATE INDEX tasks_processing_manual_receipt_task
ON tasks_processing_manual_decision_receipts(task_id,applied_at_us,decision_id);

CREATE TABLE platform_event_consumer_receipts (
    consumer TEXT NOT NULL CHECK(consumer IN ('audit')),
    decision_id BLOB NOT NULL CHECK(length(decision_id)=16),
    consumed_at_us INTEGER NOT NULL,
    PRIMARY KEY(consumer,decision_id),
    FOREIGN KEY(decision_id) REFERENCES identification_task_decisions(id)
) WITHOUT ROWID, STRICT;
