// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Data-access layer for the agent-memory subsystem.
//!
//! All SQL is parameterised and never built via string interpolation.
//! Public methods take an owning `&PgPool` and return concrete domain
//! types from [`super::types`].

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::Row;
use sqlx::types::Json as SqlxJson;
use uuid::Uuid;

use super::tokens::TiktokenCounter;
use super::types::{
    AgentFact, AgentSession, MemoryEntry, MemoryRole, MemoryTier, SessionStats, TimelineFilter,
};

/// Repository handle. Stateless — clone is `Arc`-cheap because the pool is.
#[derive(Clone)]
pub struct AgentMemoryRepo {
    pool: PgPool,
}

impl AgentMemoryRepo {
    /// Build a new repo around a connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Borrow the pool — handy for callers that need a one-off transaction.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    // -------------------------------------------------------------------
    // Sessions
    // -------------------------------------------------------------------

    /// Insert a new session row and return the persisted record.
    pub async fn create_session(
        &self,
        user_id: Uuid,
        agent_id: &str,
        model: Option<&str>,
        metadata: Value,
    ) -> Result<AgentSession, sqlx::Error> {
        let id = Uuid::new_v4();
        let row = sqlx::query(
            r#"
            INSERT INTO agent_sessions (id, user_id, agent_id, model, metadata)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, user_id, agent_id, model, metadata, created_at, updated_at, final_summary
            "#,
        )
        .bind(id)
        .bind(user_id)
        .bind(agent_id)
        .bind(model)
        .bind(SqlxJson(metadata))
        .fetch_one(&self.pool)
        .await?;

        Ok(row_to_session(&row))
    }

