-- 回填 logs.failed_count：之前熔断跳过的行也被算成了"失败"，现在 skipped
-- 列已经填好，重新算一次让历史的失败数跟实际真上游失败数对齐。
--
-- 一条 UPDATE 跑完所有行，列在子查询里直接取数；SQLite 用 log_id 上已有的
-- 索引（外键列）加速。零行的 log_attempts 子查询返回 NULL，COALESCE 到 0。
UPDATE logs SET failed_count = COALESCE((
    SELECT COUNT(*) FROM log_attempts
    WHERE log_id = logs.id AND ok = 0 AND skipped = 0
), 0);