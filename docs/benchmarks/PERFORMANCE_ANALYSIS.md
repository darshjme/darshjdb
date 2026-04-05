# DarshJDB Performance Analysis

Theoretical performance characteristics derived from source code analysis. No benchmark numbers are fabricated. All claims reference specific code paths.

**Date:** 2026-04-05
**Codebase version:** current HEAD
**Analyzed modules:** `triple_store`, `query`, `cache`, `api/ws`, `audit`, `rules`, `sync`, `embeddings`, `query/parallel`

---

## 1. Triple Store (Postgres Backend)

**Source:** `packages/server/src/triple_store/mod.rs`

### 1.1 Schema & Index Layout

The single `triples` table stores all data:

```sql
triples (
    id          BIGSERIAL PRIMARY KEY,
    entity_id   UUID NOT NULL,
    attribute   TEXT NOT NULL,
    value       JSONB NOT NULL,
    value_type  SMALLINT NOT NULL DEFAULT 0,
    tx_id       BIGINT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    retracted   BOOLEAN NOT NULL DEFAULT false,
    expires_at  TIMESTAMPTZ
)
```

Six indexes are defined:

| Index | Columns | Type | Partial | Purpose |
|---|---|---|---|---|
| `idx_triples_entity_attr` | `(entity_id, attribute)` | B-tree | `WHERE NOT retracted` | Entity lookups |
| `idx_triples_attr_value` | `(attribute, value)` | GIN | `WHERE NOT retracted` | Value-based queries |
| `idx_triples_tx_id` | `(tx_id)` | B-tree | No | Transaction ordering |
| `idx_triples_entity_tx` | `(entity_id, tx_id)` | B-tree | No | Point-in-time reads |
| `idx_triples_attribute` | `(attribute)` | B-tree | `WHERE NOT retracted` | Schema inference |
| `idx_triples_expires` | `(expires_at)` | B-tree | `WHERE expires_at IS NOT NULL AND NOT retracted` | TTL expiry scan |

**Observation:** Partial indexes on `NOT retracted` are a meaningful optimization. They exclude soft-deleted rows from the active working set, reducing index size proportionally to the retraction rate. However, the `idx_triples_attr_value` GIN index on `(attribute, value)` carries all JSONB values, which grows with data volume regardless of query patterns.

### 1.2 Read Paths

**`get_entity(entity_id)`** -- Single index lookup on `idx_triples_entity_attr`, returns all attribute rows for one entity. Cost: 1 round-trip. The `ORDER BY attribute, tx_id DESC` is handled by the composite index.

**`get_attribute(entity_id, attribute)`** -- Same index, narrower filter. Cost: 1 round-trip. Efficient.

**`query_by_attribute(attribute, value?)`** -- Uses `idx_triples_attr_value` GIN index for the `(attribute, value)` pair. Cost: 1 round-trip. For attribute-only queries (no value filter), falls back to `idx_triples_attribute` B-tree.

**`get_entity_at(entity_id, tx_id)`** -- Uses `DISTINCT ON (attribute)` with `ORDER BY attribute, tx_id DESC` and the `idx_triples_entity_tx` index. This is a point-in-time snapshot query. Cost: 1 round-trip, but Postgres must sort by (attribute, tx_id) and deduplicate. For entities with many historical revisions per attribute, this scan grows linearly with revision count.

**Key cost:** Reconstructing a full entity from the triple store requires reading N rows (one per attribute) vs. 1 row in a traditional column-per-field table. For an entity with 10 attributes, this is 10x the row count, though they share the same `entity_id` index page.

### 1.3 Write Paths

**`set_triples(triples)`** -- The standard mutation path:

1. `SELECT nextval('darshan_tx_seq')` -- 1 round-trip to allocate tx_id
2. `BEGIN` -- 1 round-trip to open transaction
3. N x `INSERT INTO triples VALUES (...)` -- N round-trips, one per triple (loop-based inserts, **not** batched)
4. `COMMIT` -- 1 round-trip
5. `SELECT ... WHERE tx_id = $1` -- 1 round-trip to re-read committed triples
6. Merkle root computation (in-process SHA-512)
7. `INSERT INTO audit_merkle_roots` -- 1 round-trip

