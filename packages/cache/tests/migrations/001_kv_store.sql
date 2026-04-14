-- DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
-- Migration: kv_store + kv_streams tables backing the L2 persistent cache tier.
-- Idempotent — safe to run multiple times.

-- ── kv_store: primary KV / hash / list / zset backing table ───────────────
CREATE TABLE IF NOT EXISTS kv_store (
    key         TEXT        PRIMARY KEY,
    value       BYTEA       NOT NULL,
    kind        TEXT        NOT NULL DEFAULT 'string',
    expires_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    size_bytes  INTEGER     NOT NULL DEFAULT 0,
    metadata    JSONB       DEFAULT '{}'
);

-- Partial index: only rows with a TTL participate in the expiry sweep scan.
CREATE INDEX IF NOT EXISTS idx_kv_expires
    ON kv_store (expires_at)
    WHERE expires_at IS NOT NULL;

-- ── kv_streams: append-only stream entries (XADD / XRANGE / XREAD) ────────
CREATE TABLE IF NOT EXISTS kv_streams (
    stream_key  TEXT        NOT NULL,
    entry_id    TEXT        NOT NULL,
    fields      JSONB       NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (stream_key, entry_id)
);

CREATE INDEX IF NOT EXISTS idx_kv_streams_key_time
    ON kv_streams (stream_key, created_at DESC);
