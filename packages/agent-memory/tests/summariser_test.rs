// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// DB-gated integration test for the episodic → semantic summariser.
//
// This test is intentionally a no-op when `DATABASE_URL` is unset so
// that `cargo test -p ddb-agent-memory` stays green in offline CI.
// When `DATABASE_URL` *is* set (a postgres with pgvector + the
// `20260414055500_agent_memory.sql` migration already applied), the
// test:
//   1. Creates a fresh agent_sessions row.
//   2. Inserts 20 episodic memory_entries rows on that session.
//   3. Calls summarise_oldest_episodic with NoneClient.
//   4. Asserts exactly 1 summary row exists (tier='semantic',
//      role='summary') and that the 20 source entries are gone.

use ddb_agent_memory::summariser::{NoneClient, summarise_oldest_episodic};
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::test]
async fn summariser_replaces_20_episodic_with_1_semantic() {
    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        eprintln!(
            "skipping summariser integration test: DATABASE_URL not set \
             (this is expected in offline CI)"
        );
        return;
    };

    let pool = match PgPoolOptions::new()
        .max_connections(2)
        .connect(&database_url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "skipping summariser integration test: could not connect \
                 to {database_url}: {e}"
            );
            return;
        }
    };

    // Bootstrap the agent_memory schema inline so the test is
    // self-contained and can run against a fresh CI Postgres service
    // container without relying on `ddb_server::ensure_agent_memory_schema`
    // (which would create a circular dependency from ddb-agent-memory
    // back to ddb-server). Skip the test entirely if pgvector is not
    // installed — the hosted CI runner may not have it.
    if let Err(e) = sqlx::raw_sql(
        "CREATE EXTENSION IF NOT EXISTS vector;

         CREATE TABLE IF NOT EXISTS agent_sessions (
             session_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
             agent_id TEXT NOT NULL,
             model TEXT NOT NULL,
             created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
             last_active_at TIMESTAMPTZ NOT NULL DEFAULT now(),
             metadata JSONB NOT NULL DEFAULT '{}'::jsonb
         );

         CREATE TABLE IF NOT EXISTS memory_entries (
             id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
             session_id UUID NOT NULL REFERENCES agent_sessions(session_id)
                 ON DELETE CASCADE,
             agent_id TEXT NOT NULL,
             role TEXT NOT NULL,
             content TEXT NOT NULL,
             content_tokens INTEGER NOT NULL DEFAULT 0,
             importance DOUBLE PRECISION NOT NULL DEFAULT 0.5,
             tier TEXT NOT NULL DEFAULT 'working',
             created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
             accessed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
             access_count INTEGER NOT NULL DEFAULT 0,
             compressed BOOLEAN NOT NULL DEFAULT false
         );",
    )
    .execute(&pool)
    .await
    {
        eprintln!(
            "skipping summariser integration test: schema bootstrap failed \
             (pgvector extension probably missing): {e}"
        );
        return;
    }

    // Unique per-run agent id so repeat runs don't collide.
    let agent_id = format!("test-summariser-{}", Uuid::new_v4());

    // 1. Fresh session.
    let session_id: Uuid = sqlx::query_scalar(
        "INSERT INTO agent_sessions (agent_id, model)
         VALUES ($1, $2)
         RETURNING session_id",
    )
    .bind(&agent_id)
    .bind("test-model")
    .fetch_one(&pool)
    .await
    .expect("insert agent_sessions");

    // 2. Insert 20 episodic entries, spaced by created_at so ORDER BY
    //    is deterministic.
    for i in 0..20i32 {
        sqlx::query(
            "INSERT INTO memory_entries
                (session_id, agent_id, role, content, content_tokens,
                 importance, tier, created_at, accessed_at)
             VALUES ($1, $2, $3, $4, $5, 0.5, 'episodic',
                     NOW() - ($6::int || ' seconds')::interval,
                     NOW())",
        )
        .bind(session_id)
        .bind(&agent_id)
        .bind(if i % 2 == 0 { "user" } else { "assistant" })
        .bind(format!("integration test message {i}"))
        .bind(8i32)
        .bind(100 - i) // older rows first
        .execute(&pool)
        .await
        .expect("insert memory_entries");
    }

    // 3. Run the summariser with the deterministic offline client.
    let llm = NoneClient;
    let new_summary_id = summarise_oldest_episodic(&pool, session_id, &llm)
        .await
        .expect("summariser runs cleanly");
    assert!(
        new_summary_id.is_some(),
        "with 20 episodic rows summariser must produce Some(id)"
    );

    // 4a. Exactly one summary row, on the semantic tier, for this session.
    let summary_rows = sqlx::query(
        "SELECT id, role, tier, content FROM memory_entries
          WHERE session_id = $1 AND tier = 'semantic'",
    )
    .bind(session_id)
    .fetch_all(&pool)
    .await
    .expect("select semantic rows");
    assert_eq!(summary_rows.len(), 1, "one summary row expected");
    let r = &summary_rows[0];
    let role: String = r.try_get("role").unwrap();
    let tier: String = r.try_get("tier").unwrap();
    assert_eq!(role, "summary");
    assert_eq!(tier, "semantic");

    // 4b. No episodic rows left for this session.
    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_entries
          WHERE session_id = $1 AND tier = 'episodic'",
    )
    .bind(session_id)
    .fetch_one(&pool)
    .await
    .expect("count episodic rows");
    assert_eq!(remaining, 0, "all 20 source rows must be deleted");

    // Best-effort cleanup so the DB doesn't accumulate debris.
    let _ = sqlx::query("DELETE FROM memory_entries WHERE session_id = $1")
        .bind(session_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM agent_sessions WHERE session_id = $1")
        .bind(session_id)
        .execute(&pool)
        .await;
}
