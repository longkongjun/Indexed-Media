ALTER TABLE tasks_scan_tasks
    ADD COLUMN last_progress_event_at_us INTEGER;

CREATE INDEX platform_outbox_events_committed_id_idx
    ON platform_outbox_events(committed_at_us, id);

PRAGMA user_version = 7;
