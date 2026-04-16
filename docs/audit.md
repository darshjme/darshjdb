# Merkle Audit Trail

DarshJDB uses a Bitcoin-inspired Merkle tree audit trail to provide cryptographic tamper detection for every write transaction. Every mutation produces a SHA-512 Merkle root over its triples, and roots are chained so that each transaction incorporates its predecessor -- forming an unbroken hash chain analogous to Bitcoin's blockchain.

## How It Works

### Triple Hashing

Each triple is hashed individually using SHA-512. The pre-image is the concatenation of the triple's identity fields:

```
SHA-512(entity_id || attribute || value_json || value_type_le_bytes || tx_id_le_bytes)
```

This produces a 64-byte digest that is deterministic and unique per triple.

### Merkle Root Computation

Given a set of triples in a transaction, DarshJDB builds a balanced binary hash tree bottom-up:

```
Triple_0  Triple_1  Triple_2  Triple_3
   |          |          |          |
SHA-512   SHA-512   SHA-512   SHA-512
   |          |          |          |
   H0         H1         H2         H3
     \       /             \       /
     H(H0||H1)            H(H2||H3)
          \                  /
           \                /
        Merkle Root = H(left || right)
```

If the number of leaves is odd, the last leaf is duplicated (Bitcoin convention). For a single triple, the root equals the triple's own hash. For zero triples, the root is 64 zero bytes (the null root).

### Chain Linking

Each transaction's Merkle root is chained with its predecessor:

```
chained_root = SHA-512(merkle_root || prev_chained_root)
```

The first transaction uses an all-zeros `prev_root` (genesis). This chain links every transaction cryptographically -- modifying any historical triple invalidates the Merkle root, which breaks the chain at that point and every subsequent link.

## Database Schema

```sql
CREATE TABLE tx_merkle_roots (
    tx_id        BIGINT PRIMARY KEY,
    merkle_root  BYTEA NOT NULL,        -- SHA-512 root of this tx's triples
    chained_root BYTEA NOT NULL,        -- SHA-512(merkle_root || prev_root)
    prev_root    BYTEA,                 -- Previous tx's chained_root
    triple_count INTEGER NOT NULL,      -- Number of triples in this tx
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
```

Indexed on `created_at` for time-range queries.

The table is created during triple-store schema bootstrap via `ensure_audit_schema()`.

## Recording Roots

Merkle roots are computed from in-memory data during the transaction commit path, avoiding a redundant Postgres round-trip:

```rust
use darshjdb::audit::{merkle_root_from_inputs, record_merkle_root};

let root = merkle_root_from_inputs(&triple_inputs, tx_id);
let chained = record_merkle_root(&pool, tx_id, &root, triple_count).await?;
```

`record_merkle_root` fetches the most recent `chained_root` from the table, computes the new chain link, and inserts the record. If no previous entry exists, the null root is used.

## Verification

### Single Transaction Verification

Recomputes the Merkle root from the stored triples and compares against the recorded root.

```rust
let result = verify_tx(&pool, tx_id).await?;
// TxVerification { tx_id, valid, detail, stored_root, computed_root, triple_count }
```

If the stored triples have been tampered with, `valid` will be `false` and `detail` will read `"TAMPER DETECTED: computed root does not match stored root"`.

### Full Chain Verification

Walks the entire `tx_merkle_roots` table and verifies that each transaction's `prev_root` matches the previous transaction's `chained_root`, and that each `chained_root` can be recomputed from the stored `merkle_root` and `prev_root`.

```rust
let result = verify_chain(&pool).await?;
// ChainVerification { valid, total_transactions, first_broken_tx, detail }
```

If the chain is broken, `first_broken_tx` identifies exactly where the tampering occurred.

### Entity Proofs

Generate Merkle inclusion proofs for all triples belonging to a specific entity. Each proof consists of sibling hashes at each level of the tree, allowing independent verification without re-hashing all triples.

```rust
let proofs = entity_proof(&pool, entity_id).await?;
// Vec<EntityMerkleProof> -- one per transaction containing this entity
```

Each `EntityMerkleProof` contains:
- `tx_id` -- the transaction
- `entity_id` -- the entity
- `proofs` -- a `Vec<MerkleProof>`, one per triple

A `MerkleProof` contains:
- `leaf_hash` -- the SHA-512 hash of the target triple (hex)
- `proof_path` -- the sibling hashes from leaf to root, each with a `Left` or `Right` position marker
- `root` -- the expected Merkle root (hex)

### Proof Verification

```rust
use darshjdb::audit::verify_proof;

let leaf = hash_triple(&triple);
let is_valid = verify_proof(&leaf, &proof.proof_path, &expected_root);
```

The verifier walks the proof path from leaf to root, hashing with each sibling in the correct order (left or right). If the final computed hash matches the expected root, the triple is proven to be part of the transaction.

## API Endpoints

Three admin endpoints expose the audit trail over HTTP:

### GET /api/admin/audit/verify/:tx_id

Verify a single transaction's Merkle root.

```bash
curl http://localhost:7700/api/admin/audit/verify/42
```

Response (200 OK if valid, 409 Conflict if tampered):

```json
{
  "tx_id": 42,
  "valid": true,
  "detail": "Merkle root matches stored triples",
  "stored_root": "a1b2c3...",
  "computed_root": "a1b2c3...",
  "triple_count": 5
}
```

Tamper response:

```json
{
  "tx_id": 42,
  "valid": false,
  "detail": "TAMPER DETECTED: computed root does not match stored root",
  "stored_root": "a1b2c3...",
  "computed_root": "d4e5f6...",
  "triple_count": 5
}
```

### GET /api/admin/audit/chain

Verify the entire hash chain is unbroken.

```bash
curl http://localhost:7700/api/admin/audit/chain
```

Response:

```json
{
  "valid": true,
  "total_transactions": 1847,
  "first_broken_tx": null,
  "detail": "Hash chain intact across 1847 transactions"
}
```

Broken chain:

```json
{
  "valid": false,
  "total_transactions": 1847,
  "first_broken_tx": 923,
  "detail": "Chain broken at tx 923: stored prev_root does not match previous chained_root"
}
```

### GET /api/admin/audit/proof/:entity_id

Get Merkle inclusion proofs for all triples belonging to an entity.

```bash
curl http://localhost:7700/api/admin/audit/proof/550e8400-e29b-41d4-a716-446655440000
```

Response:

```json
{
  "proofs": [
    {
      "tx_id": 42,
      "entity_id": "550e8400-e29b-41d4-a716-446655440000",
      "proofs": [
        {
          "leaf_hash": "a1b2c3...",
          "proof_path": [
            { "hash": "d4e5f6...", "position": "right" },
            { "hash": "789abc...", "position": "left" }
          ],
          "root": "final_root_hex..."
        }
      ]
    }
  ]
}
```

Returns 404 if no triples exist for the entity.

## Security Properties

**Tamper detection**: modifying any historical triple changes its hash, which changes the Merkle root, which breaks the chain at that transaction and every subsequent link. The break is precisely locatable.

**Lightweight verification**: proving a single entity's inclusion requires O(log n) hashes (the proof path) rather than re-hashing all triples in the transaction.

**Audit compliance**: the unbroken hash chain serves as a cryptographic audit log. Each entry records the exact triple count and timestamp, with the chain itself being self-certifying.

**Determinism**: SHA-512 hashing uses canonical JSON serialization and little-endian byte encoding for numeric fields, ensuring identical triples always produce identical hashes regardless of platform or serialization order.
