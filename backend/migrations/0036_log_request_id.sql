-- 每个客户端请求在到达时即分配一个稳定的 request_id，并预先写入一条
-- status_code = 0 的 logs 行表示"进行中"；请求完成时在同一行更新最终
-- 状态。这样：
--   1. 日志页刷新后，进行中的请求仍然可见（数据已持久化）；
--   2. pending 行有真实的 logs.id，详情页可以直接打开；
--   3. 前端用 request_id 关联 pending 事件与 final 事件。
ALTER TABLE logs ADD COLUMN request_id TEXT NOT NULL DEFAULT '';
CREATE INDEX idx_logs_request_id ON logs (request_id);