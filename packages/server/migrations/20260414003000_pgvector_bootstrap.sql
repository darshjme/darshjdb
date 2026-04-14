-- DarshJDB Phase 3 — pgvector + full-text search bootstrap.
--
-- Slice 16/30: vector + full-text + hybrid RRF search.
-- Author: Darshankumar Joshi
--
-- This migration is idempotent and safe to run repeatedly. It layers on top
-- of `001_initial.sql` which already creates a legacy `embeddings` table
-- with `BIGSERIAL id` and no composite uniqueness. We keep that table alive
-- and only add the missing bits (unique constraint, ivfflat index, default
-- attribute column, default model upgrade) so existing deployments migrate
-- forward without losing data.
--
-- On a fresh database this file fully provisions the expected schema
-- described in the slice spec:
--   - `embeddings` table with `attribute` default `'default'`,
--     `model` default `'text-embedding-3-small'`,
--     UNIQUE(entity_id, attribute),
--     HNSW + IVFFlat indexes on `embedding`.
--   - `triples` FTS index on `to_tsvector('english', value::text)`.

-- ── pgvector extension ─────────────────────────────────────────────

CREATE EXTENSION IF NOT EXISTS vector;

-- ── Embeddings table ───────────────────────────────────────────────
-- Create on fresh databases. If 001_initial already ran this is a no-op.

CREATE TABLE IF NOT EXISTS embeddings (
    id          BIGSERIAL   PRIMARY KEY,
    entity_id   UUID        NOT NULL,
    attribute   TEXT        NOT NULL DEFAULT 'default',
    embedding   vector(1536) NOT NULL,
    model       TEXT        NOT NULL DEFAULT 'text-embedding-3-small',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Forward-migrate legacy databases: set DEFAULTs for attribute and model
-- columns if they were created without them in 001_initial.sql.
ALTER TABLE embeddings
    ALTER COLUMN attribute SET DEFAULT 'default';

ALTER TABLE embeddings
    ALTER COLUMN model SET DEFAULT 'text-embedding-3-small';

-- Ensure NOT NULL on embedding. Legacy schema allowed NULL — we assume no
-- NULLs exist in practice; if they do the operator should clean up before
-- running this migration.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'embeddings'
          AND column_name = 'embedding'
          AND is_nullable = 'YES'
    ) THEN
        BEGIN
            ALTER TABLE embeddings ALTER COLUMN embedding SET NOT NULL;
        EXCEPTION WHEN others THEN
            -- Leave nullable if existing NULLs block the constraint.
            RAISE NOTICE 'embeddings.embedding left nullable (pre-existing NULL rows)';
        END;
    END IF;
END$$;

-- ── Composite uniqueness (entity_id, attribute) ────────────────────
-- Required so `ON CONFLICT (entity_id, attribute) DO UPDATE` upserts work.
-- Guarded by a DO block so pre-existing duplicates do not crash the migration.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_indexes
        WHERE schemaname = 'public'
          AND indexname  = 'uq_embeddings_entity_attribute'
    ) THEN
        BEGIN
            CREATE UNIQUE INDEX uq_embeddings_entity_attribute
                ON embeddings (entity_id, attribute);
        EXCEPTION WHEN unique_violation THEN
            RAISE NOTICE 'uq_embeddings_entity_attribute skipped: pre-existing duplicates detected';
        END;
    END IF;
END$$;

-- ── Vector similarity indexes ──────────────────────────────────────

-- HNSW for low-latency cosine ANN lookups (Phase 3 default).
CREATE INDEX IF NOT EXISTS idx_embeddings_hnsw
    ON embeddings USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- IVFFlat for L2 distance fallbacks (useful for geometric / non-normalised
-- embedding models). Requires `lists` tuning at scale; 100 is fine for
-- thousands of rows.
CREATE INDEX IF NOT EXISTS idx_embeddings_ivfflat
    ON embeddings USING ivfflat (embedding vector_l2_ops)
    WITH (lists = 100);

-- Lookup index for fetching all embeddings for a given entity.
CREATE INDEX IF NOT EXISTS idx_embeddings_entity
    ON embeddings (entity_id, attribute);

-- ── Full-text index on triples.value ───────────────────────────────
-- 001_initial.sql ships an `idx_triples_fts` over `to_tsvector('english',
-- value #>> '{}')`. That expression only handles scalar string JSONB and
-- silently degrades for objects/arrays. Phase 3 needs an FTS index over the
-- full JSONB cast to text so search_text can match nested fields too. We
-- ship it under a dedicated name so it coexists with the legacy index on
-- already-deployed databases.
CREATE INDEX IF NOT EXISTS idx_triples_fts_text
    ON triples USING gin (to_tsvector('english', value::text))
    WHERE value IS NOT NULL;
