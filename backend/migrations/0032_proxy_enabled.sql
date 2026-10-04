-- 全局代理启用开关。0=不启用（即使配置了 proxy_host/port 也不走代理），
-- 1=启用。默认 0：配置代理服务器不应自动启用代理。
INSERT OR IGNORE INTO settings (key, value) VALUES ('proxy_enabled', '0');
