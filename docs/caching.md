# Caching System

DarshJDB uses a multi-tier caching architecture to serve repeated queries from memory, bypassing Postgres entirely on cache hits. The system provides sub-millisecond reads for hot-path lookups with smart invalidation that only flushes affected queries on writes.

## Architecture

```
Query ──> L1 (DashMap, in-process) ──hit──> Result
               |
               miss
               |
               v
          L2 (Redis, optional) ──hit──> Result (promoted to L1)
               |
               miss
               |
               v
          Postgres ──> Result (cached to L1 + L2)
```

## L1: In-Process DashMap

The L1 cache is a `DashMap` (sharded concurrent hash map) that provides lock-free reads with no global mutex. Reads never block writers, achieving throughput comparable to Redis for hot-path lookups.

- Bounded to a configurable number of entries (default: 1000)
- TTL-based expiry with lazy eviction on access
- Hybrid LRU/LFU eviction when at capacity: entries that are both old AND infrequently accessed are evicted first

## L2: Redis (Optional)

When configured, the L2 layer acts as a warm cache sitting between the in-process DashMap and Postgres. L2 is accessed only on L1 misses, and hits are promoted back to L1.

The L2 backend is abstracted behind a trait for testability:

```rust
pub trait L2Backend: Send + Sync + 'static {
    async fn get(&self, key: u64) -> Option<Value>;
    async fn set(&self, key: u64, value: &Value, ttl: Duration);
    async fn delete(&self, key: u64);
    async fn flush(&self);
    fn is_available(&self) -> bool;
}
```

When Redis is not configured, a no-op backend is used -- all L2 operations become zero-cost no-ops.

## Cache Keys

Cache keys are computed from the user context and query content:

```rust
let key = CacheKey::new(Some(&user_id), &query_json);
```

The key is a 64-bit hash derived from:
1. The user's UUID (or a zero byte for anonymous queries)
2. The canonical JSON serialization of the query

Different users querying the same data get different cache keys, ensuring per-user result isolation.

## Configuration

### Environment Variables (v2 Multi-Tier)

| Variable               | Default | Description                |
|------------------------|---------|----------------------------|
| `DDB_CACHE_V2_SIZE`   | `1000`  | Maximum L1 entries         |
| `DDB_CACHE_V2_TTL`    | `60`    | TTL in seconds             |
| `DDB_CACHE_V2_ENABLED`| `true`  | Master on/off switch       |

### Environment Variables (v1 Single-Tier)

| Variable             | Default | Description                  |
|----------------------|---------|------------------------------|
| `DDB_CACHE_SIZE`     | `1000`  | Maximum cached entries       |
| `DDB_CACHE_TTL`      | `60`    | TTL in seconds               |
| `DDB_CACHE_ENABLED`  | `true`  | Master on/off switch         |

### Programmatic Construction

```rust
// v2 with Redis L2
let cache = MultiTierCache::with_l2(
    1000,                              // max L1 entries
    Duration::from_secs(60),          // TTL
    true,                              // enabled
    Arc::new(redis_backend),          // L2 backend
);

// v2 without L2
let cache = MultiTierCache::new(1000, Duration::from_secs(60), true);

// v1 single-tier
let cache = QueryCache::new(1000, Duration::from_secs(60), true);

// From environment
let cache = MultiTierCache::from_env();
let cache = QueryCache::from_env();
```

## Smart Invalidation

The v2 cache tracks the entity type and filtered attributes for each cached query. On mutation, only queries that could be affected are invalidated.

### Attribute-Level Granularity

```rust
// Invalidate only queries that filter on User.email
cache.invalidate("User", Some("email")).await;

// Queries filtering on User.name are NOT invalidated
// Queries on other entity types are NOT invalidated
```

### Entity-Type Invalidation

```rust
// Invalidate all queries targeting the User entity
cache.invalidate("User", None).await;
```

### Rules

1. If a cached query filters on the mutated attribute, it is invalidated.
2. If a cached query has no attribute filters (e.g., `SELECT * FROM User`), it is invalidated by ANY attribute mutation on that entity type.
3. Queries targeting different entity types are never affected.

### v1 Invalidation

The v1 single-tier cache invalidates by entity type only:

```rust
cache.invalidate_by_entity_type("User");
```

### Full Flush

```rust
// v2
cache.invalidate_all().await;

// v1
cache.invalidate_all();
```

## Cache Warming

Pre-populate the cache at startup with results from the most frequently executed queries of the previous session.

```rust
let popular_queries = vec![
    (CacheKey::from_raw(1), json!({"users": [...]}), "User".to_string()),
    (CacheKey::from_raw(2), json!({"posts": [...]}), "Post".to_string()),
];

cache.warm(popular_queries);
```

## Eviction Strategy

### v2: Hybrid LRU/LFU

When the L1 cache reaches capacity, the entry with the lowest hit count is evicted. Ties are broken by age (oldest evicted first). This favors keeping both frequently accessed AND recently accessed entries.

### v1: Pure LRU

When the cache reaches capacity, the entry with the oldest `last_accessed` timestamp is evicted. Each cache read updates the `last_accessed` field.

For caches up to ~10K entries, the linear scan is fast enough. Larger deployments would benefit from an intrusive linked list (like Redis's approximated LRU).

## Metrics

### v2 Metrics

```rust
let m = cache.metrics();
```

| Field                | Type    | Description                              |
|----------------------|---------|------------------------------------------|
| `l1_hits`            | `u64`   | L1 cache hit count                       |
| `l1_misses`          | `u64`   | L1 cache miss count                      |
| `l2_hits`            | `u64`   | L2 (Redis) hit count                     |
| `l2_misses`          | `u64`   | L2 miss count                            |
| `hit_rate`           | `f64`   | Combined hit rate (0.0 - 100.0)          |
| `miss_rate`          | `f64`   | Combined miss rate (0.0 - 100.0)         |
| `eviction_count`     | `u64`   | Total evictions performed                |
| `byte_usage`         | `u64`   | Approximate total byte usage of L1       |
| `l1_size`            | `usize` | Current number of L1 entries             |
| `invalidation_count` | `u64`   | Total invalidations performed            |

### v1 Stats

```rust
let s = cache.stats();
```

| Field           | Type    | Description                          |
|-----------------|---------|--------------------------------------|
| `size`          | `u64`   | Current entry count                  |
| `max_entries`   | `usize` | Maximum allowed entries              |
| `hits`          | `u64`   | Total hits since startup             |
| `misses`        | `u64`   | Total misses since startup           |
| `hit_rate`      | `f64`   | Hit rate percentage (0.0 - 100.0)    |
| `invalidations` | `u64`   | Total invalidations                  |
| `evictions`     | `u64`   | Total capacity evictions             |
| `enabled`       | `bool`  | Whether the cache is active          |

All counters use `AtomicU64` for lock-free updates on the hot path.

## Query Hashing

The `hash_query` function produces a deterministic 64-bit hash from a `serde_json::Value`:

```rust
let hash = hash_query(&query_json);
```

It hashes the canonical JSON string representation. Since `serde_json::Value` sorts object keys by default, identical queries always produce identical hashes regardless of key insertion order.

## Disabled Cache

When the cache is disabled (via environment variable or constructor), all `get` and `set` calls are no-ops with zero overhead. The disabled state is checked via a boolean flag before any hash computation or map access.
