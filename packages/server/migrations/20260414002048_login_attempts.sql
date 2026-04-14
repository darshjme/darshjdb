-- DarshJDB: Login attempt audit log for brute-force rate limiting.
--
-- Every POST /api/auth/signin call records a row BEFORE the password
-- check (success=false). On successful verification the row is flipped
-- to success=true. The auth handler then counts recent failures for
-- exponential throttling (after 5) and account lock (after 10).
--
-- Idempotent — safe to run multiple times.

CREATE TABLE IF NOT EXISTS login_attempts (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email        TEXT        NOT NULL,
    ip_address   INET        NOT NULL,
    attempted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    success      BOOLEAN     NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS idx_login_attempts_email_time
    ON login_attempts (email, attempted_at DESC);

CREATE INDEX IF NOT EXISTS idx_login_attempts_ip_time
    ON login_attempts (ip_address, attempted_at DESC);
