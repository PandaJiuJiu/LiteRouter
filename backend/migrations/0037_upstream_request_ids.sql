-- 上游回给我们、用来在它自己日志里定位这一次请求的标识，按 hop 各存一份。
--
-- 为什么需要：中转站／云厂商是靠响应头里的 request-id / cf-ray 这类 ID 找
-- 到它服务过的那一次请求的。把它按 hop 记下来，"两个并发会话内容串台"这类
-- 问题才可举证——管理员可以把「这两个 log 行、这两组 ID、这两份 req/resp」
-- 直接扔给上游，而不是靠猜。之前这些头只落在 opt-in 的 debug capture 里，
-- 没开抓取就什么证据都没有。
--
-- 三个 ID 各有用途，不是冗余：request_id 是供应商主 ID（各家头名不同，
-- 取第一个非空的）；cf_ray 是 Cloudflare 边缘节点，多个中转站共用一个
-- cf-ray 后面的不同进程，不同值说明后端进程不同；generation_id 是
-- OpenRouter 那类按 generation 计费的标识，跟 request_id 不是一回事。
ALTER TABLE log_attempts ADD COLUMN upstream_request_id TEXT NOT NULL DEFAULT '';
ALTER TABLE log_attempts ADD COLUMN upstream_cf_ray TEXT NOT NULL DEFAULT '';
ALTER TABLE log_attempts ADD COLUMN upstream_generation_id TEXT NOT NULL DEFAULT '';

-- 客户端自己带来的 request_id（请求头 x-request-id），转发给上游的同时也
-- 回写给客户端（响应头 x-litrouter-request-id 回显同一值）。记在 logs 而
-- 不是 log_attempts：一个客户端请求无论 failover 多少跳都只有一行。
--
-- 它的作用是"客户端报障时能凭这个值反查到具体的 log 行"，所以单独一列而不是
-- 塞进 request_id：request_id 的格式是 {token}|{model}|{nanos}，是网关内部
-- 给前端关联 pending/final 事件用的，跟客户端侧的 ID 是两个东西。
ALTER TABLE logs ADD COLUMN client_request_id TEXT NOT NULL DEFAULT '';
