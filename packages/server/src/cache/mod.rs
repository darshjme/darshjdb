//! Redis-inspired in-memory hot cache for DarshanDB query results.
//!
//! Provides sub-millisecond reads by serving repeated DarshanQL queries
//! from memory, bypassing Postgres entirely on cache hits. The cache is
//! invalidated reactively via the [`ChangeEvent`] broadcast channel
//! whenever mutations touch relevant entity types.
//!
//! # Design
//!
//! - **DashMap** for lock-free concurrent reads (no global mutex).
//! - **TTL + LRU eviction** to bound memory usage.
//! - **Entity-type keyed invalidation** so writes only flush affected queries.
//! - **Configurable** via `DARSHAN_CACHE_*` environment variables.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Cache entry
// ---------------------------------------------------------------------------

/// A single cached query result with metadata for TTL and LRU eviction.
struct CacheEntry {
    /// The cached JSON response data.
    data: Value,
    /// When this entry was inserted.
    created_at: Instant,
    /// Last time this entry was read (for LRU).
    last_accessed: Instant,
    /// Time-to-live for this entry.
    ttl: Duration,
    /// Number of cache hits on this entry.
    hit_count: u64,
    /// Transaction ID when this entry was cached (stale-detection).
    tx_id: i64,
    /// Entity type this query targets (for selective invalidation).
    entity_type: String,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

// ---------------------------------------------------------------------------
// Cache stats
// ---------------------------------------------------------------------------

/// Statistics exposed via the admin endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    /// Current number of entries in the cache.
    pub size: u64,
    /// Maximum allowed entries.
    pub max_entries: usize,
    /// Total cache hits since startup.
    pub hits: u64,
    /// Total cache misses since startup.
    pub misses: u64,
    /// Hit rate as a percentage (0.0 - 100.0).
    pub hit_rate: f64,
    /// Total invalidations performed.
    pub invalidations: u64,
    /// Total evictions due to capacity.
    pub evictions: u64,
    /// Whether the cache is enabled.
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// QueryCache
// ---------------------------------------------------------------------------

/// Thread-safe, lock-free in-memory cache for query results.
///
/// Uses `DashMap` (sharded concurrent hash map) for reads that
/// never block writers, achieving throughput comparable to Redis
/// for hot-path lookups.
pub struct QueryCache {
    entries: DashMap<u64, CacheEntry>,
    max_entries: usize,
    default_ttl: Duration,
    enabled: bool,

