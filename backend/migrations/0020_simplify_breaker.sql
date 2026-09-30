-- Circuit-breaker 简化：去掉滑动窗口 / 失败率 / 半开探测相关字段。
-- 这些字段之前的迁移 0018 / 0019 已写入 settings 表，本 migration 把
-- 它们清理掉，让前端 / settings 接口只看新字段。
--
-- 新字段（如果缺失则补上默认值）：
--   breaker_base_delay_secs          = 30   （原 breaker_base_open_secs）
--   breaker_max_delay_secs           = 300  （原 breaker_max_open_secs）
--   breaker_probe_interval_secs      = 30   （新增）
--   breaker_consecutive_failures     = 3    （保留，从 5 改为 3）
--
-- 旧字段直接 DELETE —— 这些值不再被读取。

INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_base_delay_secs', '30');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_max_delay_secs', '300');
INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_probe_interval_secs', '30');

-- 把旧阈值从 5 改成 3（这是新算法的合理默认值，区分滑动窗口时代的 5）
UPDATE settings SET value = '3' WHERE key = 'breaker_consecutive_failures' AND value = '5';

DELETE FROM settings WHERE key IN (
    'breaker_window_secs',
    'breaker_failure_rate',
    'breaker_min_samples',
    'breaker_client_error_consecutive_failures',
    'breaker_base_open_secs',
    'breaker_max_open_secs',
    'breaker_half_open_probes'
);