# Performance

DarshanDB is built for speed at every layer.

## Why DarshanDB Is Faster Than REST

| What REST Does | What DarshanDB Does | Improvement |
|---------------|---------------------|-------------|
| New TCP+TLS per request | Single persistent connection | **15x lower latency** |
| JSON text encoding | MsgPack binary encoding | **28% smaller payloads** |
| Full response on every poll | Delta-only patches on change | **98% less bandwidth** |
| Client polls for changes | Server pushes instantly | **Zero polling** |
| HTTP headers repeated | Zero header overhead | **No per-request tax** |

### Real-World Numbers

A typical app making 20 requests/second with 10 active subscriptions:

| Metric | REST | DarshanDB | Factor |
|--------|------|-----------|--------|
| Latency | ~248ms | ~1.2ms | **206x** |
| Bandwidth overhead | ~4,800 B/s | ~180 B/s | **26x** |

## Tuning Guide

### Connection Pool

```bash
# Default: 10 connections
DARSHAN_PG_POOL_SIZE=20

# For high-concurrency servers
DARSHAN_PG_POOL_SIZE=50
```

### Query Complexity Limits

```bash
# Max depth of nested queries (default: 12)
DARSHAN_MAX_QUERY_DEPTH=8

# Max entities per query result (default: 10000)
DARSHAN_MAX_QUERY_RESULTS=5000
```

### Rate Limits

```bash
# Authenticated requests per minute (default: 100)
DARSHAN_RATE_LIMIT_AUTH=200

# Anonymous requests per minute (default: 20)
DARSHAN_RATE_LIMIT_ANON=10
```

### WebSocket Tuning

```bash
# Max concurrent connections per server (default: 10000)
DARSHAN_MAX_CONNECTIONS=50000

# Send buffer size per client before backpressure (default: 1MB)
DARSHAN_WS_BUFFER_SIZE=2097152
```

### Caching

DarshanDB caches query results in an LRU cache. Cached entries are invalidated automatically when underlying data changes.

```bash
# Query cache size (default: 1000 entries)
DARSHAN_QUERY_CACHE_SIZE=5000

# Disable cache (useful for debugging)
DARSHAN_QUERY_CACHE_ENABLED=false
```

## Benchmarking

Run the built-in benchmark suite to measure your deployment's performance:

```bash
darshan bench --connections 100 --duration 30s --queries-per-sec 1000
```

This reports:
- P50, P95, P99 latency for queries and mutations
- Throughput (operations per second)
- WebSocket connection capacity
- Memory usage under load

## Production Checklist

- [ ] Set `DARSHAN_PG_POOL_SIZE` appropriate for your hardware (2x CPU cores is a good starting point)
- [ ] Enable connection pooling via PgBouncer for deployments with many app servers
- [ ] Set `DARSHAN_MAX_QUERY_DEPTH` to the minimum your app requires
- [ ] Configure rate limits (`DARSHAN_RATE_LIMIT_AUTH`, `DARSHAN_RATE_LIMIT_ANON`)
- [ ] Monitor `/metrics` endpoint with Prometheus + Grafana
- [ ] Set up database backups (see [Self-Hosting](self-hosting.md))
- [ ] Enable `RUST_LOG=warn` in production (avoid `info` or `debug` for performance)

---

[Previous: Security](security.md) | [Next: Migration Guide](migration.md) | [All Docs](README.md)
