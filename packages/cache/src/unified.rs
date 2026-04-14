// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache::unified — `DdbCache`: read-through / write-through L1+L2 layer.
//
// Slice 10 (Phase 1.3) of the DarshJDB Grand Transformation.
//
// Semantics
// ---------
//   * GET    — L1 first. On miss, fall through to L2; on L2 hit, populate
//              L1 so subsequent reads are O(1) in-process. On L2 miss,
//              return `None`.
//   * SET    — Write through to both tiers concurrently via `tokio::join!`
//              so the durable write cannot starve the hot-path write.
//   * DELETE — Remove from both tiers concurrently. Returns `true` if the
//              key existed in *either* tier.
//   * CLEAR  — Delegates to L1 (L2 scaffold has no bulk clear; when
//              Slice 9 lands this becomes `TRUNCATE kv_store`).
//
// Metrics
// -------
// Every read/write/delete emits Prometheus counters/gauges via the
// `metrics` crate facade. A `metrics-exporter-prometheus` recorder must
// be installed in the binary for scraping; this crate is transport-agnostic.
//
//   ddb_cache_l1_hits_total       — counter
//   ddb_cache_l1_misses_total     — counter
//   ddb_cache_l2_hits_total       — counter
//   ddb_cache_l2_misses_total     — counter
//   ddb_cache_evictions_total     — counter (reserved; populated by L1 eviction)
//   ddb_cache_memory_bytes        — gauge   (L1 + L2 resident bytes)
//
// Author: Darshankumar Joshi

use bytes::Bytes;
use std::sync::Arc;

// Slice 10 unified layer deliberately uses the Bytes-tier scaffolds
// (`BytesL1Cache` / `BytesL2Cache`) rather than the richer typed L1
// (hash/list/zset) and Postgres-backed L2 that Slices 8 & 9 deliver.
// Those tiers expose a much larger surface area; plumbing them through
// the unified read-through / write-through semantics is tracked as a
// follow-up in Slice 11/12. The byte-tier scaffolds keep the unified API
// and its tests exactly as designed.
use crate::l1_bytes::BytesL1Cache as L1Cache;
use crate::l2_bytes::BytesL2Cache as L2Cache;
use crate::pubsub::PubSubEngine;

/// Unified two-tier cache with pub/sub for keyspace notifications.
///
/// This is the single entry point higher layers (server, query engine,
/// storage) should hold. The underlying tiers remain accessible via the
/// public fields for advanced callers that need to bypass one tier.
#[derive(Debug)]
pub struct DdbCache {
    /// L1 in-memory DashMap tier.
    pub l1: Arc<L1Cache>,
    /// L2 durable (Postgres in production) tier.
    pub l2: Arc<L2Cache>,
    /// Keyspace notification pub/sub engine.
    pub pubsub: Arc<PubSubEngine>,
}

impl DdbCache {
    /// Compose a unified cache from the given tiers.
    pub fn new(l1: Arc<L1Cache>, l2: Arc<L2Cache>, pubsub: Arc<PubSubEngine>) -> Arc<Self> {
        Arc::new(Self { l1, l2, pubsub })
    }

    /// Convenience constructor that builds default in-memory tiers and a
    /// default-capacity pub/sub engine. Intended for tests and bootstrap;
    /// production callers should build tiers explicitly so the L2 tier
    /// can be wired to a real Postgres pool.
    pub fn in_memory() -> Arc<Self> {
        Self::new(
            L1Cache::new(),
            L2Cache::new_in_memory(),
            PubSubEngine::new_default(),
        )
    }

    /// Read-through GET.
    ///
    /// 1. Probe L1. Hit → record `l1_hits`, return.
    /// 2. Miss → record `l1_misses`, probe L2.
    /// 3. L2 hit → record `l2_hits`, populate L1, return.
    /// 4. L2 miss → record `l2_misses`, return `None`.
    pub async fn get(&self, key: &str) -> Option<Bytes> {
        if let Some(v) = self.l1.get(key).await {
            metrics::counter!("ddb_cache_l1_hits_total").increment(1);
            self.record_memory();
            return Some(v);
        }
        metrics::counter!("ddb_cache_l1_misses_total").increment(1);

        if let Some(v) = self.l2.get(key).await {
            metrics::counter!("ddb_cache_l2_hits_total").increment(1);
            // Populate L1 so subsequent reads are hot.
            self.l1.set(key, v.clone()).await;
            self.record_memory();
            Some(v)
        } else {
            metrics::counter!("ddb_cache_l2_misses_total").increment(1);
            self.record_memory();
            None
        }
    }

    /// Write-through SET: L1 and L2 written concurrently.
    pub async fn set(&self, key: &str, value: Bytes) {
        let l1 = self.l1.clone();
        let l2 = self.l2.clone();
        let k1 = key.to_string();
        let k2 = key.to_string();
        let v1 = value.clone();
        let v2 = value;
        let (_, _) = tokio::join!(
            async move { l1.set(&k1, v1).await },
            async move { l2.set(&k2, v2).await },
        );
        self.record_memory();
    }

