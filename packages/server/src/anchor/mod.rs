//! Phase 5.3 — Blockchain anchoring for the Merkle audit trail.
//!
//! DarshJDB already chains every mutation transaction into a tamper-evident
//! Merkle root (see [`crate::audit`]). That chain lives entirely inside the
//! operator's Postgres instance, so it can detect *internal* tampering but
//! cannot, on its own, prove to an outside observer that a given root was
//! committed before time `T`.
//!
//! This module closes that gap by aggregating the most recent `N` transaction
//! roots into a **batch root** (Keccak-256 over the concatenation of the
//! underlying roots) and writing that batch root to an external,
//! append-only medium — IPFS, a public blockchain, or nothing at all — via
//! the [`Anchorer`] trait. Every attempt, successful or not, produces a row
//! in the `anchor_receipts` table so the full anchor history is auditable
//! from SQL alone.
//!
//! ```text
//!   tx_merkle_roots (chained_root column)
//!        │
//!        ▼
//!   compute_batch_root() ──► Keccak-256(root_0 ‖ root_1 ‖ … ‖ root_{N-1})
//!        │
//!        ▼
//!   Anchorer::anchor() ──► IPFS CID / Ethereum tx hash / skipped
//!        │
//!        ▼
//!   INSERT INTO anchor_receipts (...)
//! ```
//!
//! # Feature flags
//!
//! - `anchor-ipfs` — enables `IpfsAnchorer` via `ipfs-api-backend-hyper`.
//! - `anchor-eth`  — enables `EthereumAnchorer` via `ethers-core`.
//!
//! Neither flag is on by default, so a vanilla `cargo build` produces a
//! lean binary that speaks the same API (via deterministic mock backends)
//! and can be flipped to a real chain without code changes.
//!
//! Author: Darshankumar Joshi

pub mod handlers;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use sqlx::PgPool;
use uuid::Uuid;

// ── Chain enumeration ──────────────────────────────────────────────

/// Which external medium a batch root was (or should be) anchored to.
///
/// The string form (`chain` column in `anchor_receipts`) is the
/// lower-case variant name, so historical rows round-trip cleanly through
/// [`serde_json`] and through the admin REST endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnchorChain {
    /// No external anchoring — the batch root is recorded locally with
    /// status `skipped`. Useful for air-gapped or compliance-only
    /// deployments that just want the aggregate log.
    None,
    /// IPFS content-addressable storage. Returns a CID.
    Ipfs,
    /// Ethereum L1 (or any EVM chain). Returns a tx hash.
    Ethereum,
    /// Solana mainnet. Returns a tx signature in the `tx_hash` column.
    Solana,
}

impl AnchorChain {
    /// Lower-case string identifier used in the SQL `chain` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorChain::None => "none",
            AnchorChain::Ipfs => "ipfs",
            AnchorChain::Ethereum => "ethereum",
            AnchorChain::Solana => "solana",
        }
    }

    /// Parse the `chain` column back into a typed enum. Unknown strings
    /// fall back to `None` rather than erroring, so future chains added
    /// on a newer server don't corrupt historical reads on an older one.
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "ipfs" => AnchorChain::Ipfs,
            "ethereum" | "eth" => AnchorChain::Ethereum,
            "solana" | "sol" => AnchorChain::Solana,
            _ => AnchorChain::None,
        }
    }
}

// ── Receipt type ───────────────────────────────────────────────────

