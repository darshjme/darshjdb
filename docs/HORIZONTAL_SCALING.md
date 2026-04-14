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

---

# Part II — Cluster Module Reference (v0.3.1 code-level)

The sections below document the `ddb_server::cluster` module that ships
in v0.3.1: advisory-lock leader election, singleton task supervisor,
the `LISTEN ddb_changes` fanout listener with auto-reconnect, and the
`/cluster/status` endpoint. Part I above covers the external HA stack
(Patroni, pgBouncer, WAL-G, HAProxy). Part II covers what DDB itself
does to be horizontally scalable.


Author: Darshankumar Joshi
Status: shippable — v0.3.1 active-passive background tasks + active-active HTTP.

---

## TL;DR

DarshJDB v0.3.1 can run as multiple `ddb-server` replicas behind a single
load balancer, sharing one Postgres. HTTP traffic is served active-active
on every replica. Background singleton tasks (TTL expiry sweeper,
anchor writer, embedding worker, …) are **active-passive** via
Postgres `pg_try_advisory_lock` — exactly one replica at a time runs each
task, with automatic failover if the leader dies.

This is **not** true partitioned horizontal scaling. The write path still
funnels through one shared Postgres. What it gives you is:

1. HTTP throughput that scales with replica count (read-heavy workloads).
2. Background-task correctness across N replicas (no duplicate anchor
   writes, no duplicate TTL retractions, no duplicate embedding work).
3. Cross-replica WebSocket delivery via Postgres `LISTEN/NOTIFY`.
4. Zero coordination services — no etcd, no Consul, no Zookeeper. Just
   Postgres.

True partitioned scaling (sharded triple store, gossip membership, Raft
log) is the v0.5 milestone. If you need it today, run a single replica.

---

## Topology

```
       ┌─────────────┐
clients│             │
──────▶│  load       │
       │  balancer   │
       │ (Traefik /  │
       │  nginx /    │
       │  ALB)       │
       └──────┬──────┘
              │
       ┌──────┼──────┐
       ▼      ▼      ▼
  ┌────────┐┌────────┐┌────────┐
  │ ddb-1  ││ ddb-2  ││ ddb-3  │   ← any number of replicas
  │        ││        ││        │
  │ HTTP   ││ HTTP   ││ HTTP   │   active-active
  │ WS     ││ WS     ││ WS     │   (fan-out via LISTEN/NOTIFY)
  │        ││        ││        │
  │ expiry ││ (idle) ││ (idle) │   active-passive
  │ anchor ││        ││        │   (advisory lock)
  └────┬───┘└────┬───┘└────┬───┘
       │         │         │
       └─────────┼─────────┘
                 ▼
       ┌──────────────────┐
       │  shared Postgres │
       │                  │
       │  + pgvector      │
       │  + TimescaleDB   │
       │  + (optional)    │
       │    read replica  │
       └──────────────────┘
```

Every replica:

* Accepts REST/WS traffic independently.
* Opens its own connection pool against the shared Postgres.
* Generates a process-lifetime random `node_id` (UUID v4) at startup.
* Runs its own `PgListener` task listening on the `ddb_changes`
  channel for cross-replica WebSocket fanout.
* Polls every singleton lock via `pg_try_advisory_lock`. Whichever
  replica wins a given lock runs that task for as long as it holds the
  lock session.

---

## Leader Election — Advisory Locks

### Primitives

The `cluster` module (`packages/server/src/cluster/`) exposes three
building blocks:

```rust
pub async fn try_acquire_leader(conn, lock_key) -> Result<bool>;
pub async fn release_leader(conn, lock_key) -> Result<()>;
pub fn spawn_singleton_task(pool, cluster_state, lock_key, tick, name, body) -> JoinHandle<()>;
```

`try_acquire_leader` wraps `SELECT pg_try_advisory_lock($1)` — a
**non-blocking** call. `spawn_singleton_task` spawns a Tokio task that
owns one dedicated Postgres connection for its entire lifetime. Every
`tick`, the task attempts the lock on its own session; if it wins, it
runs the body with the task's shared pool. If it loses, it sleeps until
the next tick.

### Lock table

