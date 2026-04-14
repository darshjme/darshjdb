// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
// Horizontal-scaling cluster primitives (v0.3.1).
//
// This module implements the minimum coordination layer required to run
// multiple `ddb-server` replicas against a single shared Postgres:
//
//   1. **Advisory-lock leader election** — background tasks that must run
//      on exactly one replica at a time (anchor writer, embedding worker,
//      TTL expiry sweeper, chunked-upload cleanup) go through
//      [`spawn_singleton_task`] / [`spawn_singleton_supervisor`], which
//      wrap the task body with a `pg_try_advisory_lock` gate. The lock is
//      held for the lifetime of a dedicated Postgres connection — drop =
//      release — so crashes hand leadership over automatically.
//
//   2. **Node identity** — every process generates a random [`NodeId`] at
//      startup. `GET /cluster/status` exposes the node id, uptime, and the
//      set of singleton tasks this replica is currently leading, so
//      operators can verify which replica is running what.
//
// Lock keys are compile-time `i64` constants defined below so collisions
// between subsystems are structurally impossible.
//
// This is **active-passive for background tasks, active-active for HTTP
// traffic.** True horizontal scaling with partitioned storage is v0.5
// material; advisory-lock coordination is the shippable v0.3.1 story. See
// `docs/HORIZONTAL_SCALING.md` for the full deployment topology.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use uuid::Uuid;

pub mod notify_listener;
pub mod status;

// -----------------------------------------------------------------------------
// Lock keys
// -----------------------------------------------------------------------------
//
// Every compile-time singleton gets its own `i64`. The upper 32 bits are the
// ASCII bytes `'D' 'D' 'B' _` so any key printed from `pg_locks` is greppable
// back to DarshJDB (easy to spot in a shared Postgres). The lower 32 bits are
// a per-task tag.
//
// `pg_try_advisory_lock` takes a single `bigint`, so we pack both halves into
// one `i64`. See <https://www.postgresql.org/docs/current/functions-admin.html>.

const LOCK_PREFIX: i64 = 0x4444_4200_0000_0000_u64 as i64; // "DDB\0"

/// Anchor writer (Phase 5.3): computes Keccak batch roots and submits them
/// to the configured blockchain backend.
pub const LOCK_ANCHOR_WRITER: i64 = LOCK_PREFIX | 0x0000_0001;

/// Agent-memory embedding worker (Phase 2.5): fills embeddings on
/// `memory_entries` / `agent_facts`.
pub const LOCK_EMBEDDING_WORKER: i64 = LOCK_PREFIX | 0x0000_0002;

/// Memory summariser poll (Phase 2.6): rolls hot-tier memory into warm.
/// Reserved — not spawned yet in v0.3.1.
pub const LOCK_MEMORY_SUMMARISER: i64 = LOCK_PREFIX | 0x0000_0003;

/// Session-manager expired-row cleanup. Reserved — the current session
/// manager cleans up inline during read/write, no dedicated sweeper yet.
pub const LOCK_SESSION_CLEANUP: i64 = LOCK_PREFIX | 0x0000_0004;

/// Triple-store TTL expiry sweeper (Phase 1.2): retracts expired entities
/// every 30 s.
pub const LOCK_EXPIRY_SWEEPER: i64 = LOCK_PREFIX | 0x0000_0005;

/// Chunked-upload stale-row cleanup (Phase 7.1): purges orphaned
/// `chunked_uploads` rows every 5 min.
pub const LOCK_CHUNKED_UPLOAD_CLEANUP: i64 = LOCK_PREFIX | 0x0000_0006;

// -----------------------------------------------------------------------------
// Node identity
// -----------------------------------------------------------------------------

/// A stable, process-lifetime identifier for this replica.
///
/// Generated once at startup via [`NodeId::new`] and surfaced through the
/// `/cluster/status` endpoint. There is no coordination with other replicas
/// — uniqueness is guaranteed by UUID v4's 122 bits of entropy.
#[derive(Clone, Debug)]
pub struct NodeId {
    uuid: Uuid,
    started_at: Instant,
}

impl NodeId {
    /// Create a new node identity for this process.
    pub fn new() -> Self {
        Self {
            uuid: Uuid::new_v4(),
            started_at: Instant::now(),
        }
    }

    /// The node's UUID.
    pub fn uuid(&self) -> Uuid {
        self.uuid
    }

