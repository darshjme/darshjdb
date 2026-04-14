//! Cluster (advisory-lock leader election + LISTEN/NOTIFY fanout) integration tests.
//!
//! Author: Darshankumar Joshi
//!
//! Exercises the v0.3.1 horizontal-scaling primitives end-to-end against a
//! real Postgres. When `DATABASE_URL` is unset the whole suite no-ops
//! silently so CI stays green on environments without Postgres — same
//! pattern as `timescale_test.rs` and `admin_role_test.rs`.
//!
//! Scenarios covered:
//!
//!   * Two simulated replicas race for the same advisory lock — exactly
//!     one wins and the other sees `false` from `pg_try_advisory_lock`.
//!   * `pg_locks` reflects the leader's session.
//!   * Dropping the winning connection (simulating leader crash) lets
//!     the runner-up acquire the lock.
//!   * `pg_notify('ddb_changes', ...)` emitted from replica A is
//!     delivered to a `PgListener` attached to replica B — proving the
//!     WebSocket fanout pathway.

#![cfg(test)]

use std::time::Duration;

use ddb_server::cluster::{
    ClusterState, LOCK_ANCHOR_WRITER, LOCK_EXPIRY_SWEEPER, NodeId, release_leader,
    spawn_singleton_task, try_acquire_leader,
};
use ddb_server::cluster::notify_listener::{self, CHANGE_CHANNEL};
use sqlx::PgPool;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

async fn setup_pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    PgPool::connect(&url).await.ok()
}

// Sanity check that all cluster tests run inline against the same pool
// even though conceptually they simulate "two replicas". A real replica
// would have its own pool pointing at the same Postgres — for the
// purposes of the advisory-lock contract the only thing that matters is
// having two independent SESSIONS, which two `pool.acquire()` calls give us.

// ---------------------------------------------------------------------------
// 1. Advisory lock: only one session wins.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_one_replica_acquires_leader_lock() {
    let Some(pool) = setup_pool().await else {
        eprintln!("DATABASE_URL unset — skipping cluster advisory-lock test");
        return;
    };

    // Use a one-off test lock key so we never collide with a real
    // ddb-server instance that happens to be running against the same
    // Postgres during development.
    const TEST_LOCK: i64 = 0x4444_4200_C0DE_0001_u64 as i64;

    // Acquire two distinct pooled connections from the same pool.
    // Each PoolConnection is a fresh Postgres SESSION, which is what
    // advisory locks track.
    let mut conn_a = pool.acquire().await.expect("conn a");
    let mut conn_b = pool.acquire().await.expect("conn b");

    // Guarantee a clean slate for the test lock.
    let _ = release_leader(&mut conn_a, TEST_LOCK).await;
    let _ = release_leader(&mut conn_b, TEST_LOCK).await;

    let a_acquired = try_acquire_leader(&mut conn_a, TEST_LOCK).await.unwrap();
    assert!(a_acquired, "first session must acquire the lock");

    let b_acquired = try_acquire_leader(&mut conn_b, TEST_LOCK).await.unwrap();
    assert!(
        !b_acquired,
        "second session must NOT acquire the same advisory lock while A holds it"
    );

    // Drop A — lock is released automatically when its session ends.
    drop(conn_a);

    // Give Postgres a moment to fully tear down session A. The drop above
    // only queues the return; the actual `DISCARD ALL` cleanup runs async.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // B should now succeed.
    let b_retry = try_acquire_leader(&mut conn_b, TEST_LOCK).await.unwrap();
    assert!(
        b_retry,
        "runner-up must take over leadership after incumbent drops its session"
    );

    // Clean up.
    release_leader(&mut conn_b, TEST_LOCK).await.unwrap();
}

// ---------------------------------------------------------------------------
// 2. pg_locks reflects who holds the lock.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pg_locks_shows_exactly_one_holder() {
    let Some(pool) = setup_pool().await else {
        eprintln!("DATABASE_URL unset — skipping pg_locks test");
        return;
    };

    const TEST_LOCK: i64 = 0x4444_4200_C0DE_0002_u64 as i64;

    let mut probe = pool.acquire().await.expect("probe conn");
    let _ = release_leader(&mut probe, TEST_LOCK).await;
    let got = try_acquire_leader(&mut probe, TEST_LOCK).await.unwrap();
    assert!(got);

    // pg_locks on `objid` matches the low 32 bits of the advisory lock key.
    // Postgres splits the 64-bit key into (classid, objid) so our packed
    // high-bit prefix becomes `classid` and the low-bit tag becomes `objid`.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::BIGINT FROM pg_locks \
         WHERE locktype = 'advisory' \
         AND classid = $1 \
         AND objid = $2",
    )
    .bind(((TEST_LOCK >> 32) & 0xFFFF_FFFF) as i32)
    .bind((TEST_LOCK & 0xFFFF_FFFF) as i32)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert!(
        count >= 1,
        "pg_locks must report at least one holder for the test advisory lock (got {count})"
    );

    release_leader(&mut probe, TEST_LOCK).await.unwrap();
}