| Task                     | Lock key constant                | Purpose                                          | Status       |
| ------------------------ | -------------------------------- | ------------------------------------------------ | ------------ |
| `expiry_sweeper`         | `LOCK_EXPIRY_SWEEPER`            | Retract TTL-expired triples every 30 s           | **active**   |
| `anchor_writer`          | `LOCK_ANCHOR_WRITER`             | Keccak batch roots → blockchain                  | reserved     |
| `embedding_worker`       | `LOCK_EMBEDDING_WORKER`          | Fill missing `memory_entries.embedding`          | reserved     |
| `memory_summariser`      | `LOCK_MEMORY_SUMMARISER`         | Roll hot-tier memory into warm                   | reserved     |
| `session_cleanup`        | `LOCK_SESSION_CLEANUP`           | Delete expired auth sessions                     | reserved     |
| `chunked_upload_cleanup` | `LOCK_CHUNKED_UPLOAD_CLEANUP`    | Purge orphaned `chunked_uploads` rows            | reserved     |

**Lock key format.** Every key is an `i64` whose upper 32 bits are the
ASCII signature `'D' 'D' 'B' \0` and whose lower 32 bits are a per-task
tag. That way any key printed from `pg_locks` is greppable back to
DarshJDB and collisions with unrelated advisory-lock users on a shared
Postgres are structurally impossible.

**Reserved locks** are keys that are already defined in the `cluster`
module but not yet wired into the server — they will go live as each
background task migrates onto `spawn_singleton_task` in subsequent
releases. Adding a new one is a one-line change to `cluster/mod.rs`.

### Failover semantics

* The lock is held for the lifetime of the leader task's dedicated
  Postgres **session**, not its transaction. Nothing else in DarshJDB
  uses the `DDB_*` advisory-lock prefix range.
* If the leader replica process exits (graceful shutdown, OOM kill,
  crash), its pooled connection dies, Postgres ends the session, and
  the advisory lock is released automatically. The next
  `try_acquire_leader` call from another replica returns `true`.
* If the leader's database connection is killed (e.g. Postgres restart,
  network partition) but the replica process stays alive, the task
  notices the next tick, drops the broken connection, re-acquires a
  fresh one from the pool, and re-races for leadership. No manual
  intervention.
* Failover latency = `tick` (30 s for the expiry sweeper). Tune `tick`
  down for faster failover, up for lower Postgres query load. The
  default values have been chosen to match each task's intrinsic
  cadence — there is no point polling the lock faster than the body
  runs.
* Advisory locks are **reentrant** on the same session:
  `pg_try_advisory_lock` returns `true` every time the same session
  calls it, so the leader keeps running the body every tick without
  stepping on itself. The `is_leader` debounce flag inside
  `spawn_singleton_task` ensures the `became leader` / `lost leadership`
  log lines only fire on transition.

---

## WebSocket fanout — LISTEN/NOTIFY

The triple-store write path emits
`pg_notify('ddb_changes', '{tx_id}:{entity_type}')` at every commit.
Each replica runs `cluster::notify_listener::spawn` on startup, which:

