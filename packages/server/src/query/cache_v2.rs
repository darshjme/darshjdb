//! Multi-tier query cache for DarshJDB (v2).
//!
//! Replaces the single-tier LRU cache with a two-level architecture:
//!
//! - **L1**: In-process `DashMap` (hot cache, bounded to 1000 entries).
//! - **L2**: Optional Redis (warm cache, configurable TTL and size).
//!
//! Features smart invalidation (only flush queries touching the mutated
//! entity type + attribute), cache warming on startup, and detailed metrics.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, info, warn};
use uuid::Uuid;

// ── Cache Key ──────────────────────────────────────────────────────

/// A cache key derived from (user_id, query, params).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey(u64);

impl CacheKey {
    /// Compute a cache key from the user context and query.
    pub fn new(user_id: Option<&Uuid>, query: &Value) -> Self {
        let mut hasher = DefaultHasher::new();
        if let Some(uid) = user_id {
            uid.hash(&mut hasher);
        } else {
            0u8.hash(&mut hasher);
        }
        let canonical = query.to_string();
        canonical.hash(&mut hasher);
        Self(hasher.finish())
    }

    /// Create from a raw hash (for testing).
    pub fn from_raw(hash: u64) -> Self {
        Self(hash)
    }

    /// Get the raw hash value.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

// ── Cache Entry ────────────────────────────────────────────────────

/// A cached query result with metadata.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The cached JSON result data.
    pub result: Value,
    /// When this entry was created.
    pub created_at: Instant,
    /// Number of times this entry has been hit.
    pub hit_count: u64,
    /// Approximate byte size of the serialized result.
    pub byte_size: usize,
    /// Entity type this query targets (for selective invalidation).
    entity_type: String,
    /// Attributes filtered on (for granular invalidation).
    filtered_attributes: Vec<String>,
    /// TTL for this entry.
    ttl: Duration,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

// ── Cache Metrics ──────────────────────────────────────────────────

/// Detailed cache performance metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMetrics {
    /// L1 hit count.
    pub l1_hits: u64,
    /// L1 miss count.
    pub l1_misses: u64,
    /// L2 hit count (Redis).
    pub l2_hits: u64,
    /// L2 miss count (Redis).
    pub l2_misses: u64,
    /// Total cache hit rate (0.0 - 100.0).
    pub hit_rate: f64,
    /// Total cache miss rate (0.0 - 100.0).
    pub miss_rate: f64,
    /// Number of evictions performed.
    pub eviction_count: u64,
    /// Approximate total byte usage of L1.
    pub byte_usage: u64,
    /// Number of entries currently in L1.
    pub l1_size: usize,
    /// Number of invalidations performed.
    pub invalidation_count: u64,
}

// ── L2 Redis Backend (trait for testability) ───────────────────────

/// Trait abstracting the L2 cache backend.
///
/// The default implementation is a no-op. When Redis is configured,
/// a real implementation connects via the `redis` crate.
#[allow(async_fn_in_trait)]
pub trait L2Backend: Send + Sync + 'static {
    /// Get a value from L2.
    async fn get(&self, key: u64) -> Option<Value>;
    /// Set a value in L2 with TTL.
    async fn set(&self, key: u64, value: &Value, ttl: Duration);
    /// Delete a value from L2.
    async fn delete(&self, key: u64);
    /// Delete all keys matching a pattern.
    async fn flush(&self);
    /// Whether L2 is available.
    fn is_available(&self) -> bool;
}

/// No-op L2 backend (used when Redis is not configured).
pub struct NoOpL2;

impl L2Backend for NoOpL2 {
    async fn get(&self, _key: u64) -> Option<Value> { None }
    async fn set(&self, _key: u64, _value: &Value, _ttl: Duration) {}
    async fn delete(&self, _key: u64) {}
    async fn flush(&self) {}
    fn is_available(&self) -> bool { false }
}

// ── Multi-Tier Cache ───────────────────────────────────────────────

