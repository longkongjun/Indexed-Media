-- M4：通知中心。应用内通知为唯一必达通道；通用出站 Webhook 是可选附加投递，
-- 配置独立加密/明文字段与健康分类，投递失败绝不影响本地任务（外部不是成功条件）。

CREATE TABLE notifications (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    kind TEXT NOT NULL CHECK(kind IN (
        'review.waiting', 'organization.paused', 'organization.rolled-back', 'system.notice'
    )),
    title TEXT NOT NULL CHECK(length(title) BETWEEN 1 AND 200),
    body TEXT NOT NULL CHECK(length(body) BETWEEN 1 AND 2000),
    task_id BLOB,
    created_at_us INTEGER NOT NULL,
    read_at_us INTEGER,
    CHECK((kind IN ('review.waiting','organization.paused','organization.rolled-back')) = (task_id IS NOT NULL))
) STRICT;

CREATE INDEX notifications_inbox_idx
ON notifications(read_at_us IS NULL, created_at_us DESC, id);

CREATE TABLE notification_webhooks (
    id BLOB PRIMARY KEY NOT NULL CHECK(length(id)=16),
    target_url TEXT NOT NULL CHECK(target_url LIKE 'https://%'),
    display_name TEXT CHECK(display_name IS NULL OR length(display_name) BETWEEN 1 AND 120),
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    config_version INTEGER NOT NULL CHECK(config_version>=1),
    health TEXT NOT NULL CHECK(health IN ('unconfigured','healthy','degraded','unavailable','unauthorized')),
    last_delivery_us INTEGER,
    created_at_us INTEGER NOT NULL,
    updated_at_us INTEGER NOT NULL
) STRICT;

PRAGMA user_version = 34;
