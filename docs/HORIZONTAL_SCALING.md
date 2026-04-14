# Horizontal Scaling & HA — DarshJDB v0.3.1

DarshJDB is a single Rust binary that holds the API, query engine,
reactive tracker, embedder, and RESP3 cache server in-process. The
database of record is PostgreSQL 16 (with TimescaleDB + pgvector
preloaded). This document describes how to run DarshJDB at scale
with real high availability and zero single-points-of-failure.

> **Status**: v0.3.1 ships the topology, the compose file
> (`docker-compose.ha.yml`), the supporting HAProxy/Nginx/Prometheus
> configs, and the runbook below. Cross-backend portability
> (SQLite / in-memory / FoundationDB) is tracked via the `Store`
> trait at `packages/server/src/store/mod.rs` but is a v0.3.2+ effort
> — see [`STORAGE_BACKENDS.md`](STORAGE_BACKENDS.md).

---

## 1. Stateless multi-replica topology

```
                  ┌──────────────┐
 clients  ──────► │    ddb-lb    │  nginx + ip_hash (sticky WS)
                  └──────┬───────┘
          ┌──────────────┼──────────────┐
          ▼              ▼              ▼
   ┌───────────┐  ┌───────────┐  ┌───────────┐
   │ddb-server1│  │ddb-server2│  │ddb-server3│   (stateless)
   └─────┬─────┘  └─────┬─────┘  └─────┬─────┘
         └──────────────┼──────────────┘
                        ▼
                 ┌─────────────┐
                 │  pgbouncer  │   transaction pooling, 100/pool
                 └──────┬──────┘
                        ▼
                 ┌─────────────┐
                 │   haproxy   │   routes writes to Patroni leader
                 └──────┬──────┘
            ┌───────────┼───────────┐
            ▼           ▼           ▼
      ┌─────────┐ ┌──────────┐ ┌──────────┐
      │  pg-1   │ │   pg-2   │ │   pg-3   │  Patroni + Spilo
      │ LEADER  │ │ REPLICA  │ │ REPLICA  │  streaming replication
      └────┬────┘ └──────────┘ └──────────┘
           │
           ▼
      ┌─────────┐      ┌────────┐
      │  WAL-G  │────► │ MinIO  │   continuous WAL + nightly base
      └─────────┘      └────────┘   (swap MinIO for S3 / R2)

   etcd1 + etcd2 + etcd3  →  Patroni distributed consensus store
   prometheus + grafana   →  scraping /metrics on every replica
```

Each **DarshJDB server replica** is stateless: it boots against
pgBouncer, runs migrations idempotently (advisory lock 42), and
serves HTTP + WebSocket. You can kill any replica at any time.

---

## 2. Postgres HA

### Dev profile — single-node

```bash
docker compose up -d   # docker-compose.yml
```

Single `timescale/timescaledb-ha:pg16-latest` node, no replica, no
pgBouncer, no WAL archival. **Do not run this in production.** It is
labelled NOT FOR PRODUCTION in the compose banner.

### Production profile — Patroni + etcd + HAProxy + pgBouncer

```bash
docker compose -f docker-compose.ha.yml up -d
```

The HA profile runs:

| Component | Count | Purpose |
|---|---:|---|
| etcd | 3 | Patroni distributed consensus store |
| Patroni Spilo (Postgres 16) | 3 | 1 leader + 2 streaming replicas, automatic failover |
| HAProxy | 1 | Routes port 5432 → current leader; 5433 → any replica |
| pgBouncer | 1 | Transaction-mode connection multiplexer |
| WAL-G sidecar | 1 | Continuous WAL push + nightly basebackup |
| MinIO | 1 | S3-compatible WAL archive target (swap for real S3/R2) |
| DarshJDB replicas | 3 | Stateless Rust server |
| nginx (ddb-lb) | 1 | HTTP/WS front-door with `ip_hash` |
| Prometheus | 1 | `/metrics` scraping |
| Grafana | 1 | Dashboards |

