-- 移除冗余的 internal/external 区分（与 enabled 字段重复且语义混淆）。
-- SQLite ≥3.35 支持 DROP COLUMN。
ALTER TABLE channels DROP COLUMN kind;