-- 单目标升级为多目标（JSON 数组，按顺序尝试，失败自动 failover）。
-- targets 中每个元素是 {channel, model}，channel 为空表示"任意渠道"。
ALTER TABLE model_mappings ADD COLUMN targets TEXT NOT NULL DEFAULT '';
-- 把已有行的 target_model 包成 JSON 数组迁到 targets 列，保持向后兼容读。
UPDATE model_mappings
SET targets = json_array(target_model)
WHERE targets = '' OR targets IS NULL;