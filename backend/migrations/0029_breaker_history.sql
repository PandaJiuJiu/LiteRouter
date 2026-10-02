-- 熔断器历史事件表：append-only，按时间倒序展示每个 (channel, model) 的
-- tripped / re-tripped / recovered / reset_all / reset_key 序列。
--
-- 之前熔断器只活在内存里（breaker.rs 顶部注释明确说"Process restart clears
-- every key"），运维无法回答"哪个模型什么时候熔断过、什么时候恢复"。这张表
-- 由调用 Breaker::record() / reset() / reset_key() 的位置写库：
--   - backend/src/proxy.rs::record_outcome_in_breaker
--   - backend/src/breaker_probe.rs::sweep (record 调用 + channel 缺失时的 reset_key)
--   - backend/src/breaker.rs::http_reset (清空所有 key)
--
-- 软引用 channel_name / target_model（与 log_attempts 同模式），不外键；
-- channel / model 被删除时事件仍然有意义（知道这个模型在这个渠道出过事）。
CREATE TABLE breaker_events (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_name TEXT    NOT NULL,
    target_model TEXT    NOT NULL,
    -- 'tripped' / 're-tripped' / 'recovered' / 'reset_all' / 'reset_key'
    event        TEXT    NOT NULL,
    reason       TEXT    NOT NULL DEFAULT '',
    -- tripped / re-tripped 时填触发退避（秒）；recovered / reset_* 时为 0
    backoff_secs INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);
CREATE INDEX idx_breaker_events_created_at ON breaker_events (created_at);
CREATE INDEX idx_breaker_events_key ON breaker_events (channel_name, target_model);