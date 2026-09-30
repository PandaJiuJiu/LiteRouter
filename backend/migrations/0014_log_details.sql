-- 日志详情：单次请求的元数据 + token 明细。
-- 刻意不存 prompt / 回复正文 —— 那是用户的隐私数据，而排障靠的是
-- "走了哪个渠道、映射到哪个模型、耗时多久、token 花在哪" 这些元信息。
--
--   latency_ms     从收到请求到上游响应头返回的耗时（流式时是首包时间）
--   stream          客户端是否要求了 stream
--   protocol        客户端协议：openai / anthropic
--   convert        网关是否做了协议转换：none / to_openai / to_anthropic
--   upstream_model  mappings 改写后真正发给上游的模型
--   error           上游/传输失败时的错误摘要（成功时为空）
-- token 明细取自上游 usage 块，支持 OpenAI 与 Anthropic 两种命名。
ALTER TABLE logs ADD COLUMN latency_ms    INTEGER NOT NULL DEFAULT 0;
ALTER TABLE logs ADD COLUMN stream        INTEGER NOT NULL DEFAULT 0;
ALTER TABLE logs ADD COLUMN protocol      TEXT    NOT NULL DEFAULT '';
ALTER TABLE logs ADD COLUMN convert       TEXT    NOT NULL DEFAULT '';
ALTER TABLE logs ADD COLUMN upstream_model TEXT   NOT NULL DEFAULT '';
ALTER TABLE logs ADD COLUMN error         TEXT    NOT NULL DEFAULT '';
-- Anthropic 缓存：命中部分按 0.1 倍计价，未命中部分按 1.25 倍计价，
-- 分开记才知道 prompt 到底贵在哪。
ALTER TABLE logs ADD COLUMN cache_read_tokens     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE logs ADD COLUMN cache_creation_tokens INTEGER NOT NULL DEFAULT 0;
-- OpenAI 侧对应 reasoning_tokens（o 系列思考 token），Anthropic 是
-- thinking 相关的 output token，已计入 completion_tokens。
ALTER TABLE logs ADD COLUMN reasoning_tokens INTEGER NOT NULL DEFAULT 0;
-- token 明细是排查计费异常的主要入口，按时间倒序查最近的大额请求。
CREATE INDEX idx_logs_created ON logs (created_at);