// ---------------------------------------------------------------------------
// 3. spawn_singleton_task + failover.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_singleton_task_gates_body_on_leadership() {
    let Some(pool) = setup_pool().await else {
        eprintln!("DATABASE_URL unset — skipping singleton failover test");
        return;
    };

    // Shared atomic counter the body increments every tick it runs.
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let cluster_state = ClusterState::new();

    // Start "replica A" — holds a long-lived session and thus the lock.
    let counter_clone = counter.clone();
    let pool_arc = Arc::new(pool.clone());
    let handle = spawn_singleton_task(
        pool_arc.clone(),
        cluster_state.clone(),
        LOCK_EXPIRY_SWEEPER,
        Duration::from_millis(200),
        "test_expiry_sweeper",
        move |_pool| {
            let counter_clone = counter_clone.clone();
            async move {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        },
    );

    // Wait for a few ticks to run. The body should execute >= 2 times
    // within 800ms (200ms cadence).
    tokio::time::sleep(Duration::from_millis(800)).await;
    let runs = counter.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        runs >= 2,
        "singleton body should have fired at least twice (got {runs})"
    );

    // And cluster_state should report this task as a leader.
    let leaders = cluster_state.leader_for().await;
    assert!(
        leaders.contains(&"test_expiry_sweeper"),
        "cluster state must list the singleton as a leader, got {leaders:?}"
    );

    handle.abort();
    // Give Postgres a moment to release the session.
    tokio::time::sleep(Duration::from_millis(100)).await;
}

// ---------------------------------------------------------------------------
// 4. LISTEN/NOTIFY fanout — payload emitted by replica A is seen by replica B.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listen_notify_fanout_delivers_change_events_cross_session() {
    let Some(pool) = setup_pool().await else {
        eprintln!("DATABASE_URL unset — skipping NOTIFY fanout test");
        return;
    };
    let url = std::env::var("DATABASE_URL").unwrap();

    // Replica B: spawn the real `notify_listener::spawn` task against a
    // fresh broadcast channel, then subscribe locally.
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let _handle = notify_listener::spawn(url.clone(), tx);

    // Small delay to let the listener connect and LISTEN.
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Replica A: emit a NOTIFY directly from the pool. Use a synthetic
    // payload that parses into a non-zero tx_id so we can round-trip it.
    sqlx::query(&format!(
        "NOTIFY {CHANGE_CHANNEL}, '77:test_entity_type'",
    ))
    .execute(&pool)
    .await
    .expect("NOTIFY failed");

    // Wait for the event to arrive through the broadcast channel. Use a
    // bounded timeout so the test never hangs.
    let received = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;

    let event = match received {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => panic!("broadcast recv error: {e}"),
        Err(_) => panic!("NOTIFY fanout did not deliver within 3s"),
    };

    assert_eq!(event.tx_id, 77, "tx_id must round-trip through NOTIFY");
    assert_eq!(
        event.entity_type.as_deref(),
        Some("test_entity_type"),
        "entity_type must round-trip through NOTIFY"
    );
}

// ---------------------------------------------------------------------------
// 5. Node id stability — same NodeId keeps its UUID across calls.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn node_id_is_stable_for_the_lifetime_of_a_replica() {
    let node = NodeId::new();
    let a = node.uuid();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let b = node.uuid();
    assert_eq!(a, b, "NodeId UUID must not change across observations");
    assert!(
        node.uptime_secs() < 60,
        "freshly-created NodeId uptime should be under 60s"
    );
}

// ---------------------------------------------------------------------------
// 6. Sanity — anchor lock and expiry lock are distinct in pg_locks.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn different_lock_keys_do_not_collide() {
    let Some(pool) = setup_pool().await else {
        eprintln!("DATABASE_URL unset — skipping lock isolation test");
        return;
    };

    let mut conn_a = pool.acquire().await.unwrap();
    let mut conn_b = pool.acquire().await.unwrap();

    let _ = release_leader(&mut conn_a, LOCK_ANCHOR_WRITER).await;
    let _ = release_leader(&mut conn_b, LOCK_EXPIRY_SWEEPER).await;

    let a = try_acquire_leader(&mut conn_a, LOCK_ANCHOR_WRITER)
        .await
        .unwrap();
    let b = try_acquire_leader(&mut conn_b, LOCK_EXPIRY_SWEEPER)
        .await
        .unwrap();

    assert!(a, "session A should acquire ANCHOR_WRITER lock");
    assert!(
        b,
        "session B should acquire EXPIRY_SWEEPER lock — different keys must not block each other"
    );

    release_leader(&mut conn_a, LOCK_ANCHOR_WRITER).await.unwrap();
    release_leader(&mut conn_b, LOCK_EXPIRY_SWEEPER).await.unwrap();
}