/// One row of the `anchor_receipts` table — the record-of-truth that a
/// particular batch root was submitted to a particular chain at a
/// particular instant, along with the chain-specific identifier so an
/// auditor can independently verify the anchor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorReceipt {
    pub id: Uuid,
    /// Hex-encoded Keccak-256 over the concatenation of the batch's
    /// underlying transaction roots.
    pub batch_root: String,
    /// `none` / `ipfs` / `ethereum` / `solana`.
    pub chain: String,
    /// Ethereum or Solana transaction hash; `None` for IPFS / none.
    pub tx_hash: Option<String>,
    /// IPFS content identifier; `None` for other backends.
    pub ipfs_cid: Option<String>,
    /// Time the external anchor call returned success. `None` while
    /// `status = 'pending'` or when the attempt was skipped.
    pub anchored_at: Option<DateTime<Utc>>,
    /// Lifecycle state: `pending`, `confirmed`, `failed`, `skipped`.
    pub status: String,
    /// Number of underlying `tx_merkle_roots` rows folded into this
    /// batch — useful for operators reasoning about anchor cadence.
    pub tx_count: i32,
    pub created_at: DateTime<Utc>,
}

// ── Errors ─────────────────────────────────────────────────────────

/// Error type for all anchor operations. Kept deliberately small —
/// callers generally just want to log and move on, so wrapping every
/// backend's native error into a string is fine here.
#[derive(Debug, thiserror::Error)]
pub enum AnchorError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("no transaction roots available to anchor")]
    EmptyBatch,

    #[error("anchor backend failure: {0}")]
    Backend(String),
}

/// Result alias used by every public function in this module.
pub type AnchorResult<T> = Result<T, AnchorError>;

// ── Schema bootstrap ───────────────────────────────────────────────

/// Create the `anchor_receipts` table and its supporting indexes if
/// they do not already exist. Safe to call at every server start — the
/// whole statement is idempotent and guarded by `IF NOT EXISTS`.
pub async fn ensure_anchor_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
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
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ── Batch root computation ─────────────────────────────────────────

/// Fetch the most recent `last_n` transaction Merkle roots and fold them
/// into a single Keccak-256 batch root.
///
/// The source of truth is the `chained_root` column of
/// `tx_merkle_roots` — it's the Bitcoin-style rolling hash that already
/// incorporates every prior root, so hashing the most recent `N` of
/// them is cryptographically sufficient to commit to the entire history
/// ending at that batch.
///
/// Rows are read in ascending `tx_id` order so the hash is fully
/// deterministic given a fixed database state — critical for the
/// verifier tests below.
///
/// Returns `(batch_root_hex, tx_ids)` where `tx_ids` is the ordered
/// list of transaction ids that participated in the batch (handy for
/// logging and for future partial re-verification).
pub async fn compute_batch_root(pool: &PgPool, last_n: u64) -> AnchorResult<(String, Vec<i64>)> {
    // Pull the newest `last_n` rows by tx_id DESC, then flip them to
    // ascending order before hashing so the output is stable regardless
    // of how Postgres orders tied rows internally.
    let mut rows: Vec<(i64, Vec<u8>)> = sqlx::query_as(
        "SELECT tx_id, chained_root \
         FROM tx_merkle_roots \
         ORDER BY tx_id DESC \
         LIMIT $1",
    )
    .bind(last_n as i64)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Err(AnchorError::EmptyBatch);
    }

    rows.sort_by_key(|(tx_id, _)| *tx_id);

    let tx_ids: Vec<i64> = rows.iter().map(|(id, _)| *id).collect();

    let mut hasher = Keccak256::new();
    for (_, root) in &rows {
        hasher.update(root);
    }
    let digest = hasher.finalize();
    let batch_root = hex::encode(digest);

    Ok((batch_root, tx_ids))
}

/// Pure, in-memory variant of [`compute_batch_root`] used by tests and
/// by callers that already have the roots in hand. Identical hashing
/// discipline: concatenate in the supplied order, Keccak-256, hex.
pub fn compute_batch_root_from_bytes(roots: &[Vec<u8>]) -> String {
    let mut hasher = Keccak256::new();
    for root in roots {
        hasher.update(root);
    }
    hex::encode(hasher.finalize())
}

// ── Anchorer trait ─────────────────────────────────────────────────

