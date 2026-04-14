-- Slice 28/30 — Phase 9 SurrealDB parity
-- Author: Darshankumar Joshi
--
-- Strict-mode schema definitions and admin SQL passthrough audit log.
--
-- `schema_definitions` is the per-(collection, attribute) strict schema
-- registry consulted by the triple-store write path when
-- `DdbConfig.schema.schema_mode == "strict"`. It complements the richer
-- `_schemas` registry (SurrealDB-style DEFINE TABLE / DEFINE FIELD) and
-- lets operators enforce hard typing with a single flag flip.
--
-- `admin_audit_log` captures every raw SQL statement executed via the
-- `POST /api/sql` admin passthrough so forensic reviews can reconstruct
-- every privileged mutation that bypassed DarshanQL.

CREATE TABLE IF NOT EXISTS schema_definitions (
    collection   TEXT    NOT NULL,
    attribute    TEXT    NOT NULL,
    value_type   TEXT    NOT NULL,
    required     BOOLEAN NOT NULL DEFAULT false,
    unique_index BOOLEAN NOT NULL DEFAULT false,
    default_val  JSONB,
    validator    TEXT,
    PRIMARY KEY (collection, attribute)
);

CREATE INDEX IF NOT EXISTS idx_schema_definitions_collection
    ON schema_definitions (collection);

CREATE TABLE IF NOT EXISTS admin_audit_log (
    id             BIGSERIAL   PRIMARY KEY,
    actor_user_id  UUID        NOT NULL,
    sql            TEXT        NOT NULL,
    params         JSONB       NOT NULL DEFAULT '[]'::jsonb,
    row_count      BIGINT      NOT NULL DEFAULT 0,
    duration_ms    BIGINT      NOT NULL DEFAULT 0,
    error          TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_admin_audit_log_actor
    ON admin_audit_log (actor_user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_admin_audit_log_created
    ON admin_audit_log (created_at DESC);
