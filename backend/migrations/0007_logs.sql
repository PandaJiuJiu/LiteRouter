-- 调用日志。每条记录是一次转发到上游的成功或失败请求。
--   status_code  上游返回的 HTTP 状态码
CREATE TABLE logs (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    token_name   TEXT NOT NULL,
    model        TEXT NOT NULL,
    channel_name TEXT NOT NULL,
    status_code  INTEGER NOT NULL,
    created_at   INTEGER NOT NULL
);