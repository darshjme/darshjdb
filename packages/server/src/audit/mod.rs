//! Bitcoin-inspired Merkle tree audit trail for DarshanDB.
//!
//! Every mutation transaction produces a Merkle root hash over the set
//! of triples it writes. Roots are chained: each new root incorporates
//! the previous root, forming a hash chain analogous to Bitcoin's block
//! chain. This provides:
//!
//! - **Tamper detection:** any modification to historical triples
//!   invalidates the Merkle root and breaks the chain.
//! - **Lightweight verification:** a single entity's inclusion can be
//!   proven with an O(log n) Merkle proof instead of re-hashing all
//!   triples.
//! - **Audit compliance:** the unbroken hash chain serves as a
//!   cryptographic audit log of every write.
//!
//! # Architecture
//!
//! ```text
//! Triple_0  Triple_1  Triple_2  Triple_3
//!    |          |          |          |
//! SHA-512   SHA-512   SHA-512   SHA-512
//!    |          |          |          |
//!    H0         H1         H2         H3
//!      \       /             \       /
//!      H(H0||H1)            H(H2||H3)
//!           \                  /
//!            \                /
//!         Merkle Root = H(left || right)
//! ```
//!
//! The Merkle root is then chained with the previous transaction's root:
//!
//! ```text
//! tx_merkle_root = SHA-512(merkle_root || prev_root)
//! ```

pub mod handlers;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use uuid::Uuid;

use crate::triple_store::{Triple, TripleInput};

// ── Merkle proof types ─────────────────────────────────────────────

/// Position of a sibling node in a Merkle proof path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProofPosition {
    /// The sibling hash is on the left; the target hash is on the right.
    Left,
    /// The sibling hash is on the right; the target hash is on the left.
    Right,
}

/// A single node in a Merkle inclusion proof.
///
/// Walking from the leaf to the root, each node provides the sibling
/// hash and its position so the verifier can reconstruct the path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProofNode {
    /// The sibling hash at this tree level (hex-encoded for JSON safety).
    pub hash: String,
    /// Whether the sibling sits to the left or right.
    pub position: ProofPosition,
}

/// Complete Merkle inclusion proof for one or more triples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    /// The leaf hash of the target triple.
    pub leaf_hash: String,
    /// Path from leaf to root.
    pub proof_path: Vec<MerkleProofNode>,
    /// The expected Merkle root (hex).
    pub root: String,
}

// ── Core hashing ───────────────────────────────────────────────────

/// Hash a single triple into a 64-byte SHA-512 digest.
///
/// The pre-image is the concatenation of the triple's identity fields:
/// `entity_id || attribute || value_json || value_type_le_bytes`.
/// This is deterministic and order-independent per-triple.
pub fn hash_triple(triple: &Triple) -> [u8; 64] {
    let mut hasher = Sha512::new();
    hasher.update(triple.entity_id.as_bytes());
    hasher.update(triple.attribute.as_bytes());
    // Canonical JSON serialization of the value.
    hasher.update(triple.value.to_string().as_bytes());
    hasher.update(triple.value_type.to_le_bytes());
    hasher.update(triple.tx_id.to_le_bytes());
    let result = hasher.finalize();
    let mut out = [0u8; 64];
    out.copy_from_slice(&result);
    out
}

/// Compute the Merkle root hash from a set of triples.
///
/// If the set is empty, returns all zeros (the null root).
/// For a single triple, the root is the triple's own hash.
/// Otherwise, builds a balanced binary hash tree bottom-up.
pub fn merkle_root(triples: &[Triple]) -> [u8; 64] {
    if triples.is_empty() {
        return [0u8; 64];
    }

    let mut hashes: Vec<[u8; 64]> = triples.iter().map(hash_triple).collect();

    // Build the tree bottom-up.
    while hashes.len() > 1 {
        // If odd number of leaves, duplicate the last (Bitcoin convention).
        if !hashes.len().is_multiple_of(2) {
            let last = *hashes.last().unwrap();
            hashes.push(last);
        }

        let mut next_level = Vec::with_capacity(hashes.len() / 2);
        for chunk in hashes.chunks(2) {
            next_level.push(hash_pair(&chunk[0], &chunk[1]));
        }
        hashes = next_level;
    }

    hashes[0]
}

