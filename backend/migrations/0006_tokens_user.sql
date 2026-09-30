-- 多账号体系：每个令牌归属一个用户。
-- NULL 表示"孤儿"——遗留数据，仅 admin 可见。删除用户时不级联删除其
-- 令牌，而是把 user_id 置 NULL，保留历史用量数据。
ALTER TABLE tokens ADD COLUMN user_id INTEGER;