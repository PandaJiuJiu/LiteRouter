-- 日志保留天数。
-- 默认 7 天，与 main.rs 历史行为一致；管理员可在 Settings 页调整。
-- 启动时 main 不再读它，每次 sweep 重新读 settings 表，所以热修改
-- 在下一次清理循环（约 1 小时）生效。
INSERT INTO settings (key, value) VALUES ('log_retention_days', '7')
ON CONFLICT(key) DO NOTHING;