/// Hash a single triple input (pre-write) using the same scheme as
/// [`hash_triple`]. Since `hash_triple` only consumes `entity_id`,
/// `attribute`, `value`, `value_type`, and `tx_id`, we can produce an
/// identical digest without the database-assigned `id` or `created_at`.
pub fn hash_triple_input(input: &TripleInput, tx_id: i64) -> [u8; 64] {
    let mut hasher = Sha512::new();
    hasher.update(input.entity_id.as_bytes());
    hasher.update(input.attribute.as_bytes());
    hasher.update(input.value.to_string().as_bytes());
    hasher.update(input.value_type.to_le_bytes());
    hasher.update(tx_id.to_le_bytes());
    let result = hasher.finalize();
    let mut out = [0u8; 64];
    out.copy_from_slice(&result);
    out
}

/// Compute the Merkle root from in-memory [`TripleInput`]s and a known
/// `tx_id`, avoiding a round-trip to Postgres. Produces the same root
/// as [`merkle_root`] would for the equivalent committed [`Triple`]s.
pub fn merkle_root_from_inputs(inputs: &[TripleInput], tx_id: i64) -> [u8; 64] {
    if inputs.is_empty() {
        return [0u8; 64];
    }

    let mut hashes: Vec<[u8; 64]> = inputs.iter().map(|t| hash_triple_input(t, tx_id)).collect();

    while hashes.len() > 1 {
        if !hashes.len().is_multiple_of(2) {
            let last = *hashes.last().unwrap();
            hashes.push(last);
        }

        let mut next_level = Vec::with_capacity(hashes.len() / 2);
        for chunk in hashes.chunks(2) {
            next_level.push(hash_pair(&chunk[0], &chunk[1]));
        }
        hashes = next_level;
    }

    hashes[0]
}

/// Compute the chained root: SHA-512(merkle_root || prev_root).
///
/// This links each transaction's Merkle root to its predecessor,
/// forming the hash chain. If there is no predecessor (first tx),
/// `prev_root` should be all zeros.
pub fn chained_root(merkle_root: &[u8; 64], prev_root: &[u8; 64]) -> [u8; 64] {
    hash_pair(merkle_root, prev_root)
}

/// Hash two 64-byte digests together: SHA-512(left || right).
fn hash_pair(left: &[u8; 64], right: &[u8; 64]) -> [u8; 64] {
    let mut hasher = Sha512::new();
    hasher.update(left);
    hasher.update(right);
    let result = hasher.finalize();
    let mut out = [0u8; 64];
    out.copy_from_slice(&result);
    out
}

// ── Merkle proof generation ────────────────────────────────────────

/// Generate a Merkle inclusion proof for a specific triple within a set.
///
/// Returns `None` if the triple is not found in the set.
/// The proof consists of sibling hashes at each level of the tree,
/// allowing a verifier to reconstruct the root from the leaf.
pub fn generate_proof(target: &Triple, all_triples: &[Triple]) -> Option<MerkleProof> {
    if all_triples.is_empty() {
        return None;
    }

    let target_hash = hash_triple(target);
    let mut hashes: Vec<[u8; 64]> = all_triples.iter().map(hash_triple).collect();

    // Find the target leaf index.
    let mut index = hashes.iter().position(|h| h == &target_hash)?;

    let root = merkle_root(all_triples);
    let mut proof_path = Vec::new();

    // Walk up the tree, collecting sibling hashes.
    while hashes.len() > 1 {
        if !hashes.len().is_multiple_of(2) {
            let last = *hashes.last().unwrap();
            hashes.push(last);
        }

        let sibling_index = if index % 2 == 0 { index + 1 } else { index - 1 };
        let position = if index % 2 == 0 {
            ProofPosition::Right
        } else {
            ProofPosition::Left
        };

        proof_path.push(MerkleProofNode {
            hash: hex::encode(hashes[sibling_index]),
            position,
        });

        // Move up to the parent level.
        let mut next_level = Vec::with_capacity(hashes.len() / 2);
        for chunk in hashes.chunks(2) {
            next_level.push(hash_pair(&chunk[0], &chunk[1]));
        }
        hashes = next_level;
        index /= 2;
    }

    Some(MerkleProof {
        leaf_hash: hex::encode(target_hash),
        proof_path,
        root: hex::encode(root),
    })
}

