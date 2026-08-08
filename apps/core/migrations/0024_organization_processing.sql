-- Expanding the existing status/checkpoint/reason enumerations weakens CHECK constraints without
-- changing any on-disk row. SQLite documents writable_schema as the transactional procedure for
-- this class of change. The guard tables make an unexpected prior schema fail the migration.
PRAGMA writable_schema = ON;

UPDATE sqlite_schema
SET sql = replace(
    replace(
        replace(
            sql,
            "status TEXT NOT NULL CHECK(status IN ('queued','running','waiting-confirmation','paused','cancelled'))",
            "status TEXT NOT NULL CHECK(status IN ('queued','running','waiting-confirmation','paused','cancelled','partial-success','completed','failed'))"
        ),
        "checkpoint TEXT NOT NULL CHECK(checkpoint IN ('pending','identification-complete','waiting-confirmation','dependency-blocked','skipped-auxiliary','cancelled'))",
        "checkpoint TEXT NOT NULL CHECK(checkpoint IN ('pending','identification-complete','planning-requested','plan-prepared','execution-authorized','planning-paused','file-operation-prepared','file-operation-executing','file-operation-verified','file-operation-manual-review','nfo-pending','nfo-verified','nfo-failed','local-result-prepared','catalog-committed','waiting-confirmation','dependency-blocked','skipped-auxiliary','cancelled'))"
    ),
    "'integration.rate-limited','integration.unavailable'",
    "'integration.rate-limited','integration.unavailable','organization.plan-paused','organization.io-temporary','organization.manual-review','organization.nfo-failed','organization.catalog-unavailable'"
)
WHERE type='table' AND name='tasks_processing_tasks';

UPDATE sqlite_schema
SET sql = replace(
    sql,
    "'pending','identification-complete','waiting-confirmation','dependency-blocked',
        'skipped-auxiliary','cancelled'",
    "'pending','identification-complete','planning-requested','plan-prepared',
        'execution-authorized','planning-paused','file-operation-prepared',
        'file-operation-executing','file-operation-verified','file-operation-manual-review',
        'nfo-pending','nfo-verified','nfo-failed','local-result-prepared',
        'catalog-committed','waiting-confirmation','dependency-blocked',
        'skipped-auxiliary','cancelled'"
)
WHERE type='table' AND name='tasks_processing_task_order_history';

PRAGMA schema_version = 240024;
PRAGMA writable_schema = OFF;

CREATE TEMP TABLE organization_processing_schema_guard (
    valid INTEGER NOT NULL CHECK(valid=1)
);

INSERT INTO organization_processing_schema_guard(valid)
SELECT CASE WHEN
    COUNT(*)=1
    AND instr(max(sql), "'partial-success','completed','failed'")>0
    AND instr(max(sql), "'catalog-committed'")>0
    AND instr(max(sql), "'organization.catalog-unavailable'")>0
THEN 1 ELSE 0 END
FROM sqlite_schema
WHERE type='table' AND name='tasks_processing_tasks';

INSERT INTO organization_processing_schema_guard(valid)
SELECT CASE WHEN
    COUNT(*)=1 AND instr(max(sql), "'catalog-committed'")>0
THEN 1 ELSE 0 END
FROM sqlite_schema
WHERE type='table' AND name='tasks_processing_task_order_history';

DROP TABLE organization_processing_schema_guard;

ALTER TABLE tasks_processing_tasks
ADD COLUMN organization_plan_id BLOB
    CHECK(organization_plan_id IS NULL OR length(organization_plan_id)=16)
    REFERENCES organization_plans(id);

ALTER TABLE tasks_processing_tasks
ADD COLUMN organization_result_id BLOB
    CHECK(organization_result_id IS NULL OR length(organization_result_id)=16)
    REFERENCES organization_local_results(id);

ALTER TABLE tasks_processing_tasks
ADD COLUMN catalog_media_item_id BLOB
    CHECK(catalog_media_item_id IS NULL OR length(catalog_media_item_id)=16)
    REFERENCES catalog_media_items(id);

CREATE TRIGGER organization_target_event_after_insert
AFTER INSERT ON organization_targets
BEGIN
    INSERT INTO platform_outbox_events
    (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
    VALUES (
        'organization-target.changed','1',NEW.id,
        json_object(
            'target_id',lower(hex(NEW.id)),
            'config_version',NEW.config_version,
            'change','created'
        ),
        NEW.updated_at_us
    );
END;

CREATE TRIGGER organization_target_event_after_update
AFTER UPDATE OF config_version ON organization_targets
WHEN OLD.config_version != NEW.config_version
BEGIN
    INSERT INTO platform_outbox_events
    (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
    VALUES (
        'organization-target.changed','1',NEW.id,
        json_object(
            'target_id',lower(hex(NEW.id)),
            'config_version',NEW.config_version,
            'change','updated'
        ),
        NEW.updated_at_us
    );
END;

CREATE TRIGGER organization_target_event_after_delete
AFTER DELETE ON organization_targets
BEGIN
    INSERT INTO platform_outbox_events
    (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
    VALUES (
        'organization-target.changed','1',OLD.id,
        json_object(
            'target_id',lower(hex(OLD.id)),
            'config_version',OLD.config_version,
            'change','deleted'
        ),
        OLD.updated_at_us
    );
END;

CREATE TRIGGER organization_result_event_after_insert
AFTER INSERT ON organization_local_results
BEGIN
    INSERT INTO platform_outbox_events
    (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
    VALUES (
        'organization-result.changed','1',NEW.id,
        json_object(
            'result_id',lower(hex(NEW.id)),
            'processing_task_id',lower(hex(NEW.task_id)),
            'version',NEW.version,
            'status',NEW.status
        ),
        NEW.updated_at_us
    );
END;

CREATE TRIGGER organization_result_event_after_update
AFTER UPDATE OF version,status ON organization_local_results
WHEN OLD.version != NEW.version OR OLD.status != NEW.status
BEGIN
    INSERT INTO platform_outbox_events
    (event_type,schema_version,aggregate_id,payload_json,committed_at_us)
    VALUES (
        'organization-result.changed','1',NEW.id,
        json_object(
            'result_id',lower(hex(NEW.id)),
            'processing_task_id',lower(hex(NEW.task_id)),
            'version',NEW.version,
            'status',NEW.status
        ),
        NEW.updated_at_us
    );
END;

PRAGMA user_version = 24;
