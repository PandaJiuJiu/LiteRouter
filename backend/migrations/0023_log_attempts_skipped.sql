-- log_attempts: 加 skipped 列，区分"熔断跳过"和"真上游失败"。
--
-- 之前熔断跳过的 hop 也被记成 ok=0 + status_code=0 + error='circuit breaker
-- open'，跟真上游失败混在一起，导致:
--   1) logs.failed_count 把它们算进"失败 N 次"
--   2) 前端列表页的"失败 N 次"徽章把它们算进去
--   3) 详情页 attempts 表里看到的"失败"很多其实根本没发 HTTP
--
-- skipped=1 表示这次跳过的原因是熔断（没有发出 HTTP 请求），不应当作失败
-- 计入任何统计指标。回填历史行: status_code=0 + error='circuit breaker open'
-- 是熔断跳过的特征标识（其他非 2xx 的 status_code 不会是 0，transport 错误
-- 用 -1）。
ALTER TABLE log_attempts ADD COLUMN skipped INTEGER NOT NULL DEFAULT 0;

UPDATE log_attempts
SET skipped = 1
WHERE skipped = 0
  AND status_code = 0
  AND error = 'circuit breaker open';