    /// Look up a session by primary key, scoped to the owning user.
    pub async fn get_session(
        &self,
        session_id: Uuid,
        user_id: Uuid,
    ) -> Result<Option<AgentSession>, sqlx::Error> {
        let row = sqlx::query(
            r#"
            SELECT id, user_id, agent_id, model, metadata, created_at, updated_at, final_summary
            FROM agent_sessions
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(session_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.as_ref().map(row_to_session))
    }

    /// Persist a final summary and mark the session closed.
    pub async fn close_session(
        &self,
        session_id: Uuid,
        user_id: Uuid,
        summary: &str,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE agent_sessions
            SET final_summary = $3, updated_at = now()
            WHERE id = $1 AND user_id = $2
            "#,
        )
        .bind(session_id)
        .bind(user_id)
        .bind(summary)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    // -------------------------------------------------------------------
    // Messages
    // -------------------------------------------------------------------

    /// Insert a new message into the given tier.
    ///
    /// `token_count` is computed by the caller (typically the handler,
    /// which already holds a [`TiktokenCounter`]) so the same count
    /// flows into both the working tier eviction maths and the DB row.
    pub async fn insert_message(
        &self,
        session_id: Uuid,
        tier: MemoryTier,
        role: MemoryRole,
        content: &str,
        token_count: i32,
        metadata: Value,
    ) -> Result<MemoryEntry, sqlx::Error> {
        let id = Uuid::new_v4();
        let row = sqlx::query(
            r#"
            INSERT INTO memory_entries
                (id, session_id, tier, role, content, token_count, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, session_id, tier, role, content, token_count, metadata, created_at
            "#,
        )
        .bind(id)
        .bind(session_id)
        .bind(tier.as_str())
        .bind(role.as_str())
        .bind(content)
        .bind(token_count)
        .bind(SqlxJson(metadata))
        .fetch_one(&self.pool)
        .await?;

        // Touch the parent session.
        let _ = sqlx::query("UPDATE agent_sessions SET updated_at = now() WHERE id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;

        Ok(row_to_entry(&row))
    }

    /// Fetch the most recent N entries from a tier (reverse-chron).
    pub async fn recent_in_tier(
        &self,
        session_id: Uuid,
        tier: MemoryTier,
        limit: i64,
    ) -> Result<Vec<MemoryEntry>, sqlx::Error> {
        let rows = sqlx::query(
            r#"
            SELECT id, session_id, tier, role, content, token_count, metadata, created_at
            FROM memory_entries
            WHERE session_id = $1 AND tier = $2
            ORDER BY created_at DESC
            LIMIT $3
            "#,
        )
        .bind(session_id)
        .bind(tier.as_str())
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(row_to_entry).collect())
    }

    /// Filtered timeline query.
    pub async fn timeline(
        &self,
        session_id: Uuid,
        filter: &TimelineFilter,
    ) -> Result<Vec<MemoryEntry>, sqlx::Error> {
        let limit: i64 = filter.limit.unwrap_or(100).min(1000) as i64;
        let from: Option<DateTime<Utc>> = filter.from;
        let to: Option<DateTime<Utc>> = filter.to;
        let tier: Option<&str> = filter.tier.as_deref();

        let rows = sqlx::query(
            r#"
            SELECT id, session_id, tier, role, content, token_count, metadata, created_at
            FROM memory_entries
            WHERE session_id = $1
              AND ($2::timestamptz IS NULL OR created_at >= $2)
              AND ($3::timestamptz IS NULL OR created_at <= $3)
              AND ($4::text IS NULL OR tier = $4)
            ORDER BY created_at ASC
            LIMIT $5
            "#,
        )
        .bind(session_id)
        .bind(from)
        .bind(to)
        .bind(tier)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(row_to_entry).collect())
    }

    /// Lightweight keyword search across episodic + semantic tiers.
    ///
    /// This is the slice-12 fallback used until pgvector recall lands —
    /// it ranks rows by `position($q in lower(content))` and length, so
    /// shorter, earlier-matching messages float to the top. The signature
    /// is deliberately identical to a future ANN lookup so the context
    /// builder needs no changes when the pgvector path arrives.
    pub async fn semantic_recall(
        &self,
        session_id: Uuid,
        query: &str,
        top_k: i64,
    ) -> Result<Vec<MemoryEntry>, sqlx::Error> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let needle = format!("%{}%", query.to_lowercase());
        let rows = sqlx::query(
            r#"
            SELECT id, session_id, tier, role, content, token_count, metadata, created_at
            FROM memory_entries
            WHERE session_id = $1
              AND tier IN ('episodic', 'semantic')
              AND lower(content) LIKE $2
            ORDER BY length(content) ASC, created_at DESC
            LIMIT $3
            "#,
        )
        .bind(session_id)
        .bind(needle)
        .bind(top_k)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(row_to_entry).collect())
    }

    /// Aggregate stats for a session.
    pub async fn session_stats(
        &self,
        session_id: Uuid,
    ) -> Result<Option<SessionStats>, sqlx::Error> {
        let session_row = sqlx::query(
            r#"
            SELECT created_at, updated_at
            FROM agent_sessions
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(session_row) = session_row else {
            return Ok(None);
        };

        let counts = sqlx::query(
            r#"
            SELECT tier,
                   COUNT(*)::BIGINT AS cnt,
                   COALESCE(SUM(token_count), 0)::BIGINT AS toks
            FROM memory_entries
            WHERE session_id = $1
            GROUP BY tier
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await?;

        let mut episodic = 0usize;
        let mut semantic = 0usize;
        let mut total_tokens: i64 = 0;
        for row in &counts {
            let tier: String = row.try_get("tier").unwrap_or_default();
            let cnt: i64 = row.try_get("cnt").unwrap_or(0);
            let toks: i64 = row.try_get("toks").unwrap_or(0);
            total_tokens += toks;
            match tier.as_str() {
                "episodic" => episodic = cnt as usize,
                "semantic" => semantic = cnt as usize,
                _ => {}
            }
        }

        Ok(Some(SessionStats {
            session_id,
            working_messages: 0, // patched in by handler from the WorkingMemory snapshot
            episodic_messages: episodic,
            semantic_messages: semantic,
            total_tokens,
            created_at: session_row
                .try_get("created_at")
                .unwrap_or_else(|_| Utc::now()),
            updated_at: session_row
                .try_get("updated_at")
                .unwrap_or_else(|_| Utc::now()),
        }))
    }

    // -------------------------------------------------------------------
    // Facts
    // -------------------------------------------------------------------

    /// Upsert an agent fact keyed by `(agent_id, user_id, key)`.
    pub async fn upsert_fact(
        &self,
        agent_id: &str,
        user_id: Uuid,
        key: &str,
        value: &str,
        confidence: f32,
        source: &str,
    ) -> Result<AgentFact, sqlx::Error> {
        let id = Uuid::new_v4();
        let row = sqlx::query(
            r#"
            INSERT INTO agent_facts (id, agent_id, user_id, key, value, confidence, source)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (agent_id, user_id, key)
            DO UPDATE SET value = EXCLUDED.value,
                          confidence = EXCLUDED.confidence,
                          source = EXCLUDED.source,
                          updated_at = now()
            RETURNING id, agent_id, user_id, key, value, confidence, source, updated_at
            "#,
        )
        .bind(id)
        .bind(agent_id)
        .bind(user_id)
        .bind(key)
        .bind(value)
        .bind(confidence)
        .bind(source)
        .fetch_one(&self.pool)
        .await?;

        Ok(row_to_fact(&row))
    }

    /// List facts for an `(agent_id, user_id)` pair, optionally filtered by
    /// substring match on key or value.
    pub async fn list_facts(
        &self,
        agent_id: &str,
        user_id: Uuid,
        query: Option<&str>,
    ) -> Result<Vec<AgentFact>, sqlx::Error> {
        let needle = query.map(|q| format!("%{}%", q.to_lowercase()));
        let rows = sqlx::query(
            r#"
            SELECT id, agent_id, user_id, key, value, confidence, source, updated_at
            FROM agent_facts
            WHERE agent_id = $1 AND user_id = $2
              AND ($3::text IS NULL
                   OR lower(key) LIKE $3
                   OR lower(value) LIKE $3)
            ORDER BY updated_at DESC
            "#,
        )
        .bind(agent_id)
        .bind(user_id)
        .bind(needle.as_deref())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.iter().map(row_to_fact).collect())
    }
}