/// Two-level cache with DashMap L1 and optional Redis L2.
pub struct MultiTierCache<L2: L2Backend = NoOpL2> {
    /// L1 hot cache: in-process DashMap.
    l1: DashMap<u64, CacheEntry>,
    /// L1 capacity limit.
    l1_max_entries: usize,
    /// Default TTL for entries.
    default_ttl: Duration,
    /// L2 warm cache backend.
    l2: Arc<L2>,
    /// Whether the cache is globally enabled.
    enabled: bool,

    // Atomic metrics counters.
    l1_hits: AtomicU64,
    l1_misses: AtomicU64,
    l2_hits: AtomicU64,
    l2_misses: AtomicU64,
    evictions: AtomicU64,
    invalidations: AtomicU64,
}

impl MultiTierCache<NoOpL2> {
    /// Create a new multi-tier cache without an L2 backend.
    pub fn new(max_entries: usize, default_ttl: Duration, enabled: bool) -> Self {
        Self::with_l2(max_entries, default_ttl, enabled, Arc::new(NoOpL2))
    }
}

impl<L2: L2Backend> MultiTierCache<L2> {
    /// Create a new multi-tier cache with a custom L2 backend.
    pub fn with_l2(
        max_entries: usize,
        default_ttl: Duration,
        enabled: bool,
        l2: Arc<L2>,
    ) -> Self {
        Self {
            l1: DashMap::with_capacity(max_entries.min(1024)),
            l1_max_entries: max_entries,
            default_ttl,
            l2,
            enabled,
            l1_hits: AtomicU64::new(0),
            l1_misses: AtomicU64::new(0),
            l2_hits: AtomicU64::new(0),
            l2_misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
        }
    }