/// Verify a Merkle inclusion proof.
///
/// Given a leaf hash, a proof path, and the expected root, reconstruct
/// the root by walking the path and check equality.
pub fn verify_proof(
    leaf_hash: &[u8; 64],
    proof: &[MerkleProofNode],
    expected_root: &[u8; 64],
) -> bool {
    let mut current = *leaf_hash;

    for node in proof {
        let sibling = match hex_decode_64(&node.hash) {
            Some(h) => h,
            None => return false,
        };

        current = match node.position {
            ProofPosition::Left => hash_pair(&sibling, &current),
            ProofPosition::Right => hash_pair(&current, &sibling),
        };
    }

    current == *expected_root
}

// ── Hex helpers ────────────────────────────────────────────────────

/// Decode a hex string into a 64-byte array.
fn hex_decode_64(s: &str) -> Option<[u8; 64]> {
    let bytes = hex::decode(s).ok()?;
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// Encode a 64-byte array as lowercase hex.
pub fn hex_encode(data: &[u8; 64]) -> String {
    hex::encode(data)
}

// ── SQL: tx_merkle_roots table ─────────────────────────────────────

/// Create the `tx_merkle_roots` table if it does not exist.
///
/// Called during triple-store schema bootstrap so the audit trail
/// is available from the first transaction.
pub async fn ensure_audit_schema(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS tx_merkle_roots (
            tx_id        BIGINT PRIMARY KEY,
            merkle_root  BYTEA NOT NULL,
            chained_root BYTEA NOT NULL,
            prev_root    BYTEA,
            triple_count INTEGER NOT NULL,
            created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        CREATE INDEX IF NOT EXISTS idx_merkle_roots_created
            ON tx_merkle_roots (created_at);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a pre-computed Merkle root for a completed transaction.
///
/// The caller computes the Merkle root from in-memory data and passes it
/// here, avoiding a redundant round-trip to Postgres. `prev_root` is
/// fetched from the most recent entry in the chain. If no previous entry
/// exists, the null root (all zeros) is used.
pub async fn record_merkle_root(
    pool: &sqlx::PgPool,
    tx_id: i64,
    root: &[u8; 64],
    triple_count: usize,
) -> Result<[u8; 64], sqlx::Error> {
    // Fetch the previous chained root (most recent tx).
    let prev: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT chained_root FROM tx_merkle_roots ORDER BY tx_id DESC LIMIT 1")
            .fetch_optional(pool)
            .await?;

    let prev_root_bytes = match &prev {
        Some((bytes,)) if bytes.len() == 64 => {
            let mut arr = [0u8; 64];
            arr.copy_from_slice(bytes);
            arr
        }
        _ => [0u8; 64],
    };

    let chain_root = chained_root(root, &prev_root_bytes);

    sqlx::query(
        r#"
        INSERT INTO tx_merkle_roots (tx_id, merkle_root, chained_root, prev_root, triple_count)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (tx_id) DO NOTHING
        "#,
    )
    .bind(tx_id)
    .bind(root.as_slice())
    .bind(chain_root.as_slice())
    .bind(prev_root_bytes.as_slice())
    .bind(triple_count as i32)
    .execute(pool)
    .await?;

    Ok(chain_root)
}

/// Verify that a single transaction's Merkle root matches its stored triples.
pub async fn verify_tx(pool: &sqlx::PgPool, tx_id: i64) -> Result<TxVerification, sqlx::Error> {
    // Fetch the stored root.
    let stored: Option<StoredMerkleRoot> = sqlx::query_as(
        "SELECT tx_id, merkle_root, chained_root, prev_root, triple_count, created_at FROM tx_merkle_roots WHERE tx_id = $1",
    )
    .bind(tx_id)
    .fetch_optional(pool)
    .await?;

    let stored = match stored {
        Some(s) => s,
        None => {
            return Ok(TxVerification {
                tx_id,
                valid: false,
                detail: "No Merkle root recorded for this transaction".into(),
                stored_root: None,
                computed_root: None,
                triple_count: 0,
            });
        }
    };

    // Fetch the triples for this tx.
    let triples: Vec<Triple> = sqlx::query_as(
        "SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at FROM triples WHERE tx_id = $1 ORDER BY id",
    )
    .bind(tx_id)
    .fetch_all(pool)
    .await?;

    let computed = merkle_root(&triples);
    let stored_root_arr = bytes_to_64(&stored.merkle_root);
    let valid = computed == stored_root_arr;

    Ok(TxVerification {
        tx_id,
        valid,
        detail: if valid {
            "Merkle root matches stored triples".into()
        } else {
            "TAMPER DETECTED: computed root does not match stored root".into()
        },
        stored_root: Some(hex::encode(stored_root_arr)),
        computed_root: Some(hex::encode(computed)),
        triple_count: triples.len(),
    })
}

/// Verify the entire hash chain is unbroken.
pub async fn verify_chain(pool: &sqlx::PgPool) -> Result<ChainVerification, sqlx::Error> {
    let rows: Vec<StoredMerkleRoot> = sqlx::query_as(
        "SELECT tx_id, merkle_root, chained_root, prev_root, triple_count, created_at FROM tx_merkle_roots ORDER BY tx_id ASC",
    )
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(ChainVerification {
            valid: true,
            total_transactions: 0,
            first_broken_tx: None,
            detail: "No transactions recorded yet".into(),
        });
    }

    let mut prev_chained = [0u8; 64]; // Genesis: null root.

    for row in &rows {
        let stored_prev = bytes_to_64(&row.prev_root);
        if stored_prev != prev_chained {
            return Ok(ChainVerification {
                valid: false,
                total_transactions: rows.len(),
                first_broken_tx: Some(row.tx_id),
                detail: format!(
                    "Chain broken at tx {}: stored prev_root does not match previous chained_root",
                    row.tx_id
                ),
            });
        }

        // Recompute the chained root.
        let merkle = bytes_to_64(&row.merkle_root);
        let expected_chained = chained_root(&merkle, &prev_chained);
        let actual_chained = bytes_to_64(&row.chained_root);

        if expected_chained != actual_chained {
            return Ok(ChainVerification {
                valid: false,
                total_transactions: rows.len(),
                first_broken_tx: Some(row.tx_id),
                detail: format!("Chain broken at tx {}: chained_root mismatch", row.tx_id),
            });
        }

        prev_chained = actual_chained;
    }

    Ok(ChainVerification {
        valid: true,
        total_transactions: rows.len(),
        first_broken_tx: None,
        detail: format!("Hash chain intact across {} transactions", rows.len()),
    })
}

/// Fetch Merkle proof for all triples belonging to an entity within a
/// specific transaction (or the latest transaction containing that entity).
pub async fn entity_proof(
    pool: &sqlx::PgPool,
    entity_id: Uuid,
) -> Result<Vec<EntityMerkleProof>, sqlx::Error> {
    // Find all tx_ids that have triples for this entity.
    let tx_ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT tx_id FROM triples WHERE entity_id = $1 ORDER BY tx_id DESC",
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;

    let mut proofs = Vec::new();

    for (tx_id,) in tx_ids {
        // Get all triples in that transaction.
        let all_triples: Vec<Triple> = sqlx::query_as(
            "SELECT id, entity_id, attribute, value, value_type, tx_id, created_at, retracted, expires_at FROM triples WHERE tx_id = $1 ORDER BY id",
        )
        .bind(tx_id)
        .fetch_all(pool)
        .await?;

        // Get the entity's triples within this tx.
        let entity_triples: Vec<&Triple> = all_triples
            .iter()
            .filter(|t| t.entity_id == entity_id)
            .collect();

        let mut triple_proofs = Vec::new();
        for triple in entity_triples {
            if let Some(proof) = generate_proof(triple, &all_triples) {
                triple_proofs.push(proof);
            }
        }

        if !triple_proofs.is_empty() {
            proofs.push(EntityMerkleProof {
                tx_id,
                entity_id,
                proofs: triple_proofs,
            });
        }
    }

    Ok(proofs)
}

// ── Response types ─────────────────────────────────────────────────

/// Result of verifying a single transaction's Merkle root.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TxVerification {
    pub tx_id: i64,
    pub valid: bool,
    pub detail: String,
    pub stored_root: Option<String>,
    pub computed_root: Option<String>,
    pub triple_count: usize,
}

/// Result of verifying the entire hash chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainVerification {
    pub valid: bool,
    pub total_transactions: usize,
    pub first_broken_tx: Option<i64>,
    pub detail: String,
}

