-- 全局配置键值表。目前只有 debug_logging（0=关闭，1=开启），后续其他
-- 全局开关也可以加进来，不另起表。
-- 用 INSERT OR IGNORE 保证幂等：已存在时不报错。
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT OR IGNORE INTO settings (key, value) VALUES ('debug_logging', '0');
