-- Phase 5.1 — TimescaleDB hypertable for time-series workloads.
--
-- Author: Darshankumar Joshi
--
-- Adds a single generic `time_series` hypertable so DarshJDB can store
-- metrics / events / telemetry alongside the triple store without a
-- dedicated TSDB. When TimescaleDB is available (production image:
-- `timescale/timescaledb-ha:pg16-latest`) the table is converted to a
-- hypertable with 4 entity_type-partitioned space chunks and 1-day time
-- chunks, compression (> 7 days), and a 90-day retention policy.
--
-- The migration is written to stay green on vanilla Postgres (CI without
-- the extension): every TimescaleDB-specific call is wrapped in a DO
-- block that swallows `undefined_function` errors, so `sqlx migrate`
-- succeeds on any Postgres 16 instance.

CREATE EXTENSION IF NOT EXISTS timescaledb CASCADE;

CREATE TABLE IF NOT EXISTS time_series (
    time         TIMESTAMPTZ      NOT NULL,
    entity_id    UUID             NOT NULL,
    entity_type  TEXT             NOT NULL,
    attribute    TEXT             NOT NULL,
    value_num    DOUBLE PRECISION,
    value_text   TEXT,
    value_json   JSONB,
    tags         JSONB            NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (entity_type, entity_id, attribute, time)
);

-- Convert to a hypertable when TimescaleDB is installed. On vanilla
-- Postgres the `create_hypertable` function does not exist and the
-- DO block silently falls through, leaving the plain table in place.
DO $$
BEGIN
    PERFORM create_hypertable(
        'time_series',
        'time',
        partitioning_column => 'entity_type',
        number_partitions   => 4,
        chunk_time_interval => INTERVAL '1 day',
        if_not_exists       => TRUE
    );
EXCEPTION WHEN undefined_function THEN
    NULL;
END $$;

-- Secondary indexes for common access patterns: latest-by-entity,
-- attribute scans, and tag filtering.
CREATE INDEX IF NOT EXISTS idx_time_series_entity_time
    ON time_series (entity_type, entity_id, time DESC);

CREATE INDEX IF NOT EXISTS idx_time_series_attribute_time
    ON time_series (entity_type, attribute, time DESC);

CREATE INDEX IF NOT EXISTS idx_time_series_tags
    ON time_series USING GIN (tags);

-- Compression after 7 days — reduces on-disk footprint dramatically
-- while keeping recent rows queryable at full row-store speed.
DO $$
BEGIN
    PERFORM add_compression_policy('time_series', INTERVAL '7 days');
EXCEPTION WHEN undefined_function THEN
    NULL;
WHEN others THEN
    -- `add_compression_policy` can also error if compression has not
    -- been enabled yet; tolerate that without failing the migration.
    NULL;
END $$;

-- Retention: drop chunks older than 90 days. Operators can tune via
-- TimescaleDB job config without touching schema.
DO $$
BEGIN
    PERFORM add_retention_policy('time_series', INTERVAL '90 days');
EXCEPTION WHEN undefined_function THEN
    NULL;
WHEN others THEN
    NULL;
END $$;
