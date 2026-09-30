-- 渠道官网地址。可选，仅作展示用。
ALTER TABLE channels ADD COLUMN website TEXT NOT NULL DEFAULT '';