    /// Seconds since this node started.
    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

// -----------------------------------------------------------------------------
// Leader election
// -----------------------------------------------------------------------------

/// Try to acquire a session-scoped advisory lock on `lock_key` against a
/// specific pooled connection.
///
/// Uses `pg_try_advisory_lock` (non-blocking). Returns `Ok(true)` if the
/// caller now holds the lock, `Ok(false)` if another session holds it.
///
/// **Important:** the lock is released when the underlying Postgres
/// session ends — i.e., when the pooled connection is dropped back to the
/// pool. Callers that need the lock to persist across many ticks MUST
/// hold the same `&mut PgConnection` (or `PoolConnection<Postgres>`) for
/// the lock's whole lifetime.
pub async fn try_acquire_leader(
    conn: &mut sqlx::PgConnection,
    lock_key: i64,
) -> Result<bool> {
    let row = sqlx::query("SELECT pg_try_advisory_lock($1) AS acquired")
        .bind(lock_key)
        .fetch_one(conn)
        .await
        .with_context(|| format!("pg_try_advisory_lock({lock_key})"))?;
    let acquired: bool = row
        .try_get("acquired")
        .context("pg_try_advisory_lock return column")?;
    Ok(acquired)
}

/// Release a session-scoped advisory lock on `lock_key`.
///
/// Explicit release is usually unnecessary — dropping the connection
/// releases every advisory lock it holds — but this is useful when a
/// long-lived connection wants to step down leadership voluntarily
/// (e.g., on graceful shutdown) without actually closing the connection.
pub async fn release_leader(conn: &mut sqlx::PgConnection, lock_key: i64) -> Result<()> {
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(lock_key)
        .execute(conn)
        .await
        .with_context(|| format!("pg_advisory_unlock({lock_key})"))?;
    Ok(())
}

/// Shared state describing which singletons this node currently leads.
///
/// The HTTP `/cluster/status` handler reads from this directly. Updates
/// are point-in-time — a replica may lose leadership mid-request, in
/// which case the status briefly lags reality. That's acceptable for an
/// observability endpoint.
#[derive(Clone, Default)]
pub struct ClusterState {
    inner: Arc<tokio::sync::RwLock<ClusterStateInner>>,
}

#[derive(Default)]
struct ClusterStateInner {
    /// Names of singleton tasks for which this node currently holds the
    /// advisory lock.
    leading: std::collections::BTreeSet<&'static str>,
}

impl ClusterState {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn mark_leader(&self, task: &'static str) {
        self.inner.write().await.leading.insert(task);
    }

    pub async fn mark_not_leader(&self, task: &'static str) {
        self.inner.write().await.leading.remove(task);
    }

    /// Snapshot the set of leader tasks (sorted, stable).
    pub async fn leader_for(&self) -> Vec<&'static str> {
        self.inner.read().await.leading.iter().copied().collect()
    }
}

/// Spawn a background task whose body runs only when this replica holds
/// the advisory lock for `lock_key`.
///
/// The spawned task owns one dedicated Postgres connection (taken from
/// `pool.acquire()`) for its entire lifetime. Every `tick`:
///
///   * If the task already holds the lock: run `body`, stay leader.
///   * If it does not: call `pg_try_advisory_lock`. On success, emit
///     `became leader`, update `cluster_state`, run `body`. On failure,
///     another replica holds it — sleep until the next tick.
///
/// If the dedicated connection dies (Postgres restart, network blip),
/// the next tick will re-acquire a new connection from the pool and
/// attempt to become leader again. Leadership naturally transfers
/// because the old connection's session ends and releases the lock.
///
/// The returned `JoinHandle` can be stored in a `_handle` binding or
/// dropped — dropping aborts the task and releases leadership via
/// connection drop.
pub fn spawn_singleton_task<F, Fut>(
    pool: Arc<PgPool>,
    cluster_state: ClusterState,
    lock_key: i64,
    tick: Duration,
    name: &'static str,
    body: F,
) -> JoinHandle<()>
where
    F: Fn(Arc<PgPool>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Dedicated leader-probe connection. Held for the lifetime of
        // leadership; dropped (and thus lock-released) only on reconnect
        // or task abort.
        let mut leader_conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>> = None;
        let mut is_leader = false;

        loop {
            interval.tick().await;

            // Ensure we have a probe connection. If the previous one died,
            // any lock held on it is already released server-side.
            if leader_conn.is_none() {
                match pool.acquire().await {
                    Ok(c) => leader_conn = Some(c),
                    Err(e) => {
                        warn!(
                            task = name,
                            error = %e,
                            "singleton task could not acquire probe connection, retrying"
                        );
                        if is_leader {
                            is_leader = false;
                            cluster_state.mark_not_leader(name).await;
                            info!(task = name, "lost leadership");
                        }
                        continue;
                    }
                }
            }

            // Try to become / stay leader.
            let conn = leader_conn.as_mut().expect("conn just set");
            let acquired = match try_acquire_leader(conn, lock_key).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        task = name,
                        error = %e,
                        "leader probe failed, dropping connection"
                    );
                    leader_conn = None;
                    if is_leader {
                        is_leader = false;
                        cluster_state.mark_not_leader(name).await;
                        info!(task = name, "lost leadership");
                    }
                    continue;
                }
            };

            // `pg_try_advisory_lock` returns `true` every time the same
            // session calls it — advisory locks are reentrant. So for a
            // session that is already leader, `acquired == true` on every
            // tick. We use the `is_leader` flag to debounce the "became
            // leader" log so it only fires on transition.
            if acquired {
                if !is_leader {
                    is_leader = true;
                    cluster_state.mark_leader(name).await;
                    info!(task = name, "became leader");
                }
                // Run the task body. It takes the POOL, not the leader
                // connection, so it can borrow as many short-lived
                // connections as it needs without interfering with the
                // advisory-lock session.
                body(pool.clone()).await;
            } else if is_leader {
                is_leader = false;
                cluster_state.mark_not_leader(name).await;
                info!(task = name, "lost leadership");
            }
        }
    })
}