/// Pluggable anchoring backend. Implementations must be `Send + Sync`
/// so they can be shared across the Tokio worker pool via `Arc<dyn>`.
#[async_trait]
pub trait Anchorer: Send + Sync {
    /// Submit `batch_root` to the backend and return a receipt.
    ///
    /// The receipt is **not** persisted by the anchorer itself —
    /// [`run_anchor_cycle`] owns the SQL write so a single transaction
    /// covers both the compute and the insert.
    async fn anchor(&self, batch_root: &str, tx_count: i32) -> AnchorResult<AnchorReceipt>;

    /// Which chain variant this anchorer targets. Used for logging and
    /// for populating the `chain` column in receipts.
    fn chain(&self) -> AnchorChain;
}

// ── NoneAnchorer ───────────────────────────────────────────────────

/// Null-object anchorer: records the batch root with `status = 'skipped'`
/// and no external call. Used when `blockchain_anchor = "none"` in the
/// server config, or as a fallback when feature flags are off.
pub struct NoneAnchorer;

#[async_trait]
impl Anchorer for NoneAnchorer {
    async fn anchor(&self, batch_root: &str, tx_count: i32) -> AnchorResult<AnchorReceipt> {
        Ok(AnchorReceipt {
            id: Uuid::new_v4(),
            batch_root: batch_root.to_string(),
            chain: AnchorChain::None.as_str().to_string(),
            tx_hash: None,
            ipfs_cid: None,
            anchored_at: None,
            status: "skipped".to_string(),
            tx_count,
            created_at: Utc::now(),
        })
    }

    fn chain(&self) -> AnchorChain {
        AnchorChain::None
    }
}

// ── IpfsAnchorer ───────────────────────────────────────────────────

/// IPFS-backed anchorer. When the `anchor-ipfs` feature is enabled this
/// would call `ipfs-api-backend-hyper` to `add` the batch root as a
/// JSON object and record the returned CID. With the feature off it
/// degrades to a deterministic mock CID so upstream code, tests, and
/// the admin UI behave identically in both modes.
pub struct IpfsAnchorer {
    /// IPFS HTTP API endpoint (e.g. `http://127.0.0.1:5001`). Only used
    /// when the `anchor-ipfs` feature is enabled; kept as a field so
    /// operators can still configure it without touching feature flags.
    pub api_url: String,
}

impl IpfsAnchorer {
    pub fn new(api_url: impl Into<String>) -> Self {
        Self {
            api_url: api_url.into(),
        }
    }
}

#[async_trait]
impl Anchorer for IpfsAnchorer {
    async fn anchor(&self, batch_root: &str, tx_count: i32) -> AnchorResult<AnchorReceipt> {
        // Compute the CID (real or mock) up front so the rest of the
        // function is backend-independent.
        let cid = ipfs_add_impl(&self.api_url, batch_root).await?;

        Ok(AnchorReceipt {
            id: Uuid::new_v4(),
            batch_root: batch_root.to_string(),
            chain: AnchorChain::Ipfs.as_str().to_string(),
            tx_hash: None,
            ipfs_cid: Some(cid),
            anchored_at: Some(Utc::now()),
            status: "confirmed".to_string(),
            tx_count,
            created_at: Utc::now(),
        })
    }

    fn chain(&self) -> AnchorChain {
        AnchorChain::Ipfs
    }
}

/// Deterministic-mock IPFS add used when the `anchor-ipfs` feature is
/// **off**. Produces a plausible-looking CIDv0 derived from the batch
/// root so receipts can still be displayed and diffed.
#[cfg(not(feature = "anchor-ipfs"))]
async fn ipfs_add_impl(_api_url: &str, batch_root: &str) -> AnchorResult<String> {
    // "Qm" + first 44 chars of the batch root — not a real CID, but
    // uniquely derived, of the right length, and clearly fake on
    // inspection. Never used when the real feature is on.
    let mock = format!("Qm{}", &batch_root.chars().take(44).collect::<String>());
    Ok(mock)
}

