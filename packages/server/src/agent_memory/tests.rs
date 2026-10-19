// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Unit tests for the agent-memory subsystem.
//!
//! Most tests avoid live Postgres so they run on every
//! `cargo test` without provisioning a database. The token counter,
//! working memory, and the parts of [`ContextBuilder`] that operate on
//! in-memory data are exercised directly. Full end-to-end coverage —
//! including the REST handlers — lives in `packages/server/tests/`.
//! The explicitly ignored durability regression below requires isolated Postgres.

use super::tokens::TiktokenCounter;
use super::types::{MemoryEntry, MemoryRole, MemoryTier};
use super::working::WorkingMemory;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

fn make_entry(role: MemoryRole, content: &str) -> MemoryEntry {
    let counter = TiktokenCounter::default_counter();
    MemoryEntry {
        id: Uuid::new_v4(),
        session_id: Uuid::nil(),
        tier: MemoryTier::Working,
        role,
        content: content.into(),
        token_count: counter.count(content) as i32,
        metadata: json!({}),
        created_at: Utc::now(),
    }
}

#[test]
fn working_memory_evicts_oldest_when_full() {
    let wm = WorkingMemory::with_capacity(3);
    let sid = Uuid::new_v4();
    assert!(
        wm.push(sid, make_entry(MemoryRole::User, "first"))
            .is_none()
    );
    assert!(
        wm.push(sid, make_entry(MemoryRole::Assistant, "second"))
            .is_none()
    );
    assert!(
        wm.push(sid, make_entry(MemoryRole::User, "third"))
            .is_none()
    );

    let evicted = wm
        .push(sid, make_entry(MemoryRole::Assistant, "fourth"))
        .expect("should evict oldest");
    assert_eq!(evicted.content, "first");

    let snap = wm.snapshot(sid);
    assert_eq!(snap.len(), 3);
    assert_eq!(snap[0].content, "second");
    assert_eq!(snap[2].content, "fourth");
}

#[test]
fn working_memory_total_tokens_matches_sum() {
    let wm = WorkingMemory::with_capacity(5);
    let sid = Uuid::new_v4();
    wm.push(sid, make_entry(MemoryRole::User, "alpha beta"));
    wm.push(
        sid,
        make_entry(MemoryRole::Assistant, "gamma delta epsilon"),
    );

    let snap = wm.snapshot(sid);
    let expected: i64 = snap.iter().map(|m| m.token_count as i64).sum();
    assert_eq!(wm.total_tokens(sid), expected);
    assert!(wm.total_tokens(sid) > 0);
}

#[test]
fn tier_and_role_round_trip_strings() {
    for tier in [
        MemoryTier::Working,
        MemoryTier::Episodic,
        MemoryTier::Semantic,
    ] {
        let s = tier.as_str();
        assert_eq!(MemoryTier::parse(s), Some(tier));
    }
    for role in [
        MemoryRole::System,
        MemoryRole::User,
        MemoryRole::Assistant,
        MemoryRole::Tool,
    ] {
        let s = role.as_str();
        assert_eq!(MemoryRole::parse(s), Some(role));
    }
}

#[test]
fn token_counter_counts_grow_with_text_length() {
    let c = TiktokenCounter::default_counter();
    let short = c.count("hi");
    let long = c.count("the quick brown fox jumps over the lazy dog repeatedly");
    assert!(long > short);
    assert!(short > 0);
}

#[test]
fn token_counter_handles_unicode() {
    let c = TiktokenCounter::default_counter();
    assert!(c.count("नमस्ते दुनिया") > 0);
    assert!(c.count("こんにちは世界") > 0);
}

// -------------------------------------------------------------------
// Pure budgeting helpers — exercised without touching Postgres.
// -------------------------------------------------------------------

/// Reproduce the inner per-entry budget arithmetic the context builder
/// uses, so we can prove its invariants without spinning up sqlx.
fn pack_within_budget(entries: &[MemoryEntry], budget: usize) -> (Vec<&MemoryEntry>, usize) {
    let mut packed = Vec::new();
    let mut remaining = budget;
    for e in entries.iter().rev() {
        let toks = e.token_count as usize;
        if toks > remaining {
            break;
        }
        remaining -= toks;
        packed.push(e);
    }
    packed.reverse();
    (packed, remaining)
}

