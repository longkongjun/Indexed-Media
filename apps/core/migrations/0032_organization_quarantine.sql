-- A2/A3：organization_plans 的固定操作放开可恢复删除（quarantine）。
-- SQLite 不能就地修改 CHECK；按官方重建流程换表：关闭外键检查后重命名旧表、
-- 建新表放开 operation CHECK、原样搬运行数据、删除旧表并恢复索引与外键。

PRAGMA foreign_keys=OFF;

CREATE TABLE organization_plans_rebuilt (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    file_revision_id BLOB NOT NULL CHECK(length(file_revision_id)=16),
    selected_identity_id BLOB CHECK(selected_identity_id IS NULL OR length(selected_identity_id)=16),
    version INTEGER NOT NULL CHECK(version>=1),
    target_id BLOB NOT NULL CHECK(length(target_id)=16),
    snapshot_id BLOB NOT NULL UNIQUE CHECK(length(snapshot_id)=16),
    source_root_id TEXT NOT NULL CHECK(length(source_root_id) BETWEEN 1 AND 64),
    source_relative_path TEXT NOT NULL CHECK(length(source_relative_path) BETWEEN 1 AND 4096),
    destination_root_id TEXT NOT NULL CHECK(length(destination_root_id) BETWEEN 1 AND 64),
    destination_relative_path TEXT NOT NULL CHECK(length(destination_relative_path) BETWEEN 1 AND 4096),
    operation TEXT NOT NULL CHECK(operation IN ('move','copy','hardlink','quarantine')),
    naming TEXT NOT NULL CHECK(length(naming) BETWEEN 1 AND 512),
    authorization TEXT NOT NULL CHECK(authorization IN ('automatic','one-time','paused')),
    risk_codes_json TEXT NOT NULL CHECK(json_valid(risk_codes_json) AND length(risk_codes_json)<=4096),
    config_fingerprint BLOB NOT NULL CHECK(length(config_fingerprint)=32),
    created_at_us INTEGER NOT NULL,
    UNIQUE(task_id,version),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(file_revision_id) REFERENCES discovery_file_revisions(id),
    FOREIGN KEY(snapshot_id) REFERENCES organization_config_snapshots(id)
) STRICT;

INSERT INTO organization_plans_rebuilt
SELECT id,account_id,task_id,file_revision_id,selected_identity_id,version,target_id,snapshot_id,
       source_root_id,source_relative_path,destination_root_id,destination_relative_path,
       operation,naming,authorization,risk_codes_json,config_fingerprint,created_at_us
FROM organization_plans;

DROP TABLE organization_plans;

ALTER TABLE organization_plans_rebuilt RENAME TO organization_plans;

CREATE INDEX organization_plans_current_idx
ON organization_plans(account_id,task_id,version DESC,id DESC);

PRAGMA foreign_keys=ON;

-- 同一迁移内放开 journal kind 的 CHECK（新增 'quarantine'）：
-- organization_local_results.file_journal_id 引用 journals 表，同样走
-- legacy_alter_table 重建以保持引用指向原表名。

PRAGMA foreign_keys=OFF;

