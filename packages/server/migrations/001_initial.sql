-- DarshanDB: Initial triple store schema
-- Idempotent -- safe to run multiple times.

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

-- ── Transaction sequence ───────────────────────────────────────────

CREATE SEQUENCE IF NOT EXISTS darshan_tx_seq
    START WITH 1 INCREMENT BY 1;
