-- M4：家庭成员最小身份。家庭成员不进入 identity_accounts（单管理员约束保留），
-- 只在独立表中持有身份记录；管理端对他们完全关闭（MVP-WEB-003），他们直接使用外部播放器。

CREATE TABLE family_members (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id) = 16),
    normalized_name TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL CHECK(length(display_name) BETWEEN 1 AND 100),
    password_phc TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','disabled')),
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 35;
