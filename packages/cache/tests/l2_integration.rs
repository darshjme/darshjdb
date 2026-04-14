// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// l2_integration: end-to-end tests for the L2 Postgres cache tier.
//
// These tests use `#[sqlx::test]`, which spins up a fresh database per test
// and applies all migrations in the configured fixture path. Each test runs
// against an isolated schema so they can execute in parallel.
//
// Run with:
//   cargo test -p ddb-cache --tests l2
// Requires DATABASE_URL pointing at a Postgres instance the sqlx CLI can
// `CREATE DATABASE` against.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use ddb_cache::l2::{L2Cache, L2Error};
use sqlx::PgPool;

fn cache(pool: PgPool) -> L2Cache {
    L2Cache::new(Arc::new(pool))
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_set_get_del_roundtrip(pool: PgPool) {
    let c = cache(pool);
    c.set("k1", b"hello", None).await.unwrap();
    let got = c.get("k1").await.unwrap();
    assert_eq!(got.as_deref(), Some(&b"hello"[..]));

    assert!(c.exists("k1").await.unwrap());
    assert!(c.del("k1").await.unwrap());
    assert!(!c.exists("k1").await.unwrap());
    assert!(c.get("k1").await.unwrap().is_none());
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_large_payload_uses_zstd(pool: PgPool) {
    let c = cache(pool);
    let big = vec![b'Z'; 8 * 1024];
    c.set("big", &big, None).await.unwrap();
    let got = c.get("big").await.unwrap().unwrap();
    assert_eq!(got, big);
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_ttl_and_expire(pool: PgPool) {
    let c = cache(pool);
    c.set("ephemeral", b"vanish", Some(Duration::from_secs(60)))
        .await
        .unwrap();
    let ttl = c.ttl("ephemeral").await.unwrap();
    assert!(matches!(ttl, Some(n) if n > 0 && n <= 60));

    // Reset to no TTL on a different key
    c.set("perma", b"forever", None).await.unwrap();
    assert_eq!(c.ttl("perma").await.unwrap(), Some(-1));

    // EXPIRE on an existing key
    assert!(c.expire("perma", Duration::from_secs(30)).await.unwrap());
    let ttl = c.ttl("perma").await.unwrap();
    assert!(matches!(ttl, Some(n) if n > 0 && n <= 30));

    // EXPIRE on a missing key
    assert!(!c.expire("missing", Duration::from_secs(10)).await.unwrap());
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_keys_pattern(pool: PgPool) {
    let c = cache(pool);
    c.set("user:1", b"a", None).await.unwrap();
    c.set("user:2", b"b", None).await.unwrap();
    c.set("session:1", b"c", None).await.unwrap();

    let mut all = c.keys("*").await.unwrap();
    all.sort();
    assert_eq!(all, vec!["session:1", "user:1", "user:2"]);

    let mut users = c.keys("user:*").await.unwrap();
    users.sort();
    assert_eq!(users, vec!["user:1", "user:2"]);
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_hash_ops(pool: PgPool) {
    let c = cache(pool);
    assert!(c.hset("user:42", "name", "darshan").await.unwrap());
    assert!(c.hset("user:42", "role", "founder").await.unwrap());
    // overwrite is not "new"
    assert!(!c.hset("user:42", "name", "darshan").await.unwrap());

    assert_eq!(c.hget("user:42", "name").await.unwrap().as_deref(), Some("darshan"));
    assert_eq!(c.hget("user:42", "role").await.unwrap().as_deref(), Some("founder"));
    assert_eq!(c.hget("user:42", "missing").await.unwrap(), None);

    let all = c.hgetall("user:42").await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all.get("name").map(String::as_str), Some("darshan"));
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_list_ops(pool: PgPool) {
    let c = cache(pool);
    assert_eq!(c.rpush("q", "a").await.unwrap(), 1);
    assert_eq!(c.rpush("q", "b").await.unwrap(), 2);
    assert_eq!(c.rpush("q", "c").await.unwrap(), 3);
    assert_eq!(c.lpush("q", "z").await.unwrap(), 4);

    let all = c.lrange("q", 0, -1).await.unwrap();
    assert_eq!(all, vec!["z", "a", "b", "c"]);

    let head = c.lrange("q", 0, 1).await.unwrap();
    assert_eq!(head, vec!["z", "a"]);

    let tail = c.lrange("q", -2, -1).await.unwrap();
    assert_eq!(tail, vec!["b", "c"]);
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_zset_ops(pool: PgPool) {
    let c = cache(pool);
    assert!(c.zadd("scores", 10.0, "alice").await.unwrap());
    assert!(c.zadd("scores", 5.0, "bob").await.unwrap());
    assert!(c.zadd("scores", 20.0, "carol").await.unwrap());
    // re-score is not "added"
    assert!(!c.zadd("scores", 7.5, "bob").await.unwrap());

    let asc = c.zrange("scores", 0, -1).await.unwrap();
    assert_eq!(asc, vec!["bob", "alice", "carol"]);
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_stream_xadd_xrange_xlen(pool: PgPool) {
    let c = cache(pool);
    let mut f1 = HashMap::new();
    f1.insert("event".to_string(), "login".to_string());
    f1.insert("user".to_string(), "1".to_string());
    let id1 = c.xadd("events", &f1).await.unwrap();
    assert!(id1.contains('-'));

    let mut f2 = HashMap::new();
    f2.insert("event".to_string(), "logout".to_string());
    f2.insert("user".to_string(), "1".to_string());
    let id2 = c.xadd("events", &f2).await.unwrap();
    assert_ne!(id1, id2);

    assert_eq!(c.xlen("events").await.unwrap(), 2);

    let all = c.xrange("events", "-", "+").await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, id1);
    assert_eq!(all[1].id, id2);
    assert_eq!(all[0].fields.get("event").map(String::as_str), Some("login"));
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_stream_xread_progress(pool: PgPool) {
    let c = cache(pool);
    for i in 0..5 {
        let mut f = HashMap::new();
        f.insert("n".to_string(), i.to_string());
        c.xadd("counter", &f).await.unwrap();
    }
    let first = c.xread("counter", "0", 3).await.unwrap();
    assert_eq!(first.len(), 3);
    let last_id = first.last().unwrap().id.clone();
    let next = c.xread("counter", &last_id, 10).await.unwrap();
    assert_eq!(next.len(), 2);
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_type_mismatch_errors(pool: PgPool) {
    let c = cache(pool);
    c.set("plain", b"hello", None).await.unwrap();
    let err = c.hget("plain", "x").await.unwrap_err();
    matches!(err, L2Error::TypeMismatch { .. });
}

#[sqlx::test(migrations = "../server/migrations")]
async fn l2_sweep_expired_once(pool: PgPool) {
    let c = cache(pool);

    // Insert a row that is already expired by setting an explicit past expiry.
    sqlx::query(
        r#"
        INSERT INTO kv_store (key, value, kind, expires_at, size_bytes)
        VALUES ($1, $2, 'string', now() - interval '1 hour', $3)
        "#,
    )
    .bind("dead")
    .bind(vec![0x01u8])
    .bind(1i32)
    .execute(c.pool())
    .await
    .unwrap();

    // And one that is still alive.
    c.set("alive", b"v", Some(Duration::from_secs(3600))).await.unwrap();

    let removed = c.sweep_expired_once(1000).await.unwrap();
    assert!(removed >= 1);

    assert!(!c.exists("dead").await.unwrap());
    assert!(c.exists("alive").await.unwrap());
}
