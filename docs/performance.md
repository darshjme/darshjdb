# Performance

DarshJDB is built for speed at every layer.

## Why DarshJDB Is Faster Than REST

| What REST Does | What DarshJDB Does | Improvement |
|---------------|---------------------|-------------|
| New TCP+TLS per request | Single persistent connection | **15x lower latency** |
| JSON text encoding | MsgPack binary encoding | **28% smaller payloads** |
| Full response on every poll | Delta-only patches on change | **98% less bandwidth** |
| Client polls for changes | Server pushes instantly | **Zero polling** |
| HTTP headers repeated | Zero header overhead | **No per-request tax** |

### Real-World Numbers

A typical app making 20 requests/second with 10 active subscriptions:

| Metric | REST | DarshJDB | Factor |
|--------|------|-----------|--------|
| Latency | ~248ms | ~1.2ms | **206x** |
| Bandwidth overhead | ~4,800 B/s | ~180 B/s | **26x** |

## Tuning Guide

### Connection Pool

```bash
# Default: 10 connections
DDB_PG_POOL_SIZE=20

# For high-concurrency servers
DDB_PG_POOL_SIZE=50

# Rule of thumb: 2x CPU cores for write-heavy, 4x CPU cores for read-heavy
```

#### When to Use PgBouncer

If you run multiple DarshJDB instances (e.g., 3 replicas in Kubernetes), each with a pool size of 20, that is 60 connections to PostgreSQL. PostgreSQL has a default `max_connections` of 100. Use PgBouncer to multiplex connections:

```bash
# PgBouncer config
[databases]
darshjdb = host=postgres port=5432 dbname=darshjdb

[pgbouncer]
pool_mode = transaction
max_client_conn = 200
default_pool_size = 20
```

### Query Complexity Limits

```bash
# Max depth of nested queries (default: 12)
DDB_MAX_QUERY_DEPTH=8

# Max entities per query result (default: 10000)
DDB_MAX_QUERY_RESULTS=5000
```

### Rate Limits

```bash
# Authenticated requests per minute (default: 100)
DDB_RATE_LIMIT_AUTH=200

# Anonymous requests per minute (default: 20)
DDB_RATE_LIMIT_ANON=10
```

### WebSocket Tuning

```bash
# Max concurrent connections per server (default: 10000)
DDB_MAX_CONNECTIONS=50000

# Send buffer size per client before backpressure (default: 1MB)
DDB_WS_BUFFER_SIZE=2097152
```

### Caching

DarshJDB caches query results in an LRU cache. Cached entries are invalidated automatically when underlying data changes.

```bash
# Query cache size (default: 1000 entries)
DDB_QUERY_CACHE_SIZE=5000

# Disable cache (useful for debugging)
DDB_QUERY_CACHE_ENABLED=false
```

## Capacity Planning

### Memory

| Component | Memory Usage |
|-----------|-------------|
| Base server process | ~50 MB |
| Per WebSocket connection | ~4 KB |
| Per active subscription | ~2 KB |
| Query cache (per entry) | ~1-50 KB (depends on result size) |
| V8 isolate (per function) | up to 128 MB (limit) |

**Formula:** `Base (50 MB) + Connections * 4 KB + Subscriptions * 2 KB + Cache * avg entry size`

**Example:** 10,000 connections, 20,000 subscriptions, 5,000 cache entries:
- 50 MB + 40 MB + 40 MB + 50 MB = ~180 MB
- Recommend: 512 MB with headroom for V8 functions

### CPU

| Operation | CPU Cost |
|-----------|---------|
| Query (cached) | ~0.01 ms |
| Query (uncached, simple) | ~1-5 ms |
| Query (complex, 3+ joins) | ~5-50 ms |
| Mutation (single entity) | ~2-5 ms |
| Mutation (batch, 100 ops) | ~20-50 ms |
| Subscription re-evaluation | ~0.5-2 ms |

**Rule of thumb:** 1 CPU core supports ~1,000 queries/second for simple queries. For complex workloads, benchmark with `ddb bench`.

### Disk (PostgreSQL)

| Data Point | Disk Usage |
|------------|-----------|
| Per triple (average) | ~200 bytes |
| Per entity (10 attributes) | ~2 KB |
| 1 million entities | ~2 GB |
| Indexes | ~30-50% of data size |

### Network

| Scenario | Bandwidth |
|----------|-----------|
| 1,000 idle WebSocket connections | ~30 KB/s (heartbeats) |
| 1,000 connections, 10 updates/sec | ~500 KB/s |
| 10,000 connections, 100 updates/sec | ~15 MB/s |

## Benchmarking

Run the built-in benchmark suite to measure your deployment's performance:

```bash
ddb bench --connections 100 --duration 30s --queries-per-sec 1000
```

This reports:
- P50, P95, P99 latency for queries and mutations
- Throughput (operations per second)
- WebSocket connection capacity
- Memory usage under load

### Example Output

```
DarshJDB Benchmark Results
===========================
Connections: 100 concurrent
Duration: 30 seconds
Target QPS: 1000

Queries:
  Total: 30,000
  P50: 0.8ms  P95: 2.1ms  P99: 4.5ms
  Throughput: 1,000 qps (target met)

Mutations:
  Total: 3,000
  P50: 2.3ms  P95: 5.8ms  P99: 12.1ms
  Throughput: 100 mps

WebSocket:
  Peak connections: 100
  Subscription delivery: 0.3ms avg
  Missed deliveries: 0

Memory:
  Peak RSS: 142 MB
  Heap: 89 MB
```

## PostgreSQL Tuning

For high-performance DarshJDB deployments, tune these PostgreSQL settings:

```ini
# postgresql.conf

# Memory
shared_buffers = 256MB           # 25% of available RAM
effective_cache_size = 768MB     # 75% of available RAM
work_mem = 16MB                  # Per-query sort/hash memory
maintenance_work_mem = 128MB     # For VACUUM, CREATE INDEX

# WAL
wal_buffers = 16MB
checkpoint_completion_target = 0.9
max_wal_size = 2GB

# Connections
max_connections = 200            # Match your total pool across all instances

# Planner
random_page_cost = 1.1           # For SSD storage
effective_io_concurrency = 200   # For SSD storage
```

## Production Tuning Checklist

- [ ] Set `DDB_PG_POOL_SIZE` appropriate for your hardware (2x CPU cores is a starting point)
- [ ] Enable PgBouncer for multi-instance deployments
- [ ] Set `DDB_MAX_QUERY_DEPTH` to the minimum your app requires
- [ ] Configure rate limits (`DDB_RATE_LIMIT_AUTH`, `DDB_RATE_LIMIT_ANON`)
- [ ] Monitor `/metrics` endpoint with Prometheus + Grafana
- [ ] Set up database backups (see [Self-Hosting](self-hosting.md))
- [ ] Enable `RUST_LOG=warn` in production (avoid `info` or `debug`)
- [ ] Tune PostgreSQL `shared_buffers` and `work_mem` for your hardware
- [ ] Set `DDB_QUERY_CACHE_SIZE` based on your query diversity
- [ ] Configure `DDB_MAX_CONNECTIONS` based on expected concurrent users
- [ ] Run `ddb bench` against your production hardware to establish baselines
- [ ] Set up alerting for P99 latency, connection pool exhaustion, and high error rates

---

[Previous: Security](security.md) | [Next: Migration Guide](migration.md) | [All Docs](README.md)