**Failover**: Patroni's leader election is driven by etcd. On leader
crash, a replica is promoted within ~15 seconds; HAProxy's health
check (`GET /leader` against Patroni REST on port 8008) drains the
dead node and starts routing to the new leader. Applications see
connection reset errors for ~1 to ~30 seconds depending on the
failure mode. sqlx retries the next acquire automatically.

---

## 3. Backups — WAL-G setup + restore

WAL-G is configured in two places:

1. **Continuous archive**: Spilo runs WAL-G as Postgres's
   `archive_command`. Every WAL segment is pushed to
   `$WALG_S3_PREFIX/wal_005/` as soon as it rotates.
2. **Nightly basebackup**: the `walg-backup` sidecar container runs
   `wal-g backup-push /home/postgres/pgdata` against the leader at
   03:17 UTC daily. Basebackups go to `$WALG_S3_PREFIX/basebackups_005/`.

### Environment variables

```
WALG_S3_PREFIX         = s3://darshjdb-wal/cluster-prod
AWS_ACCESS_KEY_ID      = <minio root user or real AWS key>
AWS_SECRET_ACCESS_KEY  = <minio root pw or real AWS secret>
AWS_REGION             = us-east-1
AWS_ENDPOINT           = http://minio:9000        # omit for real S3
AWS_S3_FORCE_PATH_STYLE = true                    # MinIO requires path-style
```

### Restore runbook — point-in-time recovery

1. Stop writes at the load balancer (`docker compose -f docker-compose.ha.yml stop ddb-lb`).
2. Identify target LSN / timestamp from monitoring.
3. On a fresh volume, run:
   ```bash
   wal-g backup-fetch /home/postgres/pgdata LATEST
   ```
4. Write a `recovery.signal` + `postgresql.conf` override:
   ```
   restore_command = 'wal-g wal-fetch "%f" "%p"'
   recovery_target_time = '2026-04-14 12:34:56+00'
   recovery_target_action = 'promote'
   ```
5. Start Postgres standalone (bypassing Patroni). Verify the target
   time was reached.
6. Reinitialise the Patroni cluster with this recovered volume as the
   new primary (`patronictl reinit` on the replicas).
7. Restart the load balancer.

### Verify backups are real

```bash
docker exec -it darshandb-walg-backup-1 \
  wal-g backup-list --detail
```

You should see ≥1 basebackup from the last 24h and incremental WAL
segments with a `wal_segment_backup_start` recent within 5 minutes.

---

## 4. Connection pooling — pgBouncer

**Why pgBouncer?** SQLx's built-in pool is per-process. Three DarshJDB
replicas × `max_connections=40` = 120 direct Postgres connections,
which blows past the default `max_connections=100` and leaves no
headroom for Patroni's own probes, WAL senders, and admin tools.
pgBouncer multiplexes many application-side logical connections onto
a small number of real backend connections.

**Mode**: `transaction`. Application connections are returned to the
pool at `COMMIT`/`ROLLBACK`, not on disconnect. This is the highest
multiplexing ratio and is safe as long as the application does not
rely on session-level state (temp tables, `SET LOCAL` outside a tx,
`LISTEN`/`NOTIFY`).

> ⚠ **DarshJDB caveat**: the triple store uses `LISTEN ddb_changes`
> for cross-replica invalidation (see §6). `LISTEN` is session-level
> and is incompatible with transaction-mode pooling. The fix is:
> the server opens a **dedicated raw connection** for its listener
> that bypasses pgBouncer (connecting straight to `haproxy:5432`),
> separate from the main sqlx pool. See
> `packages/server/src/triple_store/mod.rs` where `pg_notify` is
> issued, and `packages/server/src/events/mod.rs` for the listener.
> Expose a separate `DDB_LISTEN_URL` if you need to change this.

### Tuning knobs

| Var | Default | Notes |
|---|---:|---|
| `MAX_CLIENT_CONN` | 2000 | Total client-side connections pgBouncer accepts |
| `DEFAULT_POOL_SIZE` | 100 | Backend connections per (user, db) pair |
| `MIN_POOL_SIZE` | 20 | Always keep this many warm |
| `RESERVE_POOL_SIZE` | 20 | Burst capacity when default pool exhausted |
| `RESERVE_POOL_TIMEOUT` | 3s | Wait before tapping reserve |
| `SERVER_RESET_QUERY` | `DISCARD ALL` | Cleans session state on release |

