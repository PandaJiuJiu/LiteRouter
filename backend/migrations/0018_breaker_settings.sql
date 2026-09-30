-- 熔断器默认配置。沿用 settings 表 key/value 模式；上线时通过
-- sqlx::migrate! 自动加载。已存在的 key 不会被覆盖（INSERT OR IGNORE）。
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_enabled', '1');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_window_secs', '60');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_failure_rate', '0.5');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_min_samples', '5');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_consecutive_failures', '5');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_base_open_secs', '30');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_half_open_probes', '3');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_max_open_secs', '300');