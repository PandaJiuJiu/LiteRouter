-- 全局代理设置。proxy_host 为 IP/主机名，proxy_port 为端口号。
-- 两者均为空/0 时视为未配置，渠道的 use_proxy 开关无效。
INSERT OR IGNORE INTO settings (key, value) VALUES ('proxy_host', '');
INSERT OR IGNORE INTO settings (key, value) VALUES ('proxy_port', '0');