CREATE TABLE organization_journals_rebuilt (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    account_id BLOB NOT NULL CHECK(length(account_id)=16),
    task_id BLOB NOT NULL CHECK(length(task_id)=16),
    plan_id BLOB NOT NULL CHECK(length(plan_id)=16),
    plan_version INTEGER NOT NULL CHECK(plan_version>=1),
    operation_id BLOB NOT NULL CHECK(length(operation_id)=16),
    parent_operation_id BLOB CHECK(parent_operation_id IS NULL OR length(parent_operation_id)=16),
    kind TEXT NOT NULL CHECK(kind IN ('copy','move','hardlink','quarantine','source-removal','nfo')),
    status TEXT NOT NULL CHECK(status IN
        ('prepared','executing','applied','verified','compensated','manual-review')),
    source_root_id TEXT,
    source_relative_path TEXT,
    expected_source_identity BLOB,
    expected_source_size INTEGER CHECK(expected_source_size IS NULL OR expected_source_size>=0),
    expected_source_modified_at_ns INTEGER,
    destination_root_id TEXT NOT NULL,
    destination_relative_path TEXT NOT NULL,
    applied_target_identity BLOB,
    applied_size INTEGER CHECK(applied_size IS NULL OR applied_size>=0),
    applied_sha256 BLOB CHECK(applied_sha256 IS NULL OR length(applied_sha256)=32),
    nfo_preexisting INTEGER CHECK(nfo_preexisting IS NULL OR nfo_preexisting IN (0,1)),
    nfo_outcome TEXT CHECK(nfo_outcome IS NULL OR nfo_outcome IN ('preserved','generated')),
    reason TEXT,
    projection_version INTEGER NOT NULL CHECK(projection_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    UNIQUE(plan_id,operation_id),
    CHECK((source_root_id IS NULL)=(source_relative_path IS NULL)),
    CHECK((source_root_id IS NULL)=(expected_source_identity IS NULL)),
    CHECK((source_root_id IS NULL)=(expected_source_size IS NULL)),
    CHECK((source_root_id IS NULL)=(expected_source_modified_at_ns IS NULL)),
    CHECK((applied_target_identity IS NULL)=(applied_size IS NULL)),
    CHECK((applied_target_identity IS NULL)=(applied_sha256 IS NULL)),
    CHECK(kind='nfo' OR (nfo_preexisting IS NULL AND nfo_outcome IS NULL)),
    CHECK(kind!='nfo' OR nfo_preexisting IS NOT NULL),
    FOREIGN KEY(account_id) REFERENCES identity_accounts(id),
    FOREIGN KEY(task_id) REFERENCES tasks_processing_tasks(id),
    FOREIGN KEY(plan_id) REFERENCES organization_plans(id)
) STRICT;

INSERT INTO organization_journals_rebuilt
SELECT * FROM organization_file_operation_journals;

DROP TABLE organization_file_operation_journals;

ALTER TABLE organization_journals_rebuilt RENAME TO organization_file_operation_journals;

CREATE INDEX organization_journals_task_idx
ON organization_file_operation_journals(account_id,task_id,created_at_us,id);

CREATE INDEX organization_journals_recovery_idx
ON organization_file_operation_journals(status,updated_at_us,id)
WHERE status IN ('prepared','executing','applied','manual-review');

PRAGMA foreign_keys=ON;

-- profile 固定操作同样放开 quarantine（无表引用 organization_profiles，重建安全）。

PRAGMA foreign_keys=OFF;

CREATE TABLE organization_profiles_rebuilt (
    target_id BLOB PRIMARY KEY NOT NULL CHECK(length(target_id)=16),
    operation TEXT NOT NULL CHECK(operation IN ('move','copy','hardlink','quarantine')),
    naming_pattern TEXT NOT NULL CHECK(naming_pattern IN ('movie','series','generic-numbered')),
    nfo_policy TEXT NOT NULL CHECK(nfo_policy IN ('preserve-only','generate-missing')),
    automatic INTEGER NOT NULL CHECK(automatic IN (0,1)),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL,
    FOREIGN KEY(target_id) REFERENCES organization_targets(id) ON DELETE CASCADE
) STRICT;

INSERT INTO organization_profiles_rebuilt SELECT * FROM organization_profiles;

DROP TABLE organization_profiles;

ALTER TABLE organization_profiles_rebuilt RENAME TO organization_profiles;

PRAGMA foreign_keys=ON;

-- 计划配置快照的固定操作同样放开 quarantine；
-- snapshot_rules 以 WITHOUT ROWID 表引用本表，引用在 RENAME 后保持原名。

PRAGMA foreign_keys=OFF;

CREATE TABLE organization_config_snapshots_rebuilt (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    target_id BLOB NOT NULL CHECK(length(target_id)=16),
    target_config_version INTEGER NOT NULL CHECK(target_config_version>=1),
    kind TEXT NOT NULL CHECK(kind IN ('movie','series','generic-video')),
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 100),
    root_id TEXT NOT NULL CHECK(length(root_id) BETWEEN 1 AND 64),
    relative_path_display TEXT NOT NULL CHECK(length(relative_path_display) BETWEEN 1 AND 4096),
    operation TEXT NOT NULL CHECK(operation IN ('move','copy','hardlink','quarantine')),
    naming_pattern TEXT NOT NULL CHECK(naming_pattern IN ('movie','series','generic-numbered')),
    nfo_policy TEXT NOT NULL CHECK(nfo_policy IN ('preserve-only','generate-missing')),
    automatic INTEGER NOT NULL CHECK(automatic IN (0,1)),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    created_at_us INTEGER NOT NULL
) STRICT;

INSERT INTO organization_config_snapshots_rebuilt SELECT * FROM organization_config_snapshots;

DROP TABLE organization_config_snapshots;

ALTER TABLE organization_config_snapshots_rebuilt RENAME TO organization_config_snapshots;

PRAGMA foreign_keys=ON;

-- DROP TABLE 会连带删除该表的触发器；重建被重建表上的不可变触发器。

CREATE TRIGGER organization_config_snapshots_immutable
BEFORE UPDATE ON organization_config_snapshots
BEGIN SELECT RAISE(ABORT, 'organization config snapshot is immutable'); END;

CREATE TRIGGER organization_plans_immutable
BEFORE UPDATE ON organization_plans
BEGIN SELECT RAISE(ABORT, 'organization plan is immutable'); END;

PRAGMA user_version = 32;