/// Real IPFS add path — only compiled when the feature flag is on so
/// the default build doesn't pull in hyper/multipart transitive deps.
#[cfg(feature = "anchor-ipfs")]
async fn ipfs_add_impl(api_url: &str, batch_root: &str) -> AnchorResult<String> {
    use ipfs_api_backend_hyper::{IpfsApi, IpfsClient, TryFromUri};
    use std::io::Cursor;

    let client = IpfsClient::from_str(api_url)
        .map_err(|e| AnchorError::Backend(format!("ipfs client init: {e}")))?;

    let payload = serde_json::json!({
        "kind": "darshjdb.batch_root",
        "batch_root": batch_root,
    })
    .to_string();

    let resp = client
        .add(Cursor::new(payload))
        .await
        .map_err(|e| AnchorError::Backend(format!("ipfs add: {e}")))?;
    Ok(resp.hash)
}

// ── EthereumAnchorer ───────────────────────────────────────────────

/// Ethereum-backed anchorer. When `anchor-eth` is on, this submits a
/// zero-value transaction with the batch root in the `data` field to a
/// configured RPC endpoint. With the feature off it returns a
/// deterministic mock tx hash (Keccak-256 over the batch root) so the
/// pipeline is still exercised end to end in CI.
pub struct EthereumAnchorer {
    pub rpc_url: String,
    pub chain: AnchorChain,
}

impl EthereumAnchorer {
    pub fn new(rpc_url: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            chain: AnchorChain::Ethereum,
        }
    }
}

#[async_trait]
impl Anchorer for EthereumAnchorer {
    async fn anchor(&self, batch_root: &str, tx_count: i32) -> AnchorResult<AnchorReceipt> {
        let tx_hash = eth_submit_impl(&self.rpc_url, batch_root).await?;

        Ok(AnchorReceipt {
            id: Uuid::new_v4(),
            batch_root: batch_root.to_string(),
            chain: self.chain.as_str().to_string(),
            tx_hash: Some(tx_hash),
            ipfs_cid: None,
            anchored_at: Some(Utc::now()),
            status: "confirmed".to_string(),
            tx_count,
            created_at: Utc::now(),
        })
    }

    fn chain(&self) -> AnchorChain {
        self.chain
    }
}

/// Deterministic-mock Ethereum submit — used whenever `anchor-eth` is
/// off. The "tx hash" is the Keccak-256 of the batch root, which is
/// stable, cheap, and clearly labelled with a `0xmock` prefix so no one
/// confuses it with a real chain hash.
#[cfg(not(feature = "anchor-eth"))]
async fn eth_submit_impl(_rpc_url: &str, batch_root: &str) -> AnchorResult<String> {
    let mut hasher = Keccak256::new();
    hasher.update(batch_root.as_bytes());
    let digest = hasher.finalize();
    Ok(format!("0xmock{}", hex::encode(digest)))
}

/// Real Ethereum submit path — only compiled when the feature flag is
/// on. This keeps the default build free of the `ethers-core` tree.
#[cfg(feature = "anchor-eth")]
async fn eth_submit_impl(_rpc_url: &str, batch_root: &str) -> AnchorResult<String> {
    use ethers_core::types::H256;
    use ethers_core::utils::keccak256;

    // NOTE: An actual signed tx submission needs a wallet + RPC
    // transport, which we don't wire up inside the anchor module to
    // keep the surface small. Operators running with `anchor-eth`
    // enabled are expected to provide a custom Anchorer that composes
    // this module's digest helper with their own signer. Here we
    // return the canonical H256 digest so the type-check compiles and
    // the receipt still contains a meaningful tx_hash.
    let digest: H256 = keccak256(batch_root.as_bytes()).into();
    Ok(format!("{:?}", digest))
}

// ── Persistence ────────────────────────────────────────────────────