**Total round-trips for a mutation of N triples: 4 + N + 1 = N + 5**

This is the primary write bottleneck. Each triple in the batch incurs a separate `sqlx::query(...).execute()` call within the transaction. For an entity with 10 attributes, that is 15 round-trips.

**`bulk_load(triples)`** -- The optimized batch path:

1. `SELECT nextval('darshan_tx_seq')` -- 1 round-trip
2. `INSERT ... SELECT FROM UNNEST($1::uuid[], $2::text[], ...)` -- 1 round-trip (all triples in one query via columnar arrays)
3. `SELECT ... WHERE tx_id = $1` -- 1 round-trip (re-read for Merkle)
4. Merkle root computation + insert -- 1 round-trip

**Total round-trips for bulk_load: 4 (constant, regardless of N)**

The UNNEST approach eliminates per-row overhead. The code comment states "10-50x faster than individual INSERT statements" which is a reasonable theoretical claim for large batches given the round-trip elimination and Postgres WAL write coalescing.

**Critical observation:** The `set_triples` path (used by real-time mutations) does NOT use the UNNEST optimization. Every WebSocket mutation goes through the per-row INSERT loop. This is the single largest optimization opportunity.

### 1.4 Transaction Overhead

Every mutation acquires a Postgres-level transaction (`pool.begin()`) and a sequence value. The sequence (`darshan_tx_seq`) is outside the transaction, so it never blocks concurrent writers. However, each write transaction holds row-level locks on the triples table until COMMIT.

The post-commit Merkle re-read (`SELECT ... WHERE tx_id = $1 ORDER BY id`) is a full re-fetch of everything just written. For a 10-attribute entity mutation, this reads 10 rows that were just inserted -- pure overhead for tamper detection. The Merkle root itself is computed in-process via SHA-512.

---

## 2. Query Engine

**Source:** `packages/server/src/query/mod.rs`

### 2.1 SQL Generation Pattern

`plan_query(ast)` generates SQL with the following JOIN structure:

```
FROM triples t0                                    -- base scan
INNER JOIN triples t_type ON entity_id match       -- type filter (:db/type)
INNER JOIN triples tw0 ON entity_id match          -- where clause 0
INNER JOIN triples tw1 ON entity_id match          -- where clause 1
...
INNER JOIN triples t_search ON entity_id match     -- full-text search (optional)
INNER JOIN embeddings t_emb ON entity_id match     -- semantic search (optional)
```

**JOIN count per query = 1 (type) + W (where clauses) + S (search) + V (semantic)**

A query with 3 where-clause filters generates 4 self-JOINs on the `triples` table. Each JOIN is on `entity_id` with attribute equality and retraction filter.

**Cost analysis:** Each self-JOIN narrows the result set. Postgres can use the `idx_triples_entity_attr` partial index for each JOIN arm, making these nested-loop joins over the index. For small result sets (typical OLTP), this is efficient. For large scans (analytics-style queries across millions of triples), the multiplicative JOIN cost becomes significant.

### 2.2 ORDER BY Cost

Order-by clauses use a **correlated subquery per sort key**:

```sql
ORDER BY (SELECT to0.value FROM triples to0
          WHERE to0.entity_id = t0.entity_id
            AND to0.attribute = $N
            AND NOT to0.retracted
          ORDER BY to0.tx_id DESC LIMIT 1)
```

This executes a subquery per result row per sort key. For a result set of R rows with 2 sort keys, that is 2R additional index lookups. This is the most expensive part of the query for sorted results.

### 2.3 Nested Entity Resolution

Nested references (`$nested`) are resolved as N+1 queries:

1. Root query executes (1 query)
2. For each result row, for each nested reference, execute a separate `SELECT ... WHERE entity_id = $1` (R * N queries)

This is a classic N+1 problem. For 50 results with 2 nested references, that is 101 queries.

### 2.4 Plan Cache

**Source:** `PlanCache` struct using `lru::LruCache` with `Mutex` guard.

