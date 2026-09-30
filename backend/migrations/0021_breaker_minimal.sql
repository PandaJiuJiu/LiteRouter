-- Circuit-breaker 第二轮简化：去掉 consecutive-threshold / max-delay。
-- 上一次迁移 (0020) 已经删掉了滑动窗口时代的字段，但保留了
-- consecutive_failures 和 max_delay_secs。本次把这两个也一并删除：
--
--   * breaker_consecutive_failures —— 不再有「连续 N 次才熔断」的概念
--   * breaker_max_delay_secs       —— 失败冷却不翻倍也不封顶,只用 base_delay
--
-- 剩下的字段就是最终集合：
--   breaker_enabled
--   breaker_base_delay_secs
--   breaker_probe_interval_secs

DELETE FROM settings WHERE key IN (
    'breaker_consecutive_failures',
    'breaker_max_delay_secs'
);