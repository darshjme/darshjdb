-- DarshJDB — migration by Darshankumar Joshi (github.com/darshjme)
-- agent_memory: 4-tier agent memory schema for DarshJDB as an AI-agent backing store.
--
-- Phase 2.1+2.2 of the Grand Transformation (slice 12/30).
--
-- Design notes:
--   * Three tables: agent_sessions, memory_entries, agent_facts.
--   * pgvector (vector(1536)) powers semantic recall at the episodic/semantic
--     tier via HNSW indices. The embedding column is nullable because raw
--     working-tier writes can skip embeddings for latency.
--   * `tier` is an enum-via-CHECK with four levels: working, episodic,
--     semantic, archival. Promotion/demotion is driven by application code
--     (packages/agent-memory/src/tiers.rs) so DB stays policy-free.
--   * Fully idempotent. Safe to re-run against an existing database.

-- ── Extensions ─────────────────────────────────────────────────────

CREATE EXTENSION IF NOT EXISTS vector;

-- ── agent_sessions ─────────────────────────────────────────────────
-- One row per live conversation / agent run. The session aggregates a
-- sequence of memory_entries belonging to the same logical context
-- window so working-tier eviction can be scoped per-session.

CREATE TABLE IF NOT EXISTS agent_sessions (
    session_id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id             TEXT        NOT NULL,
    user_id              UUID        REFERENCES users(id) ON DELETE SET NULL,
    model                TEXT        NOT NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_active_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    context_window_size  INTEGER     NOT NULL DEFAULT 128000,
    total_tokens_in      BIGINT      NOT NULL DEFAULT 0,
    total_tokens_out     BIGINT      NOT NULL DEFAULT 0,
    metadata             JSONB       NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent_active
    ON agent_sessions (agent_id, last_active_at DESC);

CREATE INDEX IF NOT EXISTS idx_agent_sessions_user
    ON agent_sessions (user_id)
    WHERE user_id IS NOT NULL;

-- ── memory_entries ─────────────────────────────────────────────────
-- The timeline of a session: user/assistant messages, system prompts,
-- tool calls, and automatic summaries. Each entry is tagged with a
-- tier that advances via `tiers.rs::promote_demote`.

CREATE TABLE IF NOT EXISTS memory_entries (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID         NOT NULL REFERENCES agent_sessions(session_id) ON DELETE CASCADE,
    agent_id        TEXT         NOT NULL,
    role            TEXT         NOT NULL CHECK (role IN ('user','assistant','system','tool','summary')),
    content         TEXT         NOT NULL,
    content_tokens  INTEGER      NOT NULL DEFAULT 0,
    embedding       vector(1536),
    importance      DOUBLE PRECISION NOT NULL DEFAULT 0.5,
    tier            TEXT         NOT NULL DEFAULT 'working'
                    CHECK (tier IN ('working','episodic','semantic','archival')),
    summary         TEXT,
    tool_name       TEXT,
    tool_input      JSONB,
    tool_output     JSONB,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    accessed_at     TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    access_count    INTEGER      NOT NULL DEFAULT 0,
    compressed      BOOLEAN      NOT NULL DEFAULT false
);

-- Timeline scan inside a session.
CREATE INDEX IF NOT EXISTS idx_memory_entries_session_time
    ON memory_entries (session_id, created_at DESC);

-- Tier + importance filter for promotion/demotion scans.
CREATE INDEX IF NOT EXISTS idx_memory_entries_agent_tier_importance
    ON memory_entries (agent_id, tier, importance DESC);

-- HNSW ANN index on embeddings (only for rows that have one).
-- m=16, ef_construction=64 mirrors pgvector defaults tuned for ~1M rows.
CREATE INDEX IF NOT EXISTS idx_memory_entries_embedding_hnsw
    ON memory_entries USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64)
    WHERE embedding IS NOT NULL;

-- ── agent_facts ────────────────────────────────────────────────────
-- Key/value knowledge extracted *across* sessions. Scoped by
-- (agent_id, user_id, key); a NULL user_id means a global fact for
-- the agent. The COALESCE in the unique index makes the composite
-- key well-defined even when user_id is NULL.

CREATE TABLE IF NOT EXISTS agent_facts (
    id              UUID         PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        TEXT         NOT NULL,
    user_id         UUID         REFERENCES users(id) ON DELETE CASCADE,
    key             TEXT         NOT NULL,
    value           JSONB        NOT NULL,
    embedding       vector(1536),
    confidence      DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    source_session  UUID         REFERENCES agent_sessions(session_id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_facts_unique
    ON agent_facts (agent_id, COALESCE(user_id::text, 'null'), key);

CREATE INDEX IF NOT EXISTS idx_agent_facts_embedding_hnsw
    ON agent_facts USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64)
    WHERE embedding IS NOT NULL;