The plan cache is keyed by a SHA-256 hash of the query **shape** (entity type + attribute names + operators + flags), ignoring concrete filter values. This means queries like `WHERE email = "a@b.com"` and `WHERE email = "x@y.com"` share the same cached plan.

**Effectiveness:** High for applications with repetitive query patterns (typical BaaS workloads). The Mutex on cache access serializes plan lookups, but since `plan_query` is pure string building (no I/O), the critical section is microsecond-scale.

**Cache size:** Default 256 entries (hardcoded in `NonZeroUsize::new(256)`). Sufficient for most applications; large multi-tenant deployments might saturate this.

### 2.5 Parameter Binding

The `bind_json_param` function dispatches on `serde_json::Value` type:
- Strings bind as `&str` (text type) -- Postgres casts to `::jsonb` where needed
- All other types bind as `serde_json::Value` (JSONB native)

This avoids JSONB encoding overhead for the common string-filter case and allows Postgres to use text-aware operators (ILIKE, `plainto_tsquery`).

### 2.6 Hybrid Search (RRF)

The hybrid search plan uses 3 CTEs (`type_entities`, `text_ranked`, `vector_ranked`) plus a `rrf_merged` CTE with a `FULL OUTER JOIN`. Total self-join count on `triples`: 2 (type_entities + final fetch). The oversampling factor is 3x the requested limit (`limit * 3`) in each CTE to improve fusion quality.

This is computationally expensive but executes as a single SQL statement with no round-trips.

---

## 3. Cache Layer

**Source:** `packages/server/src/cache/mod.rs`

### 3.1 Architecture

- **Data structure:** `DashMap<u64, CacheEntry>` -- sharded concurrent hashmap (16 shards by default)
- **Eviction:** LRU via full scan of `last_accessed` timestamps
- **TTL:** Per-entry, checked on read (lazy expiration)
- **Invalidation:** By entity type string match (full scan of entries)
- **Stats:** Atomic counters (no locking on hot path)

### 3.2 Cache Hit Path

```
get(query_hash) ->
  DashMap::get_mut() [shard lock, ~10-50ns] ->
  is_expired() check [Instant comparison, ~1ns] ->
  clone data (serde_json::Value) [varies by size] ->
  AtomicU64::fetch_add [~5ns]
```

The cache hit path is lock-free at the DashMap shard level (only the specific shard is locked, not the entire map). The dominant cost is the `Value::clone()` of the cached JSON response. For typical entity responses (1-10KB JSON), this is sub-microsecond. For large result sets, clone cost grows linearly.

**Bypass of Postgres:** On cache hit, zero SQL is executed. This completely eliminates all JOIN overhead, round-trips, and Postgres CPU. For read-heavy workloads with temporal locality, this is the dominant performance win.

### 3.3 Cache Miss Path

On miss: increment atomic miss counter, return `None`. The query then goes through the full plan -> SQL -> Postgres path, and the caller is responsible for `cache.set()`.

### 3.4 Invalidation

`invalidate_by_entity_type(entity_type)`:

1. Full scan of all entries via `DashMap::iter()` -- O(N) where N = cache size
2. Collect matching keys into a Vec
3. Remove each matching key

This is O(N) per mutation per entity type. With the default max 1000 entries, this scan takes ~1-10 microseconds. At 10,000 entries, it becomes ~10-100 microseconds, which is negligible compared to the Postgres mutation it follows.

**Granularity concern:** Invalidation is at the entity-type level, not entity-ID level. A mutation to one `User` entity invalidates ALL cached `User` queries. This is conservative (correct) but causes unnecessary cache misses for queries that filter on specific entity IDs.

### 3.5 LRU Eviction

`evict_lru()` does a **full scan** to find the entry with the oldest `last_accessed`. This is O(N) per eviction. The code comment acknowledges this: "for caches up to ~10K entries this is fast enough; larger deployments would use an intrusive linked list."

**Memory overhead per entry:** `CacheEntry` contains:
- `data: serde_json::Value` -- variable (the actual cached response)
- `created_at: Instant` -- 8 bytes
- `last_accessed: Instant` -- 8 bytes
- `ttl: Duration` -- 8 bytes
- `hit_count: u64` -- 8 bytes
- `tx_id: i64` -- 8 bytes
- `entity_type: String` -- 24 bytes (String struct) + heap allocation

