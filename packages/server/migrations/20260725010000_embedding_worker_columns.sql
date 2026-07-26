-- DarshJDB — migration by Darshankumar Joshi (github.com/darshjme)
-- Embedding worker write-back columns for the agent memory schema.
--
-- packages/agent-memory/src/worker.rs writes `embedding`, `content_tokens`
-- and `embedded_at` on every row it embeds. `embedded_at` was never defined
-- by 20260414055500_agent_memory.sql, and `content_tokens` exists only on
-- memory_entries, so every write-back failed and the same batch was
-- re-selected (and re-billed to the provider) on every tick.
--
-- Fully idempotent. Safe to re-run against an existing database.

ALTER TABLE memory_entries
    ADD COLUMN IF NOT EXISTS embedded_at TIMESTAMPTZ;

ALTER TABLE agent_facts
    ADD COLUMN IF NOT EXISTS content_tokens INTEGER NOT NULL DEFAULT 0;

ALTER TABLE agent_facts
    ADD COLUMN IF NOT EXISTS embedded_at TIMESTAMPTZ;
