-- TTFT per hop attempt: time from request sent to first content byte received.
ALTER TABLE log_attempts ADD COLUMN ttft_ms INTEGER NOT NULL DEFAULT 0;