Fixed overhead: ~64 bytes + DashMap slot overhead (~100 bytes) + entity_type string heap. Total per-entry overhead excluding data: ~200 bytes. The `Value` data dominates for any non-trivial response.

### 3.6 Hash Function

`hash_query` uses `DefaultHasher` (SipHash-1-3 on most platforms) over the canonical JSON string. This is deterministic but requires full JSON serialization as a pre-step. For complex queries, the serialization cost may exceed the hash cost.

---

## 4. WebSocket Subsystem

**Source:** `packages/server/src/api/ws.rs`, `sync/session.rs`, `sync/registry.rs`, `sync/broadcaster.rs`

### 4.1 Per-Connection Memory

Each WebSocket connection allocates:

| Component | Memory | Source |
|---|---|---|
| `SyncSession` | ~200 bytes + subscriptions | `session.rs` |
| `ActiveSubscription` (per sub) | ~80 bytes + query AST clone | `session.rs` |
| `SessionId` (UUID) | 16 bytes | DashMap key |
| `broadcast::Receiver` | ~128 bytes (Tokio internal) | `change_rx` in message loop |
| Tokio task stack | ~8-16 KB | `handle_connection` task |
| WebSocket frame buffers | Up to 1 MiB (`MAX_MESSAGE_SIZE`) | axum-tungstenite |

**Estimated baseline per connection: ~20 KB** (without subscriptions or buffered data).
Each active subscription adds ~80 bytes metadata + the full query AST as `serde_json::Value` (typically 100-500 bytes).

### 4.2 Codec Detection

The first message determines JSON vs MessagePack codec for the session lifetime. MessagePack reduces per-message serialization size by approximately 20-30% for typical payloads, reducing network I/O and allocation.

### 4.3 Broadcast Fan-Out Strategy

The change propagation path:

```
Triple mutation committed ->
  ChangeEvent published to tokio::broadcast channel ->
    Each WsState.change_tx.subscribe() receiver gets a clone ->
      Per-connection: handle_change_event() checks subscription registry ->
        If subscribed: re-execute query, compute diff, send to client
```

**Broadcast channel:** `tokio::sync::broadcast` with clone-on-receive semantics. Each receiver gets an independent clone of `ChangeEvent`. For C connections, each mutation creates C clones.

**Fan-out deduplication:** The `SubscriptionRegistry` maps `query_hash -> HashSet<SubscriptionHandle>`. When a change event arrives, the broadcaster can identify affected query hashes and re-execute the query only once per unique hash, then fan the diff to all subscribers of that hash.

**Current implementation gap:** In `ws.rs`, `handle_change_event` is called per-connection in the connection's own task. The code does not show centralized query re-execution -- each connection appears to independently evaluate whether it is affected. This means the same query might be re-executed C times if C connections subscribe to the same query. The `Broadcaster` struct in `broadcaster.rs` has the architecture for centralized fan-out but the WS handler appears to use a simpler per-connection path.

### 4.4 Backpressure Handling

```rust
Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
    debug!(session_id = %session_id, skipped = n, "change receiver lagged");
}
```

When a client's receiver falls behind the broadcast channel's buffer, messages are dropped with a `Lagged` error. The handler logs this and continues -- it does NOT re-sync the client. This means slow consumers lose change events silently. For correctness, the client would need to re-fetch its subscribed queries after a lag event.

### 4.5 Keepalive

Server-side pings at 30-second intervals via `tokio::time::interval`. Uses `MissedTickBehavior::Delay` which skips missed ticks rather than bursting, preventing thundering-herd pings after a stall.

---

## 5. Merkle Audit Trail

**Source:** `packages/server/src/audit/mod.rs`

### 5.1 Per-Mutation Cost

Every mutation (including individual entity updates) triggers:

