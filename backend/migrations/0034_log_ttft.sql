-- Time to First Token: how long the first token took to arrive (streaming only).
-- 0 for non-streaming requests (no meaningful TTFT).
ALTER TABLE logs ADD COLUMN ttft_ms INTEGER NOT NULL DEFAULT 0;