    /// Build from environment variables.
    ///
    /// | Variable                   | Default | Description              |
    /// |----------------------------|---------|--------------------------|
    /// | `DDB_CACHE_V2_SIZE`        | 1000    | Max L1 entries           |
    /// | `DDB_CACHE_V2_TTL`         | 60      | TTL in seconds           |
    /// | `DDB_CACHE_V2_ENABLED`     | true    | Master on/off switch     |
    pub fn from_env() -> MultiTierCache<NoOpL2> {
        let max_entries: usize = std::env::var("DDB_CACHE_V2_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000);

        let ttl_secs: u64 = std::env::var("DDB_CACHE_V2_TTL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        let enabled: bool = std::env::var("DDB_CACHE_V2_ENABLED")
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true);

        info!(
            max_entries,
            ttl_secs,
            enabled,
            "cache v2 initialized from environment"
        );

        MultiTierCache::new(max_entries, Duration::from_secs(ttl_secs), enabled)
    }

    // ── Read Path ──────────────────────────────────────────────────

    /// Look up a cached result. Checks L1 first, then L2.
    pub async fn get(&self, key: &CacheKey) -> Option<Value> {
        if !self.enabled {
            return None;
        }

        // L1 lookup.
        if let Some(mut entry) = self.l1.get_mut(&key.0) {
            if entry.is_expired() {
                drop(entry);
                self.l1.remove(&key.0);
                self.l1_misses.fetch_add(1, Ordering::Relaxed);
            } else {
                entry.hit_count += 1;
                let result = entry.result.clone();
                self.l1_hits.fetch_add(1, Ordering::Relaxed);
                return Some(result);
            }
        } else {
            self.l1_misses.fetch_add(1, Ordering::Relaxed);
        }

        // L2 lookup.
        if self.l2.is_available() {
            if let Some(value) = self.l2.get(key.0).await {
                self.l2_hits.fetch_add(1, Ordering::Relaxed);
                // Promote to L1.
                self.set_l1(
                    key,
                    value.clone(),
                    "promoted".to_string(),
                    vec![],
                );
                return Some(value);
            }
            self.l2_misses.fetch_add(1, Ordering::Relaxed);
        }

        None
    }

    // ── Write Path ─────────────────────────────────────────────────

    /// Insert a query result into both cache tiers.
    pub async fn set(
        &self,
        key: &CacheKey,
        result: Value,
        entity_type: String,
        filtered_attributes: Vec<String>,
    ) {
        if !self.enabled {
            return;
        }

        self.set_l1(key, result.clone(), entity_type, filtered_attributes);

        // Write-through to L2.
        if self.l2.is_available() {
            self.l2.set(key.0, &result, self.default_ttl).await;
        }
    }

    /// Insert into L1 only (used for promotions from L2).
    fn set_l1(
        &self,
        key: &CacheKey,
        result: Value,
        entity_type: String,
        filtered_attributes: Vec<String>,
    ) {
        // Evict if at capacity.
        if self.l1.len() >= self.l1_max_entries {
            self.evict_lru();
        }

        let byte_size = result.to_string().len();
        self.l1.insert(
            key.0,
            CacheEntry {
                result,
                created_at: Instant::now(),
                hit_count: 0,
                byte_size,
                entity_type,
                filtered_attributes,
                ttl: self.default_ttl,
            },
        );
    }

    // ── Smart Invalidation ─────────────────────────────────────────

    /// Invalidate only queries that touch the mutated entity type AND attribute.
    ///
    /// This is more granular than the v1 cache which invalidated all queries
    /// for an entity type. If `attribute` is `None`, all queries for the
    /// entity type are invalidated.
    pub async fn invalidate(&self, entity_type: &str, attribute: Option<&str>) {
        let keys_to_remove: Vec<u64> = self
            .l1
            .iter()
            .filter(|entry| {
                let e = entry.value();
                if e.entity_type != entity_type {
                    return false;
                }
                match attribute {
                    Some(attr) => {
                        // If the cached query filtered on this attribute, invalidate.
                        // Also invalidate if the query had no specific attribute filters
                        // (it might return the changed attribute as part of results).
                        e.filtered_attributes.is_empty()
                            || e.filtered_attributes.iter().any(|a| a == attr)
                    }
                    None => true,
                }
            })
            .map(|entry| *entry.key())
            .collect();

        let count = keys_to_remove.len() as u64;
        for key in &keys_to_remove {
            self.l1.remove(key);
            if self.l2.is_available() {
                self.l2.delete(*key).await;
            }
        }

        if count > 0 {
            self.invalidations.fetch_add(count, Ordering::Relaxed);
            debug!(
                entity_type,
                attribute,
                count,
                "smart cache invalidation"
            );
        }
    }

    /// Flush all entries from both tiers.
    pub async fn invalidate_all(&self) {
        let count = self.l1.len() as u64;
        self.l1.clear();
        if self.l2.is_available() {
            self.l2.flush().await;
        }
        self.invalidations.fetch_add(count, Ordering::Relaxed);
        info!(count, "flushed all cache entries");
    }

    // ── Cache Warming ──────────────────────────────────────────────

    /// Pre-populate the cache with results from popular queries.
    ///
    /// Takes a list of (CacheKey, Value, entity_type) tuples and inserts
    /// them into L1. Call this at startup with the most frequently
    /// executed queries from the previous session.
    pub fn warm(&self, entries: Vec<(CacheKey, Value, String)>) {
        if !self.enabled {
            return;
        }

        let count = entries.len();
        for (key, result, entity_type) in entries {
            self.set_l1(&key, result, entity_type, vec![]);
        }
        info!(count, "cache warmed with popular queries");
    }

    // ── Metrics ────────────────────────────────────────────────────

    /// Return current cache metrics.
    pub fn metrics(&self) -> CacheMetrics {
        let l1_hits = self.l1_hits.load(Ordering::Relaxed);
        let l1_misses = self.l1_misses.load(Ordering::Relaxed);
        let l2_hits = self.l2_hits.load(Ordering::Relaxed);
        let l2_misses = self.l2_misses.load(Ordering::Relaxed);

        let total_hits = l1_hits + l2_hits;
        let total = total_hits + l1_misses; // L1 miss is the only "real" miss if L2 hits
        let total_with_l2_miss = total_hits + l1_misses.saturating_sub(l2_hits) + l2_misses;

        let hit_rate = if total > 0 {
            (total_hits as f64 / total_with_l2_miss.max(1) as f64) * 100.0
        } else {
            0.0
        };

        let byte_usage: u64 = self.l1.iter().map(|e| e.value().byte_size as u64).sum();

        CacheMetrics {
            l1_hits,
            l1_misses,
            l2_hits,
            l2_misses,
            hit_rate,
            miss_rate: 100.0 - hit_rate,
            eviction_count: self.evictions.load(Ordering::Relaxed),
            byte_usage,
            l1_size: self.l1.len(),
            invalidation_count: self.invalidations.load(Ordering::Relaxed),
        }
    }

    // ── Internal ───────────────────────────────────────────────────

    /// Evict the least-recently-used entry from L1.
    fn evict_lru(&self) {
        let mut oldest_key: Option<u64> = None;
        let mut oldest_time = Instant::now();
        let mut lowest_hits = u64::MAX;

        // Hybrid LRU/LFU: prefer evicting entries that are both old AND infrequently accessed.
        for entry in self.l1.iter() {
            let e = entry.value();
            // Score: lower is more evictable.
            // Old + low hits = most evictable.
            if e.hit_count < lowest_hits
                || (e.hit_count == lowest_hits && e.created_at < oldest_time)
            {
                oldest_time = e.created_at;
                lowest_hits = e.hit_count;
                oldest_key = Some(*entry.key());
            }
        }

        if let Some(key) = oldest_key {
            self.l1.remove(&key);
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cache(max: usize) -> MultiTierCache<NoOpL2> {
        MultiTierCache::new(max, Duration::from_secs(60), true)
    }

    #[tokio::test]
    async fn basic_set_and_get() {
        let cache = make_cache(100);
        let key = CacheKey::from_raw(1);
        let data = serde_json::json!({"users": [{"id": 1}]});

        cache.set(&key, data.clone(), "User".into(), vec![]).await;
        let result = cache.get(&key).await;

        assert_eq!(result, Some(data));
    }

    #[tokio::test]
    async fn miss_returns_none() {
        let cache = make_cache(100);
        let key = CacheKey::from_raw(999);
        assert!(cache.get(&key).await.is_none());
    }

    #[tokio::test]
    async fn ttl_expiry() {
        let cache = MultiTierCache::new(100, Duration::from_millis(1), true);
        let key = CacheKey::from_raw(1);
        cache.set(&key, serde_json::json!({}), "X".into(), vec![]).await;

        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(cache.get(&key).await.is_none());
    }

    #[tokio::test]
    async fn smart_invalidation_by_type_and_attribute() {
        let cache = make_cache(100);

        // Query filtering on "email" attribute of User.
        let k1 = CacheKey::from_raw(1);
        cache
            .set(&k1, serde_json::json!("r1"), "User".into(), vec!["email".into()])
            .await;

        // Query filtering on "name" attribute of User.
        let k2 = CacheKey::from_raw(2);
        cache
            .set(&k2, serde_json::json!("r2"), "User".into(), vec!["name".into()])
            .await;

        // Query on Post entity.
        let k3 = CacheKey::from_raw(3);
        cache
            .set(&k3, serde_json::json!("r3"), "Post".into(), vec!["title".into()])
            .await;

        // Invalidate User.email changes — should only remove k1.
        cache.invalidate("User", Some("email")).await;

        assert!(cache.get(&k1).await.is_none(), "k1 should be invalidated");
        assert!(cache.get(&k2).await.is_some(), "k2 should survive (different attr)");
        assert!(cache.get(&k3).await.is_some(), "k3 should survive (different type)");
    }

    #[tokio::test]
    async fn invalidation_without_attribute_clears_all_for_type() {
        let cache = make_cache(100);

        let k1 = CacheKey::from_raw(1);
        cache.set(&k1, serde_json::json!("r1"), "User".into(), vec!["email".into()]).await;

        let k2 = CacheKey::from_raw(2);
        cache.set(&k2, serde_json::json!("r2"), "User".into(), vec!["name".into()]).await;

        let k3 = CacheKey::from_raw(3);
        cache.set(&k3, serde_json::json!("r3"), "Post".into(), vec![]).await;

        cache.invalidate("User", None).await;

        assert!(cache.get(&k1).await.is_none());
        assert!(cache.get(&k2).await.is_none());
        assert!(cache.get(&k3).await.is_some(), "Post should survive");
    }

    #[tokio::test]
    async fn unfiltered_query_invalidated_by_any_attribute() {
        let cache = make_cache(100);

        // A query with no specific attribute filters (e.g. SELECT * FROM User).
        let k1 = CacheKey::from_raw(1);
        cache.set(&k1, serde_json::json!("r1"), "User".into(), vec![]).await;

        // Even a specific attribute invalidation should clear it.
        cache.invalidate("User", Some("email")).await;
        assert!(cache.get(&k1).await.is_none());
    }

    #[tokio::test]
    async fn invalidate_all_clears_everything() {
        let cache = make_cache(100);

        for i in 0..10u64 {
            let key = CacheKey::from_raw(i);
            cache.set(&key, serde_json::json!(i), "X".into(), vec![]).await;
        }

        cache.invalidate_all().await;
        assert_eq!(cache.l1.len(), 0);
    }

    #[tokio::test]
    async fn lru_eviction_at_capacity() {
        let cache = make_cache(3);

        let k1 = CacheKey::from_raw(1);
        let k2 = CacheKey::from_raw(2);
        let k3 = CacheKey::from_raw(3);
        let k4 = CacheKey::from_raw(4);

        cache.set(&k1, serde_json::json!("a"), "X".into(), vec![]).await;
        cache.set(&k2, serde_json::json!("b"), "X".into(), vec![]).await;
        cache.set(&k3, serde_json::json!("c"), "X".into(), vec![]).await;

        // Access k1 to bump its hit count.
        let _ = cache.get(&k1).await;

        // Insert k4 — should evict the least-used entry.
        cache.set(&k4, serde_json::json!("d"), "X".into(), vec![]).await;

        assert_eq!(cache.l1.len(), 3);
        assert!(cache.get(&k1).await.is_some(), "k1 was recently accessed");
        assert!(cache.get(&k4).await.is_some(), "k4 was just inserted");
    }

    #[tokio::test]
    async fn disabled_cache_is_noop() {
        let cache = MultiTierCache::new(100, Duration::from_secs(60), false);
        let key = CacheKey::from_raw(1);
        cache.set(&key, serde_json::json!("x"), "Y".into(), vec![]).await;
        assert!(cache.get(&key).await.is_none());
    }

    #[test]
    fn cache_key_deterministic() {
        let uid = Uuid::new_v4();
        let query = serde_json::json!({"type": "User"});

        let k1 = CacheKey::new(Some(&uid), &query);
        let k2 = CacheKey::new(Some(&uid), &query);
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_user() {
        let query = serde_json::json!({"type": "User"});
        let k1 = CacheKey::new(Some(&Uuid::new_v4()), &query);
        let k2 = CacheKey::new(Some(&Uuid::new_v4()), &query);
        assert_ne!(k1, k2);
    }

    #[test]
    fn cache_key_differs_by_query() {
        let uid = Uuid::new_v4();
        let k1 = CacheKey::new(Some(&uid), &serde_json::json!({"type": "User"}));
        let k2 = CacheKey::new(Some(&uid), &serde_json::json!({"type": "Post"}));
        assert_ne!(k1, k2);
    }

    #[tokio::test]
    async fn metrics_tracking() {
        let cache = make_cache(100);
        let key = CacheKey::from_raw(1);

        // 2 misses.
        cache.get(&key).await;
        cache.get(&key).await;

        // 1 hit.
        cache.set(&key, serde_json::json!("v"), "X".into(), vec![]).await;
        cache.get(&key).await;

        let m = cache.metrics();
        assert_eq!(m.l1_hits, 1);
        assert_eq!(m.l1_misses, 2);
        assert!(m.hit_rate > 0.0);
        assert!(m.byte_usage > 0);
        assert_eq!(m.l1_size, 1);
    }

    #[test]
    fn cache_warming() {
        let cache = make_cache(100);
        let entries = vec![
            (CacheKey::from_raw(1), serde_json::json!("a"), "User".to_string()),
            (CacheKey::from_raw(2), serde_json::json!("b"), "Post".to_string()),
            (CacheKey::from_raw(3), serde_json::json!("c"), "User".to_string()),
        ];

        cache.warm(entries);
        assert_eq!(cache.l1.len(), 3);
    }
}