1. Opens a dedicated `PgListener` session (separate from the main pool
   because `LISTEN` connections can't return to a pool while listening).
2. `LISTEN`s on the `ddb_changes` channel.
3. Parses every incoming payload and re-broadcasts it through the
   replica's in-process `tokio::sync::broadcast::Sender<ChangeEvent>`.
4. Reconnects automatically on `recv()` errors.

Because every replica listens, a mutation committed on replica A
triggers NOTIFY on the shared Postgres, which replica B receives and
re-broadcasts into its local WebSocket subscribers. **Session affinity
is not required** — a WebSocket client connected to B sees writes made
through A's REST API.

The NOTIFY payload is intentionally minimal (`tx_id:entity_type`):
clients can't rely on it carrying the full entity diff. If they need
the diff, they follow up with a query against the triple store.

---

## Cluster Status Endpoint

```
GET /cluster/status
```

Returns JSON without requiring authentication (mounted at the top
level, next to `/health`):

```json
{
  "node_id": "1f8e9a7e-4c3a-4c1e-8e7f-5a3a2b1c0d9e",
  "uptime_secs": 3712,
  "leader_for": ["expiry_sweeper"],
  "version": "0.2.0"
}
```

* `node_id` — UUID generated at process startup; survives restarts as a
  fresh value, changes on every boot.
* `uptime_secs` — seconds since this process started.
* `leader_for` — which singleton tasks this replica currently holds the
  advisory lock for. Poll multiple replicas to build a cluster-wide
  view of who is doing what.
* `version` — `CARGO_PKG_VERSION` of the running binary.

Useful for: Prometheus textfile exporter, operator dashboards, smoke
tests that assert "at most one replica reports `anchor_writer` in
`leader_for`", and confirming a rolling deploy moved leadership off
the old replica before draining it.

---

## Deployment

### Minimum viable multi-replica setup

Same-host, three replicas behind Traefik or nginx:

```yaml
# docker-compose.yml (sketch)
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_DB: darshjdb
      POSTGRES_USER: ddb
      POSTGRES_PASSWORD: ddb

  ddb-1:
    image: darshjme/ddb-server:0.3.1
    environment:
      DATABASE_URL: postgres://ddb:ddb@postgres:5432/darshjdb
      DDB_PORT: "7700"

  ddb-2:
    image: darshjme/ddb-server:0.3.1
    environment:
      DATABASE_URL: postgres://ddb:ddb@postgres:5432/darshjdb
      DDB_PORT: "7700"

  ddb-3:
    image: darshjme/ddb-server:0.3.1
    environment:
      DATABASE_URL: postgres://ddb:ddb@postgres:5432/darshjdb
      DDB_PORT: "7700"

  traefik:
    image: traefik:v3
    command:
      - --providers.docker
      - --entrypoints.web.address=:80
    # route * to ddb-1,ddb-2,ddb-3 round-robin
```

Point clients at Traefik. Every replica serves the same REST API, every
replica accepts WebSockets, exactly one runs the TTL sweeper at any
given moment.

### Production setup

Two or three replicas on separate hosts with a managed Postgres
(Hetzner Cloud, AWS RDS, DigitalOcean Managed) and an external load
balancer with health checks hitting `/health/ready`. Ensure the
Postgres instance has connection headroom for the per-replica pool
size — default `DDB_DB_MAX_CONNECTIONS=20`, so three replicas need ~60
connections plus overhead.

### Health-check choreography

* `/health` — cheap liveness (process up).
* `/health/ready` — readiness (Postgres reachable). LBs should use this.
* `/cluster/status` — operator visibility into leadership distribution.

For blue/green deploys, drain traffic from the old replica first, then
watch `/cluster/status` on the new replicas to confirm leadership has
transferred before tearing down the old one.

---

## Limitations

1. **No cross-replica L1 cache coherency.** `QueryCache` is per-process.
   A mutation on replica A invalidates A's cache but not B's.
   Workarounds: (a) reduce `DDB_QUERY_CACHE_TTL`, (b) use the NOTIFY
   payload to drive cross-replica invalidation (not yet implemented —
   tracked for v0.4).
2. **Rate limiter state is per-process.** Each replica maintains its
   own token buckets. A client hitting three replicas in rotation sees
   3× their nominal rate limit. If strict global rate limiting matters,
   terminate at the load balancer.
3. **No session affinity required for auth.** JWTs are stateless and
   DB-validated (auth `sessions` table), so any replica can serve any
   request from any client.
4. **Write throughput is still bottlenecked by the single shared
   Postgres.** Adding replicas scales reads and background-task
   parallelism, not write throughput. For that, you need read replicas
   (trivial — point `DDB_RO_DATABASE_URL` at the replica) or v0.5's
   sharding story.
5. **No automatic leader fencing.** A paused replica (GC, stop-the-world
   debugger) can briefly believe it is still leader after Postgres has
   handed leadership over, because leadership is tied to session
   liveness, not a heartbeat. This window is bounded by Postgres' TCP
   keepalives (default 2 hours — tune `tcp_keepalives_idle` in
   `postgresql.conf` if this matters). In practice the singleton tasks
   in DarshJDB are idempotent over short windows (retract-expired,
   write-anchor, backfill-embedding) so the damage from a duplicate
   run is zero.
6. **`expiry_sweeper` is the only singleton wired in v0.3.1.** The
   other lock constants are reserved for tasks that don't exist in
   this branch yet (anchor writer, agent-memory embedding worker,
   chunked-upload cleanup). Each will adopt `spawn_singleton_task` as
   they land on top of this baseline.

---

## Reference

* Code: `packages/server/src/cluster/mod.rs`
* NOTIFY fanout: `packages/server/src/cluster/notify_listener.rs`
* Status endpoint: `packages/server/src/cluster/status.rs`
* Integration tests: `packages/server/tests/cluster_test.rs`
* Postgres advisory locks: <https://www.postgresql.org/docs/current/functions-admin.html#FUNCTIONS-ADVISORY-LOCKS>
* Postgres LISTEN/NOTIFY: <https://www.postgresql.org/docs/current/sql-listen.html>
