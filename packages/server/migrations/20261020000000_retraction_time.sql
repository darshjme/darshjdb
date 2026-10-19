-- Preserve the time of future retractions. Historical timestamps cannot be
-- recovered; NULL remains explicit for rows predating this migration.
ALTER TABLE triples ADD COLUMN IF NOT EXISTS retracted_at TIMESTAMPTZ;
CREATE OR REPLACE FUNCTION darshan_stamp_retraction_time()
RETURNS trigger AS $fn$
BEGIN
    NEW.retracted_at := clock_timestamp();
    RETURN NEW;
END;
$fn$ LANGUAGE plpgsql;
DROP TRIGGER IF EXISTS trg_triples_retraction_time ON triples;
CREATE TRIGGER trg_triples_retraction_time
    BEFORE UPDATE ON triples
    FOR EACH ROW
    WHEN (NEW.retracted AND NOT OLD.retracted)
    EXECUTE FUNCTION darshan_stamp_retraction_time();