    /// DELETE from both tiers concurrently. Returns `true` if the key
    /// was present in *either* tier prior to removal.
    pub async fn delete(&self, key: &str) -> bool {
        let l1 = self.l1.clone();
        let l2 = self.l2.clone();
        let k1 = key.to_string();
        let k2 = key.to_string();
        let (d1, d2) = tokio::join!(
            async move { l1.delete(&k1).await },
            async move { l2.delete(&k2).await },
        );
        self.record_memory();
        d1 || d2
    }

    /// Clear L1 only. L2 bulk-clear lands with Slice 9.
    pub async fn clear_l1(&self) {
        self.l1.clear().await;
        self.record_memory();
    }

    /// Publish a keyspace notification on `channel`.
    pub fn notify(&self, channel: &str, payload: Bytes) -> usize {
        self.pubsub.publish(channel, payload)
    }

    /// Update the `ddb_cache_memory_bytes` gauge from the current tiers.
    fn record_memory(&self) {
        let total = self.l1.memory_bytes() + self.l2.memory_bytes();
        metrics::gauge!("ddb_cache_memory_bytes").set(total as f64);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Bytes {
        Bytes::copy_from_slice(s.as_bytes())
    }

    #[tokio::test]
    async fn read_through_l1_miss_populates_from_l2() {
        let l1 = L1Cache::new();
        let l2 = L2Cache::new_in_memory();
        let pubsub = PubSubEngine::new_default();
        let cache = DdbCache::new(l1.clone(), l2.clone(), pubsub.clone());
        drop(pubsub);

        // Seed L2 only.
        l2.set("alpha", b("one")).await;
        assert!(l1.get("alpha").await.is_none(), "L1 must start empty");

        // Read through — should hit L2 and populate L1.
        let got = cache.get("alpha").await;
        assert_eq!(got, Some(b("one")));

        // Next L1 read must be a direct hit (no L2 involvement).
        let hot = l1.get("alpha").await;
        assert_eq!(hot, Some(b("one")), "L1 must be populated on L2 hit");
    }

    #[tokio::test]
    async fn read_through_total_miss_returns_none() {
        let cache = DdbCache::in_memory();
        assert_eq!(cache.get("nope").await, None);
    }

    #[tokio::test]
    async fn write_through_writes_both_tiers() {
        let l1 = L1Cache::new();
        let l2 = L2Cache::new_in_memory();
        let cache = DdbCache::new(l1.clone(), l2.clone(), PubSubEngine::new_default());

        cache.set("k", b("v")).await;

        assert_eq!(l1.get("k").await, Some(b("v")), "L1 must be written");
        assert_eq!(l2.get("k").await, Some(b("v")), "L2 must be written");
    }

    #[tokio::test]
    async fn delete_removes_from_both_tiers() {
        let l1 = L1Cache::new();
        let l2 = L2Cache::new_in_memory();
        let cache = DdbCache::new(l1.clone(), l2.clone(), PubSubEngine::new_default());

        cache.set("k", b("v")).await;
        assert!(cache.delete("k").await, "delete must report previous presence");
        assert!(l1.get("k").await.is_none());
        assert!(l2.get("k").await.is_none());

        // Second delete: both tiers empty → false.
        assert!(!cache.delete("k").await);
    }

    #[tokio::test]
    async fn delete_reports_true_when_only_in_l2() {
        let l1 = L1Cache::new();
        let l2 = L2Cache::new_in_memory();
        let cache = DdbCache::new(l1.clone(), l2.clone(), PubSubEngine::new_default());

        // Key exists only in L2.
        l2.set("ghost", b("x")).await;
        assert!(cache.delete("ghost").await);
        assert!(l2.get("ghost").await.is_none());
    }

    #[tokio::test]
    async fn pubsub_roundtrip_delivers_message() {
        let cache = DdbCache::in_memory();
        let mut rx = cache.pubsub.subscribe("__keyspace__:foo");

        let n = cache.notify("__keyspace__:foo", b("set"));
        assert_eq!(n, 1, "exactly one subscriber must receive");

        let msg = rx.recv().await.expect("message delivered");
        assert_eq!(msg.channel, "__keyspace__:foo");
        assert_eq!(msg.payload, b("set"));
    }

    #[tokio::test]
    async fn get_after_write_through_is_l1_hit() {
        let cache = DdbCache::in_memory();
        cache.set("hot", b("path")).await;
        // Explicitly clear L2 to prove the next read must be served by L1.
        cache.l2.delete("hot").await;
        assert_eq!(cache.get("hot").await, Some(b("path")));
    }

    #[tokio::test]
    async fn unified_serves_concurrent_readers_and_writers() {
        let cache = DdbCache::in_memory();
        let mut handles = Vec::new();
        for i in 0..32u32 {
            let c = cache.clone();
            handles.push(tokio::spawn(async move {
                let key = format!("k{i}");
                let val = Bytes::copy_from_slice(format!("v{i}").as_bytes());
                c.set(&key, val.clone()).await;
                assert_eq!(c.get(&key).await, Some(val));
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    }
}