// ---------------------------------------------------------------------------
// Row decoders
// ---------------------------------------------------------------------------

fn row_to_session(row: &sqlx::postgres::PgRow) -> AgentSession {
    let metadata: SqlxJson<Value> = row
        .try_get("metadata")
        .unwrap_or_else(|_| SqlxJson(json!({})));
    AgentSession {
        id: row.try_get("id").unwrap_or_else(|_| Uuid::nil()),
        user_id: row.try_get("user_id").unwrap_or_else(|_| Uuid::nil()),
        agent_id: row.try_get("agent_id").unwrap_or_default(),
        model: row.try_get("model").ok(),
        metadata: metadata.0,
        created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
        updated_at: row.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
        final_summary: row.try_get("final_summary").ok(),
    }
}

fn row_to_entry(row: &sqlx::postgres::PgRow) -> MemoryEntry {
    let metadata: SqlxJson<Value> = row
        .try_get("metadata")
        .unwrap_or_else(|_| SqlxJson(json!({})));
    let tier_str: String = row.try_get("tier").unwrap_or_default();
    let role_str: String = row.try_get("role").unwrap_or_default();
    MemoryEntry {
        id: row.try_get("id").unwrap_or_else(|_| Uuid::nil()),
        session_id: row.try_get("session_id").unwrap_or_else(|_| Uuid::nil()),
        tier: MemoryTier::parse(&tier_str).unwrap_or(MemoryTier::Episodic),
        role: MemoryRole::parse(&role_str).unwrap_or(MemoryRole::User),
        content: row.try_get("content").unwrap_or_default(),
        token_count: row.try_get("token_count").unwrap_or(0),
        metadata: metadata.0,
        created_at: row.try_get("created_at").unwrap_or_else(|_| Utc::now()),
    }
}

fn row_to_fact(row: &sqlx::postgres::PgRow) -> AgentFact {
    AgentFact {
        id: row.try_get("id").unwrap_or_else(|_| Uuid::nil()),
        agent_id: row.try_get("agent_id").unwrap_or_default(),
        user_id: row.try_get("user_id").unwrap_or_else(|_| Uuid::nil()),
        key: row.try_get("key").unwrap_or_default(),
        value: row.try_get("value").unwrap_or_default(),
        confidence: row.try_get("confidence").unwrap_or(1.0),
        source: row.try_get("source").unwrap_or_else(|_| "explicit".into()),
        updated_at: row.try_get("updated_at").unwrap_or_else(|_| Utc::now()),
    }
}

/// Convenience — count tokens for a content string using the supplied counter.
pub fn count_tokens(counter: &TiktokenCounter, content: &str) -> i32 {
    counter.count(content) as i32
}
