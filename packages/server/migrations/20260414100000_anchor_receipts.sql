-- DarshJDB — Phase 5.3 Blockchain Anchor: anchor_receipts table.
--
-- Records a receipt for every batched Merkle-root anchor operation. Each row
-- captures the aggregate batch root computed over the most recent N Merkle
-- transaction roots, the chain-specific anchoring outcome (tx hash / IPFS CID),
-- and the lifecycle status of the anchor attempt.
--
-- Schema keeps all chain-specific fields nullable so a single table serves
-- every anchor backend (none, ipfs, ethereum, solana, …) without migration
-- churn as new backends are added behind feature flags.
--
-- Author: Darshankumar Joshi

CREATE TABLE IF NOT EXISTS anchor_receipts (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    batch_root   TEXT NOT NULL,
    chain        TEXT NOT NULL DEFAULT 'none',
    tx_hash      TEXT,
    ipfs_cid     TEXT,
    anchored_at  TIMESTAMPTZ,
    status       TEXT NOT NULL DEFAULT 'pending',
    tx_count     INTEGER NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_anchor_receipts_created
    ON anchor_receipts (created_at DESC);

CREATE INDEX IF NOT EXISTS idx_anchor_receipts_status
    ON anchor_receipts (status);

CREATE INDEX IF NOT EXISTS idx_anchor_receipts_chain
    ON anchor_receipts (chain);