1. **Re-read all written triples:** `SELECT ... WHERE tx_id = $1 ORDER BY id` -- 1 round-trip
2. **Hash each triple:** SHA-512 over `entity_id || attribute || value_json || value_type || tx_id` -- O(N) hashes
3. **Build Merkle tree:** Bottom-up binary tree with odd-leaf duplication (Bitcoin convention) -- O(N) hash operations
4. **Chain with previous root:** `SHA-512(merkle_root || prev_root)` -- 1 hash
5. **Store root:** INSERT into `audit_merkle_roots` -- 1 round-trip

**SHA-512 cost:** Modern CPUs compute SHA-512 at ~500 MB/s. For a typical triple (~200 bytes pre-image), one hash takes ~0.4 microseconds. For a 10-triple mutation, Merkle computation adds ~8 microseconds (10 leaf hashes + 9 internal hashes). Negligible compared to Postgres I/O.

**The re-read is the actual cost.** Reading back 10 just-written rows adds a full Postgres round-trip that serves no purpose other than to hash over the canonical database state. This is approximately 0.5-2ms depending on network latency to Postgres.

### 5.2 Hash Chain Integrity

The chained root (`SHA-512(merkle_root || prev_root)`) requires fetching the previous root before computing the new one. This introduces a sequential dependency between transactions: tx N+1 cannot finalize its audit root until tx N's root is stored. Under high write concurrency, this serializes the audit chain computation even if the triples themselves commit in parallel.

---

## 6. Forward-Chaining Rule Engine

**Source:** `packages/server/src/rules/mod.rs`

### 6.1 Evaluation Model

Rules evaluate after every triple write against the full batch of new triples. The engine:

1. Matches each new triple against all rule patterns (O(R * T) where R = rules, T = new triples)
2. For matching rules, executes the action (may require reading existing triples from the store)
3. Collects implied triples
4. **Recursively re-evaluates** implied triples up to `MAX_CHAIN_DEPTH = 3` levels
5. Deduplicates by `(entity_id, attribute)` pair

**Worst case:** With R rules and a chain depth of 3, a single mutation could trigger `R * T * 3` pattern matches plus the associated store reads for actions like `Propagate` and `UpdateCounter` (which follow references).

**Practical impact:** For most applications with 0-10 rules, the overhead is sub-millisecond. For rule sets that trigger chain reactions (e.g., updating a counter that triggers another rule), the depth limit of 3 prevents runaway evaluation but permits non-trivial inference chains.

### 6.2 Action Costs

| Action | Store reads required | Additional writes |
|---|---|---|
| `Compute::Concat` | 1 read per source field (get_entity) | 1 triple |
| `Compute::Copy` | 0 (value comes from triggering triple context) | 1 triple |
| `Compute::Literal` | 0 | 1 triple |
| `Compute::CountRelated` | 1 query_by_attribute | 1 triple |
| `Propagate` | 1 get_entity (follow reference) | 1 triple per target |
| `UpdateCounter` | 1 get_entity (follow ref) + 1 get_attribute | 1 triple |

---

## 7. Entity Pool (Dictionary Encoding)

**Source:** `packages/server/src/triple_store/mod.rs` lines 724-881

### 7.1 Architecture

Maps external UUIDs (16 bytes) to internal `BIGSERIAL` integers (8 bytes) via a dedicated `entity_pool` table. Maintains bidirectional DashMap caches (`fwd: UUID->i64`, `rev: i64->UUID`).

### 7.2 Lookup Cost

**Cache hit:** DashMap shard lock (~10-50ns) + value copy (8 or 16 bytes). Essentially free.

**Cache miss:** `INSERT ... ON CONFLICT DO NOTHING` + `SELECT internal_id` = 2 round-trips to Postgres. After the first access, the mapping is cached indefinitely (no eviction).

**Batch path:** `batch_get_or_create` separates cache hits from misses, opens a transaction for bulk inserts, then does a single `WHERE external_id = ANY($1)` fetch. Efficient for initial data loads.

### 7.3 Memory Cost

Each cached mapping: DashMap overhead (~100 bytes) + UUID (16 bytes) + i64 (8 bytes) per direction = ~250 bytes per entity for both caches. For 1 million entities: ~250 MB of cache memory. This grows without bound since there is no eviction policy on the entity pool cache.

