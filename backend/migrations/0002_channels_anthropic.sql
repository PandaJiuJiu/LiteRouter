-- 支持双协议（OpenAI + Anthropic）。当一个上游同时支持两种协议（如
-- Volcengine Ark），让用户填两个 base_url，用同一组 api_key 即可。
ALTER TABLE channels ADD COLUMN base_url_anthropic TEXT NOT NULL DEFAULT '';