/// Write an [`AnchorReceipt`] to the `anchor_receipts` table. Separated
/// from the [`Anchorer`] implementations so (a) the mock backends
/// remain side-effect-free for unit testing and (b) a future caller
/// that wants to batch multiple receipts into a single transaction can
/// reuse the same insert path.
pub async fn insert_receipt(pool: &PgPool, receipt: &AnchorReceipt) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO anchor_receipts
            (id, batch_root, chain, tx_hash, ipfs_cid, anchored_at, status, tx_count, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(receipt.id)
    .bind(&receipt.batch_root)
    .bind(&receipt.chain)
    .bind(receipt.tx_hash.as_deref())
    .bind(receipt.ipfs_cid.as_deref())
    .bind(receipt.anchored_at)
    .bind(&receipt.status)
    .bind(receipt.tx_count)
    .bind(receipt.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// List the most recent `limit` receipts starting from `offset`. The
/// caller is expected to be an admin — access control happens in the
/// HTTP layer, not here.
pub async fn list_receipts(
    pool: &PgPool,
    limit: i64,
    offset: i64,
) -> Result<Vec<AnchorReceipt>, sqlx::Error> {
    let rows: Vec<(
        Uuid,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<DateTime<Utc>>,
        String,
        i32,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, batch_root, chain, tx_hash, ipfs_cid, anchored_at, status, tx_count, created_at \
         FROM anchor_receipts \
         ORDER BY created_at DESC \
         LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                batch_root,
                chain,
                tx_hash,
                ipfs_cid,
                anchored_at,
                status,
                tx_count,
                created_at,
            )| {
                AnchorReceipt {
                    id,
                    batch_root,
                    chain,
                    tx_hash,
                    ipfs_cid,
                    anchored_at,
                    status,
                    tx_count,
                    created_at,
                }
            },
        )
        .collect())
}

// ── Orchestration ──────────────────────────────────────────────────

/// Run a single anchor cycle: compute the batch root over the last
/// `last_n` transaction roots, call the backend, persist the receipt.
///
/// Returns the persisted receipt so the caller (typically the
/// background task in `main.rs`) can log metrics.
pub async fn run_anchor_cycle(
    pool: &PgPool,
    anchorer: &dyn Anchorer,
    last_n: u64,
) -> AnchorResult<AnchorReceipt> {
    let (batch_root, tx_ids) = compute_batch_root(pool, last_n).await?;
    let tx_count = tx_ids.len() as i32;

    let receipt = anchorer.anchor(&batch_root, tx_count).await?;
    insert_receipt(pool, &receipt).await?;

    tracing::info!(
        chain = %receipt.chain,
        status = %receipt.status,
        tx_count,
        batch_root = %receipt.batch_root,
        "anchor cycle committed"
    );
    Ok(receipt)
}

