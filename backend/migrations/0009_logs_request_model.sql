-- 区分"客户端请求的模型"和"实际转发到的上游模型"。
-- 例如用户请求 `my-gpt4o`（被 mappings 重写为 `gpt-4o-2024-08-06`），
-- request_model 记 `my-gpt4o`，model 记 `gpt-4o-2024-08-06`，便于排查
-- "为什么我的请求被转发到了某个奇怪的模型"。
ALTER TABLE logs ADD COLUMN request_model TEXT NOT NULL DEFAULT '';