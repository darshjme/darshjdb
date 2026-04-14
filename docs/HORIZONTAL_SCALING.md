# Horizontal Scaling — DarshJDB v0.3.1

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