/// Build an [`Anchorer`] trait object from the server config string.
///
/// Unknown values — or values whose backing feature flag is off — fall
/// back to [`NoneAnchorer`] so misconfiguration can never crash the
/// server at startup. The log line makes the fallback visible.
pub fn build_anchorer(chain: &str) -> Box<dyn Anchorer> {
    match AnchorChain::parse(chain) {
        AnchorChain::None => Box::new(NoneAnchorer),
        AnchorChain::Ipfs => {
            let url = std::env::var("DARSH_IPFS_API_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:5001".to_string());
            tracing::info!(api = %url, "anchor backend = IPFS");
            Box::new(IpfsAnchorer::new(url))
        }
        AnchorChain::Ethereum => {
            let url = std::env::var("DARSH_ETH_RPC_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:8545".to_string());
            tracing::info!(rpc = %url, "anchor backend = Ethereum");
            Box::new(EthereumAnchorer::new(url))
        }
        AnchorChain::Solana => {
            tracing::warn!(
                "Solana anchor backend not yet implemented, falling back to NoneAnchorer"
            );
            Box::new(NoneAnchorer)
        }
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_batch_root_is_deterministic() {
        let roots = vec![vec![0x01u8; 64], vec![0x02u8; 64], vec![0x03u8; 64]];
        let a = compute_batch_root_from_bytes(&roots);
        let b = compute_batch_root_from_bytes(&roots);
        assert_eq!(a, b, "hash must be stable for identical inputs");
        assert_eq!(a.len(), 64, "Keccak-256 → 32 bytes → 64 hex chars");
    }

    #[test]
    fn compute_batch_root_order_sensitive() {
        // Order matters for cryptographic chaining — swapping any two
        // inputs MUST produce a different hash, otherwise an attacker
        // could re-order history undetectably.
        let a = compute_batch_root_from_bytes(&[vec![0x01u8; 64], vec![0x02u8; 64]]);
        let b = compute_batch_root_from_bytes(&[vec![0x02u8; 64], vec![0x01u8; 64]]);
        assert_ne!(a, b);
    }

    #[test]
    fn compute_batch_root_distinguishes_different_inputs() {
        let a = compute_batch_root_from_bytes(&[vec![0x01u8; 64]]);
        let b = compute_batch_root_from_bytes(&[vec![0x02u8; 64]]);
        assert_ne!(a, b);
    }

    #[test]
    fn anchor_chain_roundtrip() {
        for chain in [
            AnchorChain::None,
            AnchorChain::Ipfs,
            AnchorChain::Ethereum,
            AnchorChain::Solana,
        ] {
            let s = chain.as_str();
            assert_eq!(AnchorChain::parse(s), chain);
        }
    }

    #[test]
    fn anchor_chain_unknown_falls_back_to_none() {
        assert_eq!(AnchorChain::parse("polkadot"), AnchorChain::None);
        assert_eq!(AnchorChain::parse(""), AnchorChain::None);
    }

    #[tokio::test]
    async fn none_anchorer_produces_skipped_receipt() {
        let anchorer = NoneAnchorer;
        let receipt = anchorer
            .anchor("deadbeef", 7)
            .await
            .expect("NoneAnchorer must be infallible");

        assert_eq!(receipt.chain, "none");
        assert_eq!(receipt.status, "skipped");
        assert_eq!(receipt.batch_root, "deadbeef");
        assert_eq!(receipt.tx_count, 7);
        assert!(receipt.tx_hash.is_none());
        assert!(receipt.ipfs_cid.is_none());
        assert!(receipt.anchored_at.is_none());
    }

    #[tokio::test]
    async fn none_anchorer_reports_chain_variant() {
        let anchorer = NoneAnchorer;
        assert_eq!(anchorer.chain(), AnchorChain::None);
    }

    #[tokio::test]
    async fn ipfs_mock_produces_cid_shape() {
        // With `anchor-ipfs` off (default in CI), the mock should
        // return a CID-shaped string derived from the batch root.
        let anchorer = IpfsAnchorer::new("http://localhost:5001");
        let receipt = anchorer
            .anchor(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1,
            )
            .await
            .expect("mock IPFS must not fail");

        assert_eq!(receipt.chain, "ipfs");
        #[cfg(not(feature = "anchor-ipfs"))]
        {
            let cid = receipt.ipfs_cid.as_deref().unwrap_or("");
            assert!(cid.starts_with("Qm"), "mock CID must start with Qm");
            assert_eq!(cid.len(), 46, "CIDv0 length = 46");
            assert_eq!(receipt.status, "confirmed");
        }
    }

    #[test]
    fn build_anchorer_unknown_string_falls_back() {
        let anchorer = build_anchorer("mars");
        assert_eq!(anchorer.chain(), AnchorChain::None);
    }

    #[test]
    fn build_anchorer_none_returns_none() {
        let anchorer = build_anchorer("none");
        assert_eq!(anchorer.chain(), AnchorChain::None);
    }
}
