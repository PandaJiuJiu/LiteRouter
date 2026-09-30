-- 模型路由别名：客户端请求 alias 时改写为 target_model 真实上游 id。
-- target_model 是单值字段，下一个迁移把它升级为 JSON 数组以支持多目标。
CREATE TABLE model_mappings (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    alias        TEXT NOT NULL UNIQUE,
    target_model TEXT NOT NULL,
    created_at   INTEGER NOT NULL
);