/// Merkle proofs for all of an entity's triples within a transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMerkleProof {
    pub tx_id: i64,
    pub entity_id: Uuid,
    pub proofs: Vec<MerkleProof>,
}

/// Internal row type for reading from tx_merkle_roots.
#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
struct StoredMerkleRoot {
    tx_id: i64,
    merkle_root: Vec<u8>,
    chained_root: Vec<u8>,
    prev_root: Vec<u8>,
    triple_count: i32,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// Convert a Vec<u8> to a [u8; 64], zero-padding if too short.
fn bytes_to_64(bytes: &[u8]) -> [u8; 64] {
    let mut out = [0u8; 64];
    let len = bytes.len().min(64);
    out[..len].copy_from_slice(&bytes[..len]);
    out
}

// ── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn make_triple(
        entity_id: Uuid,
        attribute: &str,
        value: serde_json::Value,
        tx_id: i64,
    ) -> Triple {
        Triple {
            id: 0,
            entity_id,
            attribute: attribute.to_string(),
            value,
            value_type: 0,
            tx_id,
            created_at: Utc::now(),
            retracted: false,
            expires_at: None,
        }
    }

    #[test]
    fn test_hash_triple_deterministic() {
        let id = Uuid::new_v4();
        let t1 = make_triple(id, "user/name", json!("Alice"), 1);
        let t2 = make_triple(id, "user/name", json!("Alice"), 1);
        assert_eq!(hash_triple(&t1), hash_triple(&t2));
    }

    #[test]
    fn test_hash_triple_different_values() {
        let id = Uuid::new_v4();
        let t1 = make_triple(id, "user/name", json!("Alice"), 1);
        let t2 = make_triple(id, "user/name", json!("Bob"), 1);
        assert_ne!(hash_triple(&t1), hash_triple(&t2));
    }

    #[test]
    fn test_merkle_root_empty() {
        assert_eq!(merkle_root(&[]), [0u8; 64]);
    }

    #[test]
    fn test_merkle_root_single() {
        let id = Uuid::new_v4();
        let t = make_triple(id, "user/name", json!("Alice"), 1);
        let root = merkle_root(std::slice::from_ref(&t));
        assert_eq!(root, hash_triple(&t));
    }

    #[test]
    fn test_merkle_root_two() {
        let id = Uuid::new_v4();
        let t1 = make_triple(id, "user/name", json!("Alice"), 1);
        let t2 = make_triple(id, "user/email", json!("alice@example.com"), 1);
        let root = merkle_root(&[t1.clone(), t2.clone()]);
        let expected = hash_pair(&hash_triple(&t1), &hash_triple(&t2));
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_three_duplicates_last() {
        let id = Uuid::new_v4();
        let t1 = make_triple(id, "a", json!(1), 1);
        let t2 = make_triple(id, "b", json!(2), 1);
        let t3 = make_triple(id, "c", json!(3), 1);

        let root = merkle_root(&[t1.clone(), t2.clone(), t3.clone()]);

        // With 3 leaves, t3 is duplicated: [t1, t2, t3, t3]
        let h0 = hash_triple(&t1);
        let h1 = hash_triple(&t2);
        let h2 = hash_triple(&t3);
        let h3 = h2; // duplicated
        let left = hash_pair(&h0, &h1);
        let right = hash_pair(&h2, &h3);
        let expected = hash_pair(&left, &right);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_chained_root() {
        let root_a = [1u8; 64];
        let prev = [0u8; 64];
        let chained = chained_root(&root_a, &prev);
        // Should be deterministic.
        assert_eq!(chained, chained_root(&root_a, &prev));
        // Different prev => different chained.
        let prev2 = [2u8; 64];
        assert_ne!(chained, chained_root(&root_a, &prev2));
    }

    #[test]
    fn test_verify_proof_single() {
        let id = Uuid::new_v4();
        let t = make_triple(id, "user/name", json!("Alice"), 1);
        let triples = vec![t.clone()];
        let proof = generate_proof(&t, &triples).unwrap();
        let leaf = hash_triple(&t);
        let root = merkle_root(&triples);
        assert!(verify_proof(&leaf, &proof.proof_path, &root));
    }

    #[test]
    fn test_verify_proof_multiple() {
        let id = Uuid::new_v4();
        let t1 = make_triple(id, "a", json!(1), 1);
        let t2 = make_triple(id, "b", json!(2), 1);
        let t3 = make_triple(id, "c", json!(3), 1);
        let t4 = make_triple(id, "d", json!(4), 1);
        let triples = vec![t1.clone(), t2.clone(), t3.clone(), t4.clone()];
        let root = merkle_root(&triples);

        // Verify proof for each triple.
        for t in &triples {
            let proof = generate_proof(t, &triples).unwrap();
            let leaf = hash_triple(t);
            assert!(
                verify_proof(&leaf, &proof.proof_path, &root),
                "Proof failed for triple with attribute '{}'",
                t.attribute
            );
        }
    }

    #[test]
    fn test_verify_proof_wrong_root_fails() {
        let id = Uuid::new_v4();
        let t1 = make_triple(id, "a", json!(1), 1);
        let t2 = make_triple(id, "b", json!(2), 1);
        let triples = vec![t1.clone(), t2.clone()];

        let proof = generate_proof(&t1, &triples).unwrap();
        let leaf = hash_triple(&t1);
        let wrong_root = [0xFFu8; 64];
        assert!(!verify_proof(&leaf, &proof.proof_path, &wrong_root));
    }

    #[test]
    fn test_generate_proof_not_in_set() {
        let id = Uuid::new_v4();
        let t1 = make_triple(id, "a", json!(1), 1);
        let t2 = make_triple(id, "b", json!(2), 1);
        let outsider = make_triple(id, "z", json!(99), 1);
        let triples = vec![t1, t2];
        assert!(generate_proof(&outsider, &triples).is_none());
    }

    #[test]
    fn test_hex_roundtrip() {
        let data = [42u8; 64];
        let encoded = hex_encode(&data);
        let decoded = hex_decode_64(&encoded).unwrap();
        assert_eq!(data, decoded);
    }
}
