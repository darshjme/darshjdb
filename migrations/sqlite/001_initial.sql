-- DarshJDB — SQLite triple store schema (v0.3.2)
--
-- This is the SQLite translation of the Postgres `triples` table
-- defined in `packages/server/src/triple_store/mod.rs::ensure_schema`.
-- It is NOT applied by sqlx-migrate — the SqliteStore applies the
-- statements itself via `rusqlite::Connection::execute_batch` during
-- `SqliteStore::open`. This file is the canonical reference and is
-- what external tools (CI, docs generators) should read from.
--
-- Translation rules:
--   JSONB          -> TEXT with json1 extension (json_extract, json_valid)
--   UUID           -> TEXT canonical string form
--   BIGSERIAL      -> INTEGER PRIMARY KEY AUTOINCREMENT
--   SMALLINT       -> INTEGER (SQLite has no SMALLINT; storage class INTEGER)
--   BOOLEAN        -> INTEGER 0/1 (SQLite has no native bool)
--   TIMESTAMPTZ    -> TEXT in RFC3339 form (stored via chrono::DateTime::to_rfc3339)
--   partial index  -> WHERE clause on CREATE INDEX (SQLite supports this since 3.8)
--   SEQUENCE       -> `darshan_tx_seq` single-row table with a CAS-style UPDATE
--
-- Full-text search and vector search are v0.4 work — see the TODO at
-- the bottom of this file.

PRAGMA foreign_keys = ON;

-- Triple storage. Column order and names mirror the Postgres schema so
-- serde_json round-tripping stays trivial.
CREATE TABLE IF NOT EXISTS triples (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_id   TEXT    NOT NULL,
    attribute   TEXT    NOT NULL,
    value       TEXT    NOT NULL,
    value_type  INTEGER NOT NULL DEFAULT 0,
    tx_id       INTEGER NOT NULL,
    created_at  TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    retracted   INTEGER NOT NULL DEFAULT 0 CHECK (retracted IN (0, 1)),
    expires_at  TEXT,
    CHECK (json_valid(value)),
    CHECK (value_type BETWEEN 0 AND 6),
    CHECK (length(attribute) BETWEEN 1 AND 512)
);

-- Composite index for entity lookups filtered by attribute. SQLite
-- supports partial indexes, so we can match the Postgres predicate.
CREATE INDEX IF NOT EXISTS idx_triples_entity_attr
    ON triples (entity_id, attribute)
    WHERE retracted = 0;

-- Transaction ordering index.
CREATE INDEX IF NOT EXISTS idx_triples_tx_id
    ON triples (tx_id);

-- Covering index for point-in-time reads.
CREATE INDEX IF NOT EXISTS idx_triples_entity_tx
    ON triples (entity_id, tx_id);

-- Attribute scan for schema inference.
CREATE INDEX IF NOT EXISTS idx_triples_attribute
    ON triples (attribute)
    WHERE retracted = 0;

-- TTL expiry scan index.
CREATE INDEX IF NOT EXISTS idx_triples_expires
    ON triples (expires_at)
    WHERE expires_at IS NOT NULL AND retracted = 0;

-- Transaction id sequence. Postgres uses `darshan_tx_seq`; SQLite has no
-- native sequences, so we emulate with a single-row counter table and
-- bump it via `UPDATE ... SET next = next + 1 RETURNING next` inside a
-- transaction. SQLite 3.35+ supports RETURNING.
CREATE TABLE IF NOT EXISTS darshan_tx_seq (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    next_value  INTEGER NOT NULL
);

INSERT OR IGNORE INTO darshan_tx_seq (id, next_value) VALUES (1, 1);

-- TODO(v0.4): FTS5 virtual table for /search endpoints:
--   CREATE VIRTUAL TABLE triples_fts USING fts5(
--       attribute, value_text,
--       content='triples', content_rowid='id', tokenize='porter unicode61'
--   );
-- Needs triggers on INSERT/UPDATE/DELETE to keep the shadow table in
-- sync. Deferred to v0.4 together with the sqlite-vec extension load
-- for vector search.