    // Atomic counters for stats (no locking on the hot path).
    hits: AtomicU64,
    misses: AtomicU64,
    invalidations: AtomicU64,
    evictions: AtomicU64,
}

impl QueryCache {
    /// Create a new query cache.
    ///
    /// - `max_entries`: upper bound on cached queries (LRU eviction above this).
    /// - `default_ttl`: how long entries live before expiring.
    /// - `enabled`: master switch (when false, get/set are no-ops).
    pub fn new(max_entries: usize, default_ttl: Duration, enabled: bool) -> Self {
        Self {
            entries: DashMap::with_capacity(max_entries),
            max_entries,
            default_ttl,
            enabled,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            invalidations: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Build a cache from `DARSHAN_CACHE_*` environment variables.
    ///
    /// | Variable                | Default | Description              |
    /// |-------------------------|---------|--------------------------|
    /// | `DARSHAN_CACHE_SIZE`    | 1000    | Max cached entries       |
    /// | `DARSHAN_CACHE_TTL`     | 60      | TTL in seconds           |
    /// | `DARSHAN_CACHE_ENABLED` | true    | Master on/off switch     |
    pub fn from_env() -> Self {
        let max_entries: usize = std::env::var("DARSHAN_CACHE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000);

        let ttl_secs: u64 = std::env::var("DARSHAN_CACHE_TTL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);

        let enabled: bool = std::env::var("DARSHAN_CACHE_ENABLED")
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true);

        tracing::info!(
            max_entries,
            ttl_secs,
            enabled,
            "query cache initialized from environment"
        );

        Self::new(max_entries, Duration::from_secs(ttl_secs), enabled)
    }

    // ── Public API ─────────────────────────────────────────────────

    /// Look up a cached result by query hash.
    ///
    /// Returns `Some(data)` on hit, `None` on miss or expiry.
    /// Automatically evicts expired entries on access.
    pub fn get(&self, query_hash: u64) -> Option<Value> {
        if !self.enabled {
            return None;
        }

        match self.entries.get_mut(&query_hash) {
            Some(mut entry) => {
                if entry.is_expired() {
                    drop(entry);
                    self.entries.remove(&query_hash);
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    entry.hit_count += 1;
                    entry.last_accessed = Instant::now();
                    let data = entry.data.clone();
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    Some(data)
                }
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Insert a query result into the cache.
    ///
    /// If the cache is at capacity, the least-recently-used entry
    /// is evicted first.
    pub fn set(&self, query_hash: u64, data: Value, tx_id: i64, entity_type: String) {
        if !self.enabled {
            return;
        }

        // Evict if at capacity.
        if self.entries.len() >= self.max_entries {
            self.evict_lru();
        }

        let now = Instant::now();
        self.entries.insert(
            query_hash,
            CacheEntry {
                data,
                created_at: now,
                last_accessed: now,
                ttl: self.default_ttl,
                hit_count: 0,
                tx_id,
                entity_type,
            },
        );
    }

    /// Invalidate all cached queries targeting a specific entity type.
    ///
    /// Called when a mutation (create/update/delete) touches entities
    /// of this type. Only flushes relevant entries, not the whole cache.
    pub fn invalidate_by_entity_type(&self, entity_type: &str) {
        let keys_to_remove: Vec<u64> = self
            .entries
            .iter()
            .filter(|entry| entry.value().entity_type == entity_type)
            .map(|entry| *entry.key())
            .collect();

        let count = keys_to_remove.len() as u64;
        for key in keys_to_remove {
            self.entries.remove(&key);
        }

        if count > 0 {
            self.invalidations.fetch_add(count, Ordering::Relaxed);
            tracing::debug!(
                entity_type,
                count,
                "invalidated cache entries for entity type"
            );
        }
    }

    /// Flush the entire cache. Used for admin operations or schema changes.
    pub fn invalidate_all(&self) {
        let count = self.entries.len() as u64;
        self.entries.clear();
        self.invalidations.fetch_add(count, Ordering::Relaxed);
        tracing::info!(count, "invalidated all cache entries");
    }

    /// Return current cache statistics.
    pub fn stats(&self) -> CacheStats {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 {
            (hits as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        CacheStats {
            size: self.entries.len() as u64,
            max_entries: self.max_entries,
            hits,
            misses,
            hit_rate,
            invalidations: self.invalidations.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            enabled: self.enabled,
        }
    }

    // ── Internal ───────────────────────────────────────────────────

    /// Evict the least-recently-used entry to make room.
    ///
    /// Scans all entries and removes the one with the oldest
    /// `last_accessed` timestamp. For caches up to ~10K entries
    /// this is fast enough; larger deployments would use an
    /// intrusive linked list (like Redis's approximated LRU).
    fn evict_lru(&self) {
        let mut oldest_key: Option<u64> = None;
        let mut oldest_time = Instant::now();

        for entry in self.entries.iter() {
            if entry.value().last_accessed < oldest_time {
                oldest_time = entry.value().last_accessed;
                oldest_key = Some(*entry.key());
            }
        }

        if let Some(key) = oldest_key {
            self.entries.remove(&key);
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }
}

// ---------------------------------------------------------------------------
// Query hashing
// ---------------------------------------------------------------------------

/// Compute a deterministic hash for a full query (including parameter values).
///
/// Unlike [`PlanCache::shape_key`] which hashes only the query *shape*,
/// this hashes the entire serialized query so that different filter
/// values produce different cache keys.
pub fn hash_query(query_json: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    // Canonical serialization: serde_json sorts object keys by default
    // when using Value, so identical queries produce identical strings.
    let canonical = query_json.to_string();
    canonical.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_set_and_get() {
        let cache = QueryCache::new(100, Duration::from_secs(60), true);
        let data = serde_json::json!({"users": [{"id": 1}]});
        let hash = hash_query(&serde_json::json!({"type": "User"}));

        cache.set(hash, data.clone(), 1, "User".into());
        let result = cache.get(hash);

        assert!(result.is_some());
        assert_eq!(result.unwrap(), data);

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 0);
        assert_eq!(stats.size, 1);
    }

    #[test]
    fn miss_returns_none() {
        let cache = QueryCache::new(100, Duration::from_secs(60), true);
        let result = cache.get(12345);

        assert!(result.is_none());

        let stats = cache.stats();
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn ttl_expiry() {
        let cache = QueryCache::new(100, Duration::from_millis(1), true);
        let hash = hash_query(&serde_json::json!({"type": "User"}));
        cache.set(hash, serde_json::json!({}), 1, "User".into());

        // Sleep just past the TTL.
        std::thread::sleep(Duration::from_millis(5));

        let result = cache.get(hash);
        assert!(result.is_none(), "expired entry should return None");
    }

    #[test]
    fn invalidate_by_entity_type() {
        let cache = QueryCache::new(100, Duration::from_secs(60), true);

        let h1 = hash_query(&serde_json::json!({"type": "User", "id": 1}));
        let h2 = hash_query(&serde_json::json!({"type": "User", "id": 2}));
        let h3 = hash_query(&serde_json::json!({"type": "Post", "id": 1}));

        cache.set(h1, serde_json::json!({}), 1, "User".into());
        cache.set(h2, serde_json::json!({}), 1, "User".into());
        cache.set(h3, serde_json::json!({}), 1, "Post".into());

        assert_eq!(cache.entries.len(), 3);

        cache.invalidate_by_entity_type("User");

        assert_eq!(cache.entries.len(), 1);
        assert!(cache.get(h1).is_none());
        assert!(cache.get(h2).is_none());
        assert!(cache.get(h3).is_some());
    }

    #[test]
    fn invalidate_all() {
        let cache = QueryCache::new(100, Duration::from_secs(60), true);
        for i in 0..10u64 {
            cache.set(i, serde_json::json!({}), 1, "X".into());
        }
        assert_eq!(cache.entries.len(), 10);

        cache.invalidate_all();

        assert_eq!(cache.entries.len(), 0);
        assert_eq!(cache.stats().invalidations, 10);
    }

    #[test]
    fn lru_eviction_at_capacity() {
        let cache = QueryCache::new(3, Duration::from_secs(60), true);

        cache.set(1, serde_json::json!("a"), 1, "X".into());
        std::thread::sleep(Duration::from_millis(2));
        cache.set(2, serde_json::json!("b"), 1, "X".into());
        std::thread::sleep(Duration::from_millis(2));
        cache.set(3, serde_json::json!("c"), 1, "X".into());

        // Access entry 1 to make it recently used.
        let _ = cache.get(1);
        std::thread::sleep(Duration::from_millis(2));

        // Insert a 4th — should evict entry 2 (least recently accessed).
        cache.set(4, serde_json::json!("d"), 1, "X".into());

        assert_eq!(cache.entries.len(), 3);
        assert!(cache.get(1).is_some(), "entry 1 was accessed recently");
        assert!(cache.get(2).is_none(), "entry 2 should have been evicted");
        assert!(cache.get(3).is_some());
        assert!(cache.get(4).is_some());
    }

    #[test]
    fn disabled_cache_is_noop() {
        let cache = QueryCache::new(100, Duration::from_secs(60), false);
        cache.set(1, serde_json::json!("x"), 1, "Y".into());
        assert!(cache.get(1).is_none());
        assert_eq!(cache.entries.len(), 0);
    }

    #[test]
    fn hash_determinism() {
        let q = serde_json::json!({"type": "User", "$where": [{"attribute": "email", "op": "Eq", "value": "a@b.com"}]});
        let h1 = hash_query(&q);
        let h2 = hash_query(&q);
        assert_eq!(h1, h2, "same query must produce same hash");
    }

    #[test]
    fn different_queries_different_hashes() {
        let q1 = serde_json::json!({"type": "User"});
        let q2 = serde_json::json!({"type": "Post"});
        assert_ne!(hash_query(&q1), hash_query(&q2));
    }

    #[test]
    fn stats_hit_rate_calculation() {
        let cache = QueryCache::new(100, Duration::from_secs(60), true);

        // 3 misses.
        cache.get(1);
        cache.get(2);
        cache.get(3);

        // 1 hit setup.
        cache.set(10, serde_json::json!("v"), 1, "X".into());
        cache.get(10); // hit

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 3);
        assert!((stats.hit_rate - 25.0).abs() < 0.01);
    }
}
