-- 多账号体系核心表。
--   password_hash  PBKDF2-HMAC-SHA256 32 字节输出，hex 编码存 64 字符
--   password_salt  16 字节随机 salt，hex 编码存 32 字符
--   is_admin       1=管理员（可管理用户、渠道、路由）；0=普通用户
CREATE TABLE users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    password_salt TEXT NOT NULL,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);