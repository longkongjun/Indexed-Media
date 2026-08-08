ALTER TABLE download_tasks
ADD COLUMN blocked_config_version INTEGER
CHECK(blocked_config_version IS NULL OR blocked_config_version>=1);

CREATE INDEX download_tasks_blocked_config_idx
ON download_tasks(connection_id,blocked_config_version,status)
WHERE blocked_config_version IS NOT NULL;
