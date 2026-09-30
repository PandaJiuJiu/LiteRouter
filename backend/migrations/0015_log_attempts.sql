-- 一次客户端请求 = logs 里的一行；转发过程中的每一次上游尝试 =
-- log_attempts 里的一行。此前是一次尝试一行，于是同一个用户请求 failover
-- 四次就会在列表里占四行，看不出它们其实是一个请求。
--
-- logs 行的 model / channel_name / status_code / token 明细取自**最后一次**
-- 尝试（即客户端真正拿到的那个结果）；每一次尝试的细节都在 log_attempts。
--
-- 刻意不加 FOREIGN KEY：本项目所有关联都是软引用（token_name /
-- channel_name 都是字符串而非 id），SQLite 默认不开启 foreign_keys 约束，
-- 加了也是摆设。删除由 db::cleanup_old_logs 显式做（先子后父）。
CREATE TABLE log_attempts (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    log_id         INTEGER NOT NULL,
    -- 第几次尝试，从 0 开始，按实际发生顺序
    seq            INTEGER NOT NULL,
    upstream_model TEXT    NOT NULL DEFAULT '',
    channel_name   TEXT    NOT NULL DEFAULT '',
    status_code    INTEGER NOT NULL DEFAULT 0,
    error          TEXT    NOT NULL DEFAULT '',
    latency_ms     INTEGER NOT NULL DEFAULT 0,
    convert        TEXT    NOT NULL DEFAULT '',
    -- 1 = 该次尝试成功（2xx）并返回给了客户端；0 = 失败并继续 failover
    ok             INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_log_attempts_log_id ON log_attempts (log_id);

-- 列表页每行都要显示"失败 N 次"角标，hover 还要列出失败的模型。子表此时
-- 才刚由当次请求写入，join 统计既不划算也拿不到（流式要等流结束才落库），
-- 直接把计数冗余在父行上。
ALTER TABLE logs ADD COLUMN failed_count INTEGER NOT NULL DEFAULT 0;
