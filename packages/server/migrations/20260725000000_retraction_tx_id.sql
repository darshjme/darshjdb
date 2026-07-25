-- Time-versioned retractions.
--
-- `retracted` is a mutable boolean on the ORIGINAL row, so a triple asserted
-- at tx 5 and retracted at tx 100 still reports tx_id = 5 with retracted =
-- true. Point-in-time reads (`get_entity_at`) and snapshot restore therefore
-- reconstructed the attribute as absent at every tx, silently dropping fields.
--
-- `retracted_tx_id` records WHEN the retraction happened. NULL means live.
-- Idempotent -- safe to run multiple times.

ALTER TABLE triples ADD COLUMN IF NOT EXISTS retracted_tx_id BIGINT;

CREATE INDEX IF NOT EXISTS idx_triples_retracted_tx
    ON triples (retracted_tx_id)
    WHERE retracted_tx_id IS NOT NULL;

-- Backfill rows retracted before this column existed. The assertion tx is the
-- best available approximation and is no worse than the previous behaviour.
UPDATE triples
SET retracted_tx_id = tx_id
WHERE retracted AND retracted_tx_id IS NULL;

-- Stamp the retraction tx automatically so every retraction path (including
-- raw SQL and DarshQL) stays time-versioned.
CREATE OR REPLACE FUNCTION darshan_stamp_retraction_tx()
RETURNS trigger AS $fn$
BEGIN
    NEW.retracted_tx_id := nextval('darshan_tx_seq');
    RETURN NEW;
END;
$fn$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_triples_retraction_tx ON triples;
CREATE TRIGGER trg_triples_retraction_tx
    BEFORE UPDATE ON triples
    FOR EACH ROW
    WHEN (NEW.retracted AND NEW.retracted_tx_id IS NULL)
    EXECUTE FUNCTION darshan_stamp_retraction_tx();
