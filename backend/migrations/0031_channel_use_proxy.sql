-- 渠道级代理开关。1=使用全局代理（需同时 proxy_enabled=1），0=直连。
-- 默认 0 保持现有行为：不走代理。
ALTER TABLE channels ADD COLUMN use_proxy INTEGER NOT NULL DEFAULT 0;