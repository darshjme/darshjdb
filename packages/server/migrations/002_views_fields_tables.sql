-- DarshJDB: Supporting tables for features that don't fit EAV.
-- Idempotent -- safe to run multiple times.
--
-- Views, Fields, and Tables are stored as EAV triples (no new tables
-- needed for those). This migration adds dedicated tables for
-- high-volume, security-sensitive, or append-only data that benefits
-- from relational indexing.

-- ── Webhook deliveries ───────────────────────────────────────────
-- High-volume delivery log — one row per attempt per webhook.

CREATE TABLE IF NOT EXISTS webhook_deliveries (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    webhook_id      UUID        NOT NULL,
    event_kind      TEXT        NOT NULL,
    payload         JSONB       NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'pending',  -- pending, delivered, failed
    attempts        INTEGER     NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    response_status INTEGER,
    response_body   TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_webhook
    ON webhook_deliveries(webhook_id, created_at DESC);

-- ── API keys ─────────────────────────────────────────────────────
-- Separate table for security — not in EAV.

CREATE TABLE IF NOT EXISTS api_keys (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name        TEXT        NOT NULL,
    key_prefix  TEXT        NOT NULL,       -- first 8 chars for display
    key_hash    TEXT        NOT NULL,       -- blake3 hash
    scopes      JSONB       NOT NULL DEFAULT '["read"]',
    rate_limit  INTEGER,
    expires_at  TIMESTAMPTZ,
    created_by  UUID        REFERENCES users(id),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_used_at TIMESTAMPTZ,
    revoked     BOOLEAN     NOT NULL DEFAULT false
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_hash
    ON api_keys(key_hash) WHERE NOT revoked;

-- ── Activity log ─────────────────────────────────────────────────
-- High-volume append-only audit trail.

CREATE TABLE IF NOT EXISTS activity_log (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_type TEXT        NOT NULL,
    entity_id   UUID        NOT NULL,
    action      TEXT        NOT NULL,
    user_id     UUID,
    changes     JSONB,
    metadata    JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_activity_entity
    ON activity_log(entity_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_activity_user
    ON activity_log(user_id, created_at DESC);

-- ── Notifications ────────────────────────────────────────────────
-- In-app notification lifecycle.

CREATE TABLE IF NOT EXISTS notifications (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID        NOT NULL REFERENCES users(id),
    kind            TEXT        NOT NULL,
    title           TEXT        NOT NULL,
    body            TEXT,
    resource_type   TEXT,
    resource_id     UUID,
    read            BOOLEAN     NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_notifications_user
    ON notifications(user_id, read, created_at DESC);

-- ── Event log for KB extraction (DAF-inspired) ──────────────────
-- Structured event stream for knowledge-base extraction, replay,
-- and debugging.

CREATE TABLE IF NOT EXISTS ddb_events (
    id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    kind        TEXT        NOT NULL,
    entity_type TEXT,
    entity_id   UUID,
    attribute   TEXT,
    old_value   JSONB,
    new_value   JSONB,
    user_id     UUID,
    tx_id       BIGINT,
    metadata    JSONB,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_events_kind
    ON ddb_events(kind, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_events_entity
    ON ddb_events(entity_id, created_at DESC);