Monitor `pgbouncer:6432` via its admin console:
```bash
psql -h pgbouncer -p 6432 -U postgres pgbouncer -c 'SHOW POOLS;'
```

---

## 5. Cache server (RESP3 superset) — AUTH required

`ddb-cache-server` on port 7701 is a RESP3-compatible Redis superset
that DarshJDB uses for L2 cache + pub/sub. In the HA profile:

- `DARSH_CACHE_PASSWORD` is a **required** env var (`${VAR:?}` form);
  the server fails to start if unset.
- Clients must `AUTH <password>` on connect.
- TLS: terminate at the load balancer or use a sidecar (stunnel /
  nginx TCP stream mode). TLS on the RESP3 port directly is a v0.3.2
  milestone.

---

## 6. Cross-replica change fanout — `LISTEN ddb_changes`

DarshJDB has WebSocket subscriptions that stream live updates to
connected clients. In a multi-replica topology, a mutation that lands
on replica 2 must be delivered to subscribers attached to replicas 1
and 3. DarshJDB already solves this today via Postgres's native
pub/sub:

1. `PgTripleStore::set_triples` issues `NOTIFY ddb_changes, '<json>'`
   inside the write transaction (see
   `packages/server/src/triple_store/mod.rs`, function
   `set_triples_in_tx` — the NOTIFY is part of the same commit, so
   it's durable or it's gone).
2. Every replica runs a background task that does
   `LISTEN ddb_changes` on a dedicated raw connection and pushes
   events into the local reactive tracker + WebSocket broadcast bus.
3. The reactive tracker in `packages/server/src/events/mod.rs`
   re-runs affected live queries and fans the results to subscribed
   WebSocket sessions.

**Why it works across replicas**: `NOTIFY`/`LISTEN` is cluster-wide
in Postgres. Every connection, on any replica (including streaming
read replicas), receives the event. There is no application-level
gossip, no extra message broker, no Redis pub/sub dependency for
this path.

**Known caveats**:

- The L1 DashMap cache inside each replica is **not coherent** across
  replicas — it only holds read-mostly schema + query-plan entries,
  and is invalidated by the `ddb_changes` event stream. In the worst
  case you see a 50ms staleness window after a write.
- The **per-replica rate limiter** is also per-process — 3 replicas
  effectively multiply the configured per-IP rate limit by 3 unless
  you front them with a global rate limiter at the load balancer.
- A **WebSocket subscription** binds to the replica that accepted the
  HTTP upgrade. If that replica restarts, the client reconnects
  (usually to a different replica via `ip_hash` fall-through) and the
  server replays the subscription. Client state must be reapplied.

---

## 7. Checklist for going live

- [ ] `.env` has every `${VAR:?}` value set (the compose file will
  refuse to start otherwise).
- [ ] `DARSH_CACHE_PASSWORD` is ≥ 32 random bytes.
- [ ] `DDB_JWT_SECRET` is ≥ 32 random bytes.
- [ ] `MINIO_ROOT_PASSWORD` is rotated from any default.
- [ ] WAL-G `backup-list` shows ≥1 basebackup after first 24h.
- [ ] HAProxy stats UI at `:7000` shows 1 leader + 2 replicas UP.
- [ ] Grafana dashboard shows `/metrics` scraping from all 3
  DarshJDB replicas.
- [ ] `psql -h haproxy -p 5432 -U ddb darshjdb -c 'SELECT pg_is_in_recovery();'`
  returns `f` (false — it's the leader).
- [ ] `psql -h haproxy -p 5433 -U ddb darshjdb -c 'SELECT pg_is_in_recovery();'`
  returns `t` (true — replica).
- [ ] Kill the leader container (`docker kill patroni-pg-1`) and
  verify HAProxy routes to the new leader within 30s.
