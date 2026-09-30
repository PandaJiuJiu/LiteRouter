-- 4xx 连续阈值：从 1 次改为 2 次触发熔断。让单次瞬时 4xx 不至于立即熔断，
-- 同时也比 5xx 的默认 5 次阈值更激进——4xx 在本系统里意味着"上游 channel
-- 没有这个模型 / channel 配置错"，是稳定事实，应该更快记住。
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_client_error_consecutive_failures', '2');