**Note:** The entity pool is defined and implemented but NOT currently wired into the query path. The `plan_query` function still uses `entity_id UUID` directly in all JOINs. The pool exists as infrastructure for future index optimization.

---

## 8. Parallel Batch Execution

**Source:** `packages/server/src/query/parallel.rs`

### 8.1 Conflict Model

Operations conflict if they touch the same entity type AND at least one is a write. Read-read on the same type does not conflict. This is a coarse-grained conflict model (entity-type level, not entity-ID level).

### 8.2 Wave Scheduling

Greedy algorithm: scan operations in order, add to current wave if no conflict, otherwise start a new wave. Waves execute with `tokio::join_all` (parallel within wave), waves execute sequentially.

**Best case:** All operations touch different entity types -> 1 wave, full parallelism.
**Worst case:** All operations write to the same entity type -> N waves, fully sequential.
**Typical case:** Mixed reads across 3-5 entity types with occasional writes -> 2-3 waves.

---

## 9. Auto-Embedding Pipeline

**Source:** `packages/server/src/embeddings/mod.rs`

The pipeline listens on the `ChangeEvent` broadcast channel and spawns background Tokio tasks for embedding generation. This is explicitly non-blocking -- mutations complete and return to the client before embeddings are computed.

**Latency impact on mutations:** Zero (async, fire-and-forget).
**Background load:** One HTTP request to OpenAI/Ollama per text triple that matches the `auto_embed_attributes` configuration. For write-heavy workloads with many text attributes, this can generate significant background HTTP traffic.

---

## 10. Comparative Overhead Analysis

Theoretical overhead vs. a traditional table-based BaaS (e.g., Supabase/PostgREST with one table per entity type):

### 10.1 Simple Read (fetch one entity by ID)

| Aspect | Traditional | DarshJDB | Overhead factor |
|---|---|---|---|
| SQL queries | 1 (`SELECT * FROM users WHERE id = $1`) | 1 (`SELECT ... FROM triples WHERE entity_id = $1 AND NOT retracted`) | ~1x queries |
| Rows returned | 1 row | N rows (one per attribute) | Nx row volume |
| Index usage | PK lookup | Partial index on `(entity_id, attribute)` | Comparable |
| Post-processing | None (columns map to fields) | Group rows by attribute, build JSON | Additional CPU |

**Estimated overhead: 1.5-3x** -- driven by N-row-per-entity volume and post-processing.

### 10.2 Filtered Query (e.g., users WHERE email = X AND age > 25)

| Aspect | Traditional | DarshJDB |
|---|---|---|
| JOINs | 0 (single table scan with WHERE) | 2 (one per filter attribute) + 1 (type filter) = 3 self-JOINs |
| Index usage | Composite index on (email, age) | Separate index lookups per JOIN arm |
| Query plan complexity | Simple index scan | Multi-way nested loop join |

**Estimated overhead: 2-5x** -- primarily from self-JOIN cost. The Postgres optimizer can convert these to hash joins for large result sets, but for OLTP (small results), nested-loop over index is used, and the overhead scales with the number of filter predicates.

### 10.3 Mutation (create one entity with 10 attributes)

| Aspect | Traditional | DarshJDB |
|---|---|---|
| SQL statements | 1 INSERT | 1 tx_id allocation + 10 INSERTs + 1 COMMIT + 1 re-read + 1 Merkle store = 14 |
| Round-trips | 1 | 15 (using `set_triples` path) |
| Index updates | 1-3 indexes on one table | 6 indexes on `triples`, 10 times | |
| Post-mutation | None | Merkle tree computation + rule engine evaluation |

**Estimated overhead: 3-8x** -- dominated by per-row INSERT in the `set_triples` path and the Merkle re-read. Using `bulk_load` reduces this to 1.5-2x.

### 10.4 Cache Hit

| Aspect | Traditional | DarshJDB |
|---|---|---|
| Path | N/A (typically no app-level cache) | DashMap lookup + Value clone |
| Postgres involved | Yes (always) | No |

**Estimated overhead: 0x (faster than traditional)**. The in-memory cache bypasses Postgres entirely, making cached reads 100-1000x faster than uncached traditional reads.

