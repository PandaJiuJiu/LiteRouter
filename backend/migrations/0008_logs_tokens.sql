-- 用量统计：记录每次转发的 token 消耗，按 (token, created_at) 建索引
-- 支持每分钟 RPM 计算和按 UTC 日聚合 daily_token_limit。
ALTER TABLE logs ADD COLUMN prompt_tokens     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE logs ADD COLUMN completion_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE logs ADD COLUMN total_tokens      INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_logs_token_created ON logs (token_name, created_at);