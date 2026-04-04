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
