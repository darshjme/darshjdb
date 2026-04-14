// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Idempotent Postgres DDL bootstrap for the agent-memory subsystem.
//!
//! This is the slice 12 schema scaffold. It is safe to call on every
//! server start — every `CREATE` is `IF NOT EXISTS` and every column
//! addition is guarded by an `information_schema` check.

use sqlx::PgPool;

/// Create / migrate every agent-memory table in a single transaction.
pub async fn ensure_agent_memory_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        -- Sessions ---------------------------------------------------------
        CREATE TABLE IF NOT EXISTS agent_sessions (
            id              UUID PRIMARY KEY,
            user_id         UUID NOT NULL,
            agent_id        TEXT NOT NULL,
            model           TEXT,
            metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            final_summary   TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_user
            ON agent_sessions (user_id);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent
            ON agent_sessions (agent_id);

        -- Memory entries ---------------------------------------------------
        CREATE TABLE IF NOT EXISTS memory_entries (
            id              UUID PRIMARY KEY,
            session_id      UUID NOT NULL REFERENCES agent_sessions(id) ON DELETE CASCADE,
            tier            TEXT NOT NULL,
            role            TEXT NOT NULL,
            content         TEXT NOT NULL,
            token_count     INTEGER NOT NULL DEFAULT 0,
            metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
            embedding       JSONB,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
        );
        CREATE INDEX IF NOT EXISTS idx_memory_session_created
            ON memory_entries (session_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_memory_session_tier
            ON memory_entries (session_id, tier, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_memory_tier
            ON memory_entries (tier);

        -- Agent facts ------------------------------------------------------
        CREATE TABLE IF NOT EXISTS agent_facts (
            id              UUID PRIMARY KEY,
            agent_id        TEXT NOT NULL,
            user_id         UUID NOT NULL,
            key             TEXT NOT NULL,
            value           TEXT NOT NULL,
            confidence      REAL NOT NULL DEFAULT 1.0,
            source          TEXT NOT NULL DEFAULT 'explicit',
            updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            UNIQUE (agent_id, user_id, key)
        );
        CREATE INDEX IF NOT EXISTS idx_agent_facts_lookup
            ON agent_facts (agent_id, user_id);
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
