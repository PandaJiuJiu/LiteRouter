-- 单令牌配额：
--   rpm_limit         每分钟请求数；0 = 不限
--   daily_token_limit 当日（按 created_at 除以 86400 段）token 总数；0 = 不限
ALTER TABLE tokens ADD COLUMN rpm_limit INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tokens ADD COLUMN daily_token_limit INTEGER NOT NULL DEFAULT 0;