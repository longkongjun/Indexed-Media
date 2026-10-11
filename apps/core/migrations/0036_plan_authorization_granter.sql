-- SA-1-001 修复：一次性授权回执记录授权者身份（销 SA-2-001）。
-- 列允许 NULL 以兼容存量回执行；新回执由代码层强制写入（capability 通道不再
-- 产生授权回执，Web 人工路径是唯一授权产生方）。

ALTER TABLE organization_plan_authorization_receipts
  ADD COLUMN granted_by_kind TEXT CHECK(granted_by_kind IN ('web'));
ALTER TABLE organization_plan_authorization_receipts
  ADD COLUMN granted_by_id BLOB CHECK(length(granted_by_id) = 16);

PRAGMA user_version = 36;
