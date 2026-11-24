-- 请求体体积（字节），在请求到达时、转发前就记录，与成败无关。
--
-- 为什么需要：出错的任务（上游没回 usage）在 logs 里 prompt_tokens = 0，
-- 管理员看不出"这次请求到底多大"。一个 INTEGER 就能回答"多大"这个问题，
-- 零隐私、零存储压力，且因为 size 在 relay 入口就已知，出错/成功/被
-- breaker 跳过所有路径都能填上。
--
-- 口径：原始 HTTP 请求体的字节数（client → gateway 这一跳），不是 token
-- 数，不计入任何 token 消耗。与 prompt_tokens 完全独立。
ALTER TABLE logs ADD COLUMN request_bytes INTEGER NOT NULL DEFAULT 0;