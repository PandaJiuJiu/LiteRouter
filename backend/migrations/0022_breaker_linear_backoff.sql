-- Circuit-breaker 第三次调整：加入 max_delay，并把退避改回线性递增。
--
-- 上一轮 (0021) 把 breaker_max_delay_secs 删了，认为不需要封顶。
-- 但用户反馈希望退避随失败次数增长（避免对持续故障的上游持续打探测），
-- 又有上限以免无限累积 —— 因此：
--
--   * 退避算法：线性递增（每次 +base_delay）
--   * 上限：max_delay_secs，默认 600s
--   * 阶梯：base_delay=30s 时是 30 → 60 → 90 → … → 600 (20 次后到顶)
--
-- 新字段（缺失则补默认值）：
--   breaker_max_delay_secs = 600

INSERT OR IGNORE INTO settings (key, value) VALUES ('breaker_max_delay_secs', '600');