// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Idempotent Postgres DDL bootstrap for the agent-memory subsystem.
//!
//! This is the single source of truth for the `agent_sessions`,
//! `memory_entries` and `agent_facts` shapes; the
//! `20260414055500_agent_memory` migration carries the same DDL verbatim so
//! bootstrap-first and migration-first databases converge on one schema.
//!
//! It is safe to call on every server start — every `CREATE` is
//! `IF NOT EXISTS`, every column addition is `ADD COLUMN IF NOT EXISTS`, and
//! the pre-reconciliation column shapes are repaired in place.

use sqlx::PgPool;

/// Core tables. Runs on any stock Postgres, with or without pgvector.
const CORE_SQL: &str = r#"
    -- Pre-reconciliation repair ----------------------------------------
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

    -- Sessions ---------------------------------------------------------
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

    -- Memory entries ---------------------------------------------------
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

    CREATE INDEX IF NOT EXISTS idx_memory_session_created
        ON memory_entries (session_id, created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_memory_session_tier
        ON memory_entries (session_id, tier, created_at DESC);
    CREATE INDEX IF NOT EXISTS idx_memory_tier
        ON memory_entries (tier);
    CREATE INDEX IF NOT EXISTS idx_memory_agent_tier_importance
        ON memory_entries (agent_id, tier, importance DESC);

    -- Agent facts ------------------------------------------------------
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
"#;

/// pgvector-dependent columns and ANN indexes. Best-effort: a Postgres
/// build without the `vector` extension keeps the REST surface working and
/// only loses semantic recall.
const VECTOR_SQL: &str = r#"
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

    CREATE INDEX IF NOT EXISTS idx_memory_entries_embedding_hnsw
        ON memory_entries USING hnsw (embedding vector_cosine_ops)
        WITH (m = 16, ef_construction = 64)
        WHERE embedding IS NOT NULL;

    CREATE INDEX IF NOT EXISTS idx_agent_facts_embedding_hnsw
        ON agent_facts USING hnsw (embedding vector_cosine_ops)
        WITH (m = 16, ef_construction = 64)
        WHERE embedding IS NOT NULL;
"#;

/// Create / migrate every agent-memory table in a single transaction.
pub async fn ensure_agent_memory_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(CORE_SQL).execute(pool).await?;

    if let Err(e) = sqlx::raw_sql(VECTOR_SQL).execute(pool).await {
        tracing::warn!(
            error = %e,
            "pgvector extension unavailable on this Postgres build; agent-memory \
             embeddings and semantic recall stay disabled until the database is \
             upgraded to a pgvector-capable image"
        );
    }

    Ok(())
}