#[test]
fn budget_packing_respects_max_tokens() {
    let entries = vec![
        make_entry(MemoryRole::User, "first message"),
        make_entry(
            MemoryRole::Assistant,
            "second message a bit longer than the first",
        ),
        make_entry(MemoryRole::User, "third"),
    ];

    // Generous budget: everything fits.
    let total: usize = entries.iter().map(|e| e.token_count as usize).sum();
    let (packed, remaining) = pack_within_budget(&entries, total + 100);
    assert_eq!(packed.len(), entries.len());
    assert_eq!(remaining, 100);

    // Tight budget: only the most recent entry fits.
    let last_tokens = entries.last().unwrap().token_count as usize;
    let (packed, remaining) = pack_within_budget(&entries, last_tokens);
    assert_eq!(packed.len(), 1);
    assert_eq!(remaining, 0);
    assert_eq!(packed[0].content, "third");

    // Zero budget: nothing fits.
    let (packed, remaining) = pack_within_budget(&entries, 0);
    assert!(packed.is_empty());
    assert_eq!(remaining, 0);
}

/// Run explicitly with DDB_TEST_DATABASE_URL pointing at an isolated test server.
/// This test fails rather than silently skipping when the database is unavailable.
#[tokio::test]
#[ignore = "requires isolated PostgreSQL; run with --ignored"]
async fn durable_working_window_survives_restart_and_concurrent_writers() {
    use super::{AgentMemoryRepo, ContextBuildOptions, ContextBuilder, ensure_agent_memory_schema};
    use std::collections::HashSet;
    let url = std::env::var("DDB_TEST_DATABASE_URL").expect("DDB_TEST_DATABASE_URL required");
    let admin = sqlx::PgPool::connect(&url).await.unwrap();
    let db_name = format!("memory_regression_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {db_name}"))
        .execute(&admin)
        .await
        .unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect_with(options.database(&db_name))
        .await
        .unwrap();
    ensure_agent_memory_schema(&pool).await.unwrap();
    let repo = AgentMemoryRepo::new(pool.clone());
    let session = repo
        .create_session(Uuid::new_v4(), "durability-test", None, json!({}))
        .await
        .unwrap();
    let (first, promoted) = repo
        .append_working(
            session.id,
            MemoryRole::User,
            "survives restart",
            3,
            json!({"keep":true}),
            32,
        )
        .await
        .unwrap();
    assert!(promoted.is_empty());
    // Entirely fresh RAM state and repository handle, as on another replica.
    let mut cold = ContextBuilder::new(
        session.id,
        TiktokenCounter::default_counter(),
        AgentMemoryRepo::new(pool.clone()),
        WorkingMemory::default(),
    );
    let bundle = cold.build(&ContextBuildOptions::default()).await.unwrap();
    assert_eq!(bundle.messages.len(), 1);
    assert_eq!(bundle.messages[0].content, first.content);
    assert_eq!(
        repo.session_stats(session.id)
            .await
            .unwrap()
            .unwrap()
            .working_messages,
        1
    );
    let mut writes = tokio::task::JoinSet::new();
    for i in 0..39 {
        let replica = AgentMemoryRepo::new(pool.clone());
        writes.spawn(async move {
            replica
                .append_working(
                    session.id,
                    MemoryRole::User,
                    &format!("message {i}"),
                    3,
                    json!({}),
                    32,
                )
                .await
                .unwrap()
        });
    }
    let mut original_ids = HashSet::from([first.id]);
    while let Some(result) = writes.join_next().await {
        original_ids.insert(result.unwrap().0.id);
    }
    let rows = repo
        .timeline(session.id, &Default::default())
        .await
        .unwrap();
    assert_eq!(rows.len(), 40, "promotion must never duplicate messages");
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<HashSet<_>>(),
        original_ids
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.tier == MemoryTier::Working)
            .count(),
        32
    );
    assert_eq!(
        rows.iter()
            .filter(|r| r.tier == MemoryTier::Episodic)
            .count(),
        8
    );
    let original = rows.iter().find(|r| r.id == first.id).unwrap();
    assert_eq!(original.tier, MemoryTier::Episodic);
    assert_eq!(original.created_at, first.created_at);
    assert_eq!(original.metadata, first.metadata);
    let stats = repo.session_stats(session.id).await.unwrap().unwrap();
    assert_eq!(stats.working_messages, 32);
    assert_eq!(stats.episodic_messages, 8);
    let bundle = cold.build(&ContextBuildOptions::default()).await.unwrap();
    assert_eq!(bundle.messages.len(), 40);
    assert_eq!(
        bundle
            .messages
            .iter()
            .filter(|m| m.tier == "working")
            .count(),
        32
    );
    assert_eq!(
        bundle
            .messages
            .iter()
            .map(|m| &m.content)
            .collect::<HashSet<_>>()
            .len(),
        40
    );
    pool.close().await;
    // Only this test's random, freshly-created database is removed.
    sqlx::query(&format!("DROP DATABASE {db_name}"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
}
