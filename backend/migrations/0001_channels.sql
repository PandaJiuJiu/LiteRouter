-- 初始渠道表。
-- `kind` 是早期版本里的"internal/external"区分字段，下一个迁移会删掉，
-- 这里保留以还原 schema 历史轨迹。
CREATE TABLE channels (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    base_url    TEXT NOT NULL DEFAULT '',
    api_key     TEXT NOT NULL,
    models      TEXT NOT NULL DEFAULT '',
    enabled     INTEGER NOT NULL DEFAULT 1,
    kind        TEXT NOT NULL DEFAULT 'external',
    created_at  INTEGER NOT NULL
);