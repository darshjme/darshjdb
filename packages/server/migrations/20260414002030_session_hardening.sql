-- DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
-- Migration: harden the `sessions` table for VYASA Phase 0.4.
--
-- Adds columns required for absolute timeout, overflow eviction, refresh-token
-- hash storage with uniqueness, structured revocation, and per-device IP/UA
-- forensics. Idempotent — safe to run multiple times.
--
-- Notes
-- =====
-- * The legacy `revoked BOOLEAN` column is preserved for backwards compatibility
--   with code paths still touching it; the canonical "is active" predicate
--   going forward is `revoked_at IS NULL`. Code that revokes a session sets
--   both columns in lockstep.
-- * `device_fingerprint`, `user_agent`, and `refresh_token_hash` already exist
--   in the bootstrap schema (`ensure_auth_schema`), so the ADD here is a no-op
--   for existing deployments — but we re-state them so a database created from
--   migrations alone matches production.
-- * Uniqueness on `refresh_token_hash` is enforced via the partial unique index
--   below rather than a table constraint, which is safe in the presence of
--   historical duplicates provided we revoke colliding rows first (see the
--   "Cleanup" block).

-- ── 1. Add new columns ────────────────────────────────────────────────────
ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS refresh_token_hash    TEXT,
    ADD COLUMN IF NOT EXISTS device_fingerprint    TEXT        NOT NULL DEFAULT '',
    ADD COLUMN IF NOT EXISTS ip_address            INET,
    ADD COLUMN IF NOT EXISTS user_agent            TEXT,
    ADD COLUMN IF NOT EXISTS last_active_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN IF NOT EXISTS absolute_expires_at   TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '24 hours'),
    ADD COLUMN IF NOT EXISTS revoked_at            TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS revoke_reason         TEXT;

-- ── 2. Backfill: align legacy `revoked` flag with `revoked_at` ────────────
-- Any session that was revoked under the old boolean schema should also have
-- a structured `revoked_at` so the new partial indexes treat it correctly.
UPDATE sessions
   SET revoked_at = COALESCE(revoked_at, now()),
       revoke_reason = COALESCE(revoke_reason, 'legacy_revoked')
 WHERE revoked = true
   AND revoked_at IS NULL;

-- ── 3. Cleanup: pre-resolve duplicates before unique indexes ──────────────
-- Revoke historical duplicates so the partial unique indexes below can be
-- created without violation. Keeps the most recent row per (user, device).
WITH ranked AS (
    SELECT session_id,
           ROW_NUMBER() OVER (
               PARTITION BY user_id, device_fingerprint
               ORDER BY created_at DESC, session_id DESC
           ) AS rn
      FROM sessions
     WHERE revoked_at IS NULL
)
UPDATE sessions s
   SET revoked_at = now(),
       revoke_reason = 'dedupe_on_migration',
       revoked = true
  FROM ranked r
 WHERE s.session_id = r.session_id
   AND r.rn > 1;

-- Also dedupe on refresh_token_hash so the unique index does not collide.
WITH ranked_hash AS (
    SELECT session_id,
           ROW_NUMBER() OVER (
               PARTITION BY refresh_token_hash
               ORDER BY created_at DESC, session_id DESC
           ) AS rn
      FROM sessions
     WHERE revoked_at IS NULL
       AND refresh_token_hash IS NOT NULL
)
UPDATE sessions s
   SET revoked_at = now(),
       revoke_reason = 'dedupe_on_migration',
       revoked = true
  FROM ranked_hash r
 WHERE s.session_id = r.session_id
   AND r.rn > 1;

-- ── 4. Indexes ────────────────────────────────────────────────────────────

-- Hot path: enumerate active sessions for a given user.
CREATE INDEX IF NOT EXISTS idx_sessions_user_active
    ON sessions (user_id)
    WHERE revoked_at IS NULL;

-- One active session per (user, device) — the foundation of overflow eviction
-- and the device-binding security guarantee.
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_user_device
    ON sessions (user_id, device_fingerprint)
    WHERE revoked_at IS NULL;

-- Lookup by refresh-token hash on the refresh path. Unique among active rows
-- so a leaked-then-rotated token cannot collide with a live session.
CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_refresh_hash
    ON sessions (refresh_token_hash)
    WHERE revoked_at IS NULL;