/// Spawn a supervisor that runs a long-lived background worker only
/// while this replica holds the advisory lock for `lock_key`.
///
/// Unlike [`spawn_singleton_task`] — which invokes a short-lived body
/// on every tick — this variant is designed for tasks that own their
/// own internal loop (e.g. the agent-memory embedding worker, which
/// ticks every 5 s internally). The `start` closure is called exactly
/// once per leadership term and must return a `JoinHandle`; losing
/// leadership aborts that handle.
///
/// Leader polling cadence is fixed at 10 s, which is conservative for
/// long-running workers — flapping is more costly than slow failover.
pub fn spawn_singleton_supervisor<S>(
    pool: Arc<PgPool>,
    cluster_state: ClusterState,
    lock_key: i64,
    name: &'static str,
    start: S,
) -> JoinHandle<()>
where
    S: Fn() -> JoinHandle<()> + Send + Sync + 'static,
{
    tokio::spawn(async move {
        let probe_interval = Duration::from_secs(10);
        let mut ticker = tokio::time::interval(probe_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        let mut leader_conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>> = None;
        let mut worker: Option<JoinHandle<()>> = None;

        loop {
            ticker.tick().await;

            if leader_conn.is_none() {
                match pool.acquire().await {
                    Ok(c) => leader_conn = Some(c),
                    Err(e) => {
                        warn!(
                            task = name,
                            error = %e,
                            "supervisor could not acquire probe connection"
                        );
                        if let Some(h) = worker.take() {
                            h.abort();
                            cluster_state.mark_not_leader(name).await;
                            info!(task = name, "lost leadership (probe conn failure)");
                        }
                        continue;
                    }
                }
            }

            let conn = leader_conn.as_mut().expect("conn just set");
            let acquired = match try_acquire_leader(conn, lock_key).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(task = name, error = %e, "supervisor leader probe failed");
                    leader_conn = None;
                    if let Some(h) = worker.take() {
                        h.abort();
                        cluster_state.mark_not_leader(name).await;
                        info!(task = name, "lost leadership");
                    }
                    continue;
                }
            };

            if acquired {
                let needs_start = match worker.as_ref() {
                    None => true,
                    Some(h) if h.is_finished() => {
                        warn!(task = name, "worker exited while leader; restarting");
                        true
                    }
                    _ => false,
                };
                if needs_start {
                    if worker.is_none() {
                        info!(task = name, "became leader");
                        cluster_state.mark_leader(name).await;
                    }
                    worker = Some(start());
                }
            } else if let Some(h) = worker.take() {
                h.abort();
                cluster_state.mark_not_leader(name).await;
                info!(task = name, "lost leadership");
            }
        }
    })
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_keys_are_unique() {
        let keys = [
            LOCK_ANCHOR_WRITER,
            LOCK_EMBEDDING_WORKER,
            LOCK_MEMORY_SUMMARISER,
            LOCK_SESSION_CLEANUP,
            LOCK_EXPIRY_SWEEPER,
            LOCK_CHUNKED_UPLOAD_CLEANUP,
        ];
        let mut sorted = keys.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            keys.len(),
            "cluster lock keys must be pairwise unique"
        );
    }

    #[test]
    fn lock_keys_carry_ddb_prefix() {
        // All keys should share the top 32-bit signature so they're
        // greppable in `pg_locks`.
        let prefix_mask: i64 = 0xFFFF_FFFF_0000_0000_u64 as i64;
        for k in [
            LOCK_ANCHOR_WRITER,
            LOCK_EMBEDDING_WORKER,
            LOCK_MEMORY_SUMMARISER,
            LOCK_SESSION_CLEANUP,
            LOCK_EXPIRY_SWEEPER,
            LOCK_CHUNKED_UPLOAD_CLEANUP,
        ] {
            assert_eq!(k & prefix_mask, LOCK_PREFIX & prefix_mask);
        }
    }

    #[tokio::test]
    async fn node_id_is_unique_and_has_uptime() {
        let a = NodeId::new();
        let b = NodeId::new();
        assert_ne!(a.uuid(), b.uuid());
        let _ = a.uptime_secs();
    }

    #[tokio::test]
    async fn cluster_state_tracks_leadership() {
        let s = ClusterState::new();
        assert!(s.leader_for().await.is_empty());
        s.mark_leader("anchor_writer").await;
        s.mark_leader("embedding_worker").await;
        let v = s.leader_for().await;
        assert_eq!(v, vec!["anchor_writer", "embedding_worker"]);
        s.mark_not_leader("anchor_writer").await;
        assert_eq!(s.leader_for().await, vec!["embedding_worker"]);
    }
}
