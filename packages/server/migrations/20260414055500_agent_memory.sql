-- DarshJDB — migration by Darshankumar Joshi (github.com/darshjme)
-- agent_memory: 4-tier agent memory schema for DarshJDB as an AI-agent backing store.
--
-- Phase 2.1+2.2 of the Grand Transformation (slice 12/30).
--
-- Design notes:
--   * Three tables: agent_sessions, memory_entries, agent_facts.
--   * This file carries the same DDL as
--     packages/server/src/agent_memory/schema.rs::ensure_agent_memory_schema,
--     which runs on every server start. Keep the two in lock-step — a
--     database bootstrapped by either path must end up identical.
--   * pgvector (vector(1536)) powers semantic recall at the episodic/semantic
--     tier via HNSW indices. The embedding column is nullable because raw
--     working-tier writes can skip embeddings for latency, and the whole
--     vector section is optional so a Postgres without the extension still
--     serves the memory REST surface.
--   * `tier` is an enum-via-CHECK with four levels: working, episodic,
--     semantic, archival. Promotion/demotion is driven by application code
--     (packages/agent-memory/src/tiers.rs) so DB stays policy-free.
--   * Fully idempotent. Safe to re-run against an existing database.

-- ── Pre-reconciliation repair ──────────────────────────────────────
-- Databases created by the first revision of this migration keyed
-- agent_sessions on `session_id`; every consumer keys on `id`.

DO $do$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'agent_sessions' AND column_name = 'session_id'
    ) AND NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'agent_sessions' AND column_name = 'id'
    ) THEN
        ALTER TABLE agent_sessions RENAME COLUMN session_id TO id;
    END IF;
END$do$;

-- ── agent_sessions ─────────────────────────────────────────────────
-- One row per live conversation / agent run. The session aggregates a
-- sequence of memory_entries belonging to the same logical context
-- window so working-tier eviction can be scoped per-session.

CREATE TABLE IF NOT EXISTS agent_sessions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL,
    agent_id        TEXT NOT NULL,
    model           TEXT,
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    final_summary   TEXT
);

ALTER TABLE agent_sessions
    ADD COLUMN IF NOT EXISTS model TEXT;
ALTER TABLE agent_sessions
    ALTER COLUMN model DROP NOT NULL;
ALTER TABLE agent_sessions
    ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE agent_sessions
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE agent_sessions
    ADD COLUMN IF NOT EXISTS final_summary TEXT;

CREATE INDEX IF NOT EXISTS idx_agent_sessions_user
    ON agent_sessions (user_id);

CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent
    ON agent_sessions (agent_id, updated_at DESC);

-- ── memory_entries ─────────────────────────────────────────────────
-- The timeline of a session: user/assistant messages, system prompts,
-- tool calls, and automatic summaries. Each entry is tagged with a
-- tier that advances via `tiers.rs::promote_demote`.

CREATE TABLE IF NOT EXISTS memory_entries (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
    agent_id        TEXT,
    tier            TEXT NOT NULL DEFAULT 'working'
                    CHECK (tier IN ('working','episodic','semantic','archival')),
    role            TEXT NOT NULL
                    CHECK (role IN ('user','assistant','system','tool','summary')),
    content         TEXT NOT NULL,
    token_count     INTEGER NOT NULL DEFAULT 0,
    content_tokens  INTEGER NOT NULL DEFAULT 0,
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    importance      DOUBLE PRECISION NOT NULL DEFAULT 0.5,
    summary         TEXT,
    tool_name       TEXT,
    tool_input      JSONB,
    tool_output     JSONB,
    embedded_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    accessed_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    access_count    INTEGER NOT NULL DEFAULT 0,
    compressed      BOOLEAN NOT NULL DEFAULT false
);

-- `agent_id` is written by the tiering/summariser path only; the REST
-- write path scopes by session, so the column stays nullable.
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS agent_id TEXT;
ALTER TABLE memory_entries
    ALTER COLUMN agent_id DROP NOT NULL;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS token_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS content_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS metadata JSONB NOT NULL DEFAULT '{}'::jsonb;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS importance DOUBLE PRECISION NOT NULL DEFAULT 0.5;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS summary TEXT;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS tool_name TEXT;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS tool_input JSONB;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS tool_output JSONB;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS embedded_at TIMESTAMPTZ;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS accessed_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS access_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS compressed BOOLEAN NOT NULL DEFAULT false;

-- Timeline scan inside a session.
CREATE INDEX IF NOT EXISTS idx_memory_session_created
    ON memory_entries (session_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_memory_session_tier
    ON memory_entries (session_id, tier, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_memory_tier
    ON memory_entries (tier);

-- Tier + importance filter for promotion/demotion scans.
CREATE INDEX IF NOT EXISTS idx_memory_agent_tier_importance
    ON memory_entries (agent_id, tier, importance DESC);

-- ── agent_facts ────────────────────────────────────────────────────
-- Key/value knowledge extracted *across* sessions, scoped by
-- (agent_id, user_id, key). The unique index backs the REST upsert's
-- ON CONFLICT clause.

CREATE TABLE IF NOT EXISTS agent_facts (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    agent_id        TEXT NOT NULL,
    user_id         UUID NOT NULL,
    key             TEXT NOT NULL,
    value           TEXT NOT NULL,
    confidence      REAL NOT NULL DEFAULT 1.0,
    source          TEXT NOT NULL DEFAULT 'explicit',
    content_tokens  INTEGER NOT NULL DEFAULT 0,
    embedded_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

DO $do$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'agent_facts'
          AND column_name = 'value'
          AND data_type = 'jsonb'
    ) THEN
        ALTER TABLE agent_facts
            ALTER COLUMN value TYPE TEXT USING value #>> '{}';
    END IF;
END$do$;

ALTER TABLE agent_facts
    ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'explicit';
ALTER TABLE agent_facts
    ADD COLUMN IF NOT EXISTS content_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_facts
    ADD COLUMN IF NOT EXISTS embedded_at TIMESTAMPTZ;
ALTER TABLE agent_facts
    ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE UNIQUE INDEX IF NOT EXISTS uq_agent_facts_agent_user_key
    ON agent_facts (agent_id, user_id, key);

CREATE INDEX IF NOT EXISTS idx_agent_facts_lookup
    ON agent_facts (agent_id, user_id);

-- ── Vector columns (optional) ──────────────────────────────────────
-- Requires pgvector. The server bootstrap runs this section
-- best-effort; applying this file on a Postgres without the extension
-- fails here and leaves the tables above fully usable.

CREATE EXTENSION IF NOT EXISTS vector;

DO $do$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'memory_entries'
          AND column_name = 'embedding'
          AND udt_name <> 'vector'
    ) THEN
        ALTER TABLE memory_entries DROP COLUMN embedding;
    END IF;
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'agent_facts'
          AND column_name = 'embedding'
          AND udt_name <> 'vector'
    ) THEN
        ALTER TABLE agent_facts DROP COLUMN embedding;
    END IF;
END$do$;

ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS embedding vector(1536);
ALTER TABLE agent_facts
    ADD COLUMN IF NOT EXISTS embedding vector(1536);

-- HNSW ANN index on embeddings (only for rows that have one).
-- m=16, ef_construction=64 mirrors pgvector defaults tuned for ~1M rows.
CREATE INDEX IF NOT EXISTS idx_memory_entries_embedding_hnsw
    ON memory_entries USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64)
    WHERE embedding IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_agent_facts_embedding_hnsw
    ON agent_facts USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64)
    WHERE embedding IS NOT NULL;