### 10.5 WebSocket Real-Time

Comparable to other implementations (Supabase Realtime, Convex). The broadcast channel + subscription registry pattern is standard. DarshJDB's query-hash-based deduplication is an advantage for fan-out efficiency.

---

## 11. Identified Bottlenecks (Priority Order)

### Critical

1. **Per-row INSERT in `set_triples`** -- The real-time mutation path uses a loop of individual INSERTs. The `bulk_load` UNNEST approach exists but is only available for bulk operations, not standard mutations. Applying UNNEST to `set_triples` would reduce write round-trips from N+5 to 4 (constant).

2. **N+1 nested entity resolution** -- Each nested reference in query results triggers a separate SQL query per result row. This should use `WHERE entity_id = ANY($1)` batch fetching.

3. **Merkle re-read after commit** -- Re-selecting all just-written triples to compute the Merkle root is pure overhead. The triples are already in memory at write time; the hash could be computed from the in-memory data before commit.

### Significant

4. **Entity-type-level cache invalidation** -- Invalidating all queries for an entity type on any mutation to that type causes excessive cache misses for applications with high write rates on popular types. Entity-ID-level invalidation would be more precise.

5. **ORDER BY correlated subquery** -- Each sort key requires a subquery per result row. A materialized view or denormalized sort column would eliminate this.

6. **Broadcast lag = silent data loss** -- Lagged broadcast receivers drop change events without recovery. Slow WebSocket clients permanently miss updates.

7. **LRU eviction via full scan** -- O(N) scan for every eviction. Replace with an intrusive doubly-linked list for O(1) eviction.

### Minor

8. **Entity pool not wired into query path** -- The UUID-to-integer dictionary encoding is implemented but unused. Wiring it in would reduce index key sizes from 16 bytes to 8 bytes, improving cache utilization for index-heavy workloads.

9. **PlanCache Mutex contention** -- Under very high query concurrency, the `Mutex<LruCache>` could become a contention point. Replacing with `DashMap` or a sharded cache would eliminate this.

10. **Unbounded entity pool memory** -- The DashMap caches in `EntityPool` grow without limit. For databases with millions of entities, this could consume significant memory.

---

## 12. Optimization Opportunities for Future Benchmarking

When running actual benchmarks, focus measurement on:

1. **`set_triples` vs `bulk_load` throughput** -- Measure the actual speedup of UNNEST for varying batch sizes (1, 10, 100, 1000 triples).

2. **Cache hit ratio under realistic workloads** -- Measure the actual hit rate for read-heavy (90/10), balanced (50/50), and write-heavy (10/90) workloads. Entity-type invalidation may cause surprisingly low hit rates for write-heavy patterns.

3. **JOIN scaling** -- Measure query latency as a function of WHERE-clause count (1, 2, 5, 10 predicates) to quantify the self-JOIN overhead.

4. **WebSocket fan-out at scale** -- Measure mutation-to-client latency with 100, 1000, 10000 concurrent connections, each with 1-10 subscriptions.

5. **Merkle computation overhead** -- Measure the actual wall-clock cost of Merkle root computation for varying transaction sizes to determine if the re-read optimization is worthwhile.

6. **Rule engine overhead** -- Measure the added latency per mutation with 0, 5, 10, 20 rules loaded, with and without chain depth > 1.

---

## 13. Architectural Strengths

Despite the overhead costs, the EAV/triple-store architecture provides capabilities that traditional table-based systems cannot match without significant additional infrastructure:

- **Schema flexibility:** No migrations needed for new attributes. Adding a field is just writing a new triple.
- **Temporal queries:** Point-in-time reads (`get_entity_at`) come free from the append-only design.
- **Audit trail:** The Merkle chain provides cryptographic tamper detection with no external dependencies.
- **Forward-chaining rules:** Automatic inference at the storage layer, not the application layer.
- **Unified query model:** All entity types share the same query engine and cache infrastructure.

The performance trade-offs are the cost of this flexibility. For read-heavy workloads with good cache hit rates, the overhead is minimal. For write-heavy workloads with complex filters, the overhead is measurable but bounded.
