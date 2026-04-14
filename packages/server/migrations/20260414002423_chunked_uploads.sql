-- DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
-- Migration: chunked upload state table for VYASA Phase 7.1.
--
-- Tracks in-flight multi-part uploads so clients can resume after a dropped
-- connection, and the server can reassemble chunks in order, deduplicate
-- retries, and garbage-collect abandoned uploads.
--
-- Notes
-- =====
-- * `received_chunks` is a sparse array — order does not matter for presence
--   checks, the bitmap is ANY()'d on every chunk PUT to reject duplicates.
-- * `status` is free-form text (`in_progress` | `completed` | `aborted`) so
--   we never add an enum migration downstream. The cleanup task only ever
--   cares about `in_progress` rows older than 24h.
-- * `gen_random_uuid()` requires pgcrypto. It is already enabled by the
--   bootstrap schema (001_initial.sql) via `CREATE EXTENSION IF NOT EXISTS
--   pgcrypto` — do not duplicate it here.

CREATE TABLE IF NOT EXISTS chunked_uploads (
    upload_id        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    entity_id        UUID,
    path             TEXT        NOT NULL,
    total_chunks     INTEGER     NOT NULL,
    received_chunks  INTEGER[]   NOT NULL DEFAULT '{}',
    content_type     TEXT        NOT NULL,
    file_size        BIGINT,
    status           TEXT        NOT NULL DEFAULT 'in_progress',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at     TIMESTAMPTZ,
    CONSTRAINT chunked_uploads_total_chunks_positive CHECK (total_chunks > 0)
);

-- Hot path: the cleanup task scans for stale in-progress rows by (status, created_at).
CREATE INDEX IF NOT EXISTS idx_chunked_uploads_status
    ON chunked_uploads (status, created_at);
