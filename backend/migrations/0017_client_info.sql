-- 记录请求的来源信息，方便定位是哪个客户端/哪个 IP 发起的调用。
-- client_ip: 从 X-Forwarded-For（反向代理）或连接地址获取，尽量取第一个真实 IP
-- user_agent: 客户端的 User-Agent 头，标识 SDK / curl / 浏览器 / 自定义应用
ALTER TABLE logs ADD COLUMN client_ip  TEXT NOT NULL DEFAULT '';
ALTER TABLE logs ADD COLUMN user_agent TEXT NOT NULL DEFAULT '';
