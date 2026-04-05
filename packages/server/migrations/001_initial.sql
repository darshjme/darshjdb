-- DarshanDB: Initial triple store schema
-- Idempotent -- safe to run multiple times.

-- ── pgvector extension ────────────────────────────────────────────

CREATE EXTENSION IF NOT EXISTS vector;

-- ── Core table ─────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS triples (
    id          BIGSERIAL   PRIMARY KEY,
    entity_id   UUID        NOT NULL,
    attribute   TEXT        NOT NULL,
    value       JSONB       NOT NULL,
    value_type  SMALLINT    NOT NULL DEFAULT 0,
    tx_id       BIGINT      NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    retracted   BOOLEAN     NOT NULL DEFAULT false
);

-- ── Indexes ────────────────────────────────────────────────────────

-- Composite index for entity lookups filtered by attribute.
CREATE INDEX IF NOT EXISTS idx_triples_entity_attr
    ON triples (entity_id, attribute)
    WHERE NOT retracted;

-- GIN index for value-based queries (contains, equality on JSONB).
CREATE INDEX IF NOT EXISTS idx_triples_attr_value
    ON triples USING gin (attribute, value)
    WHERE NOT retracted;

-- Transaction ordering.
CREATE INDEX IF NOT EXISTS idx_triples_tx_id
    ON triples (tx_id);

-- Covering index for point-in-time reads.
CREATE INDEX IF NOT EXISTS idx_triples_entity_tx
    ON triples (entity_id, tx_id);

-- Attribute scan for schema inference.
CREATE INDEX IF NOT EXISTS idx_triples_attribute
    ON triples (attribute)
    WHERE NOT retracted;

-- Full-text search GIN index on the text representation of JSONB values.
CREATE INDEX IF NOT EXISTS idx_triples_fts
    ON triples USING gin (to_tsvector('english', value #>> '{}'))
    WHERE NOT retracted;

-- ── Transaction sequence ───────────────────────────────────────────

CREATE SEQUENCE IF NOT EXISTS darshan_tx_seq
    START WITH 1 INCREMENT BY 1;

-- ── Entity Pool ───────────────────────────────────────────────────
-- Maps external UUIDs to compact internal integer IDs.
-- All index lookups become integer comparisons instead of 16-byte
-- UUID + text comparisons — the single biggest performance win
-- from Ontotext GraphDB's dictionary encoding.

CREATE TABLE IF NOT EXISTS entity_pool (
    internal_id BIGSERIAL PRIMARY KEY,
    external_id UUID NOT NULL UNIQUE
);
CREATE INDEX IF NOT EXISTS idx_entity_pool_external
    ON entity_pool (external_id);

-- ── Attribute Pool ────────────────────────────────────────────────
-- Maps attribute name strings to compact integer IDs.

CREATE TABLE IF NOT EXISTS attribute_pool (
    internal_id SERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
CREATE INDEX IF NOT EXISTS idx_attribute_pool_name
    ON attribute_pool (name);

-- ── Embeddings (pgvector) ─────────────────────────────────────────
-- Stores vector embeddings linked to entity triples for semantic search.
-- The dimension (1536) matches OpenAI text-embedding-ada-002 but is
-- configurable at insert time — pgvector accepts any vector length
-- and the HNSW index handles mixed sizes gracefully.

CREATE TABLE IF NOT EXISTS embeddings (
    id          BIGSERIAL       PRIMARY KEY,
    entity_id   UUID            NOT NULL,
    attribute   TEXT            NOT NULL,
    embedding   vector(1536),
    model       TEXT            NOT NULL DEFAULT 'text-embedding-ada-002',
    created_at  TIMESTAMPTZ     NOT NULL DEFAULT now()
);

-- HNSW index for fast approximate nearest-neighbour search using
-- cosine distance. m=16 gives good recall/speed trade-off; adjust
-- ef_construction for higher recall at the cost of build time.
CREATE INDEX IF NOT EXISTS idx_embeddings_hnsw
    ON embeddings USING hnsw (embedding vector_cosine_ops)
    WITH (m = 16, ef_construction = 64);

-- Lookup index for fetching all embeddings for a given entity.
CREATE INDEX IF NOT EXISTS idx_embeddings_entity
    ON embeddings (entity_id, attribute);
