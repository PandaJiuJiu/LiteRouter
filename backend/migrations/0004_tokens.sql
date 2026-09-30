-- 内部令牌（对客户端暴露的 sk-xxx）。
--   name         显示用，便于运维识别
--   key          实际鉴权串，全局唯一
--   enabled      0/1 开关；停用时 /v1 直接 401
--   accessed_at  最后一次成功转发的时间戳，用于排查"令牌是否在用"
CREATE TABLE tokens (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    key         TEXT NOT NULL UNIQUE,
    enabled     INTEGER NOT NULL DEFAULT 1,
    created_at  INTEGER NOT NULL,
    accessed_at INTEGER NOT NULL DEFAULT 0
);