// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// Four-tier memory management: working → episodic → semantic → archival.
//
// Design:
//   * Working tier is in-process, capped at WORKING_CAPACITY entries per
//     session. When it overflows the oldest entries are flushed to Postgres
//     at tier='episodic' in one parameterised batch INSERT.
//   * Episodic tier lives in Postgres. When its per-agent count exceeds
//     EPISODIC_CAPACITY, we score every episodic row using
//     `score_entry` (importance × Ebbinghaus decay + log-smoothed access
//     count) and move the bottom SEMANTIC_BOTTOM_FRACTION to 'semantic',
//     and the bottom ARCHIVAL_BOTTOM_FRACTION to 'archival' (also zstd
//     compressing the content column in place).
//   * `update_importance` is a pure function used by the reader path to
//     age/reinforce an entry's importance on each access.
//
// All DB writes use parameterised SQL — never format!() — to avoid injection
// and to keep the plan cache hot.

use std::collections::VecDeque;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum number of entries retained in the in-process working tier per session.
pub const WORKING_CAPACITY: usize = 20;

/// Episodic-tier soft cap per agent. Crossing this triggers promote/demote.
pub const EPISODIC_CAPACITY: usize = 200;

/// Bottom fraction of scored episodic rows that are demoted to `semantic`.
pub const SEMANTIC_BOTTOM_FRACTION: f64 = 0.20;

/// Bottom fraction of scored episodic rows that are demoted to `archival`
/// *and* compressed with zstd. Archival is a subset of the semantic sweep,
/// so this must be ≤ SEMANTIC_BOTTOM_FRACTION.
pub const ARCHIVAL_BOTTOM_FRACTION: f64 = 0.05;

/// Importance decay rate (per hour) for the Ebbinghaus-style forgetting curve
/// used by `score_entry`.
pub const FORGETTING_DECAY_PER_HOUR: f64 = 0.1;

/// Importance decay rate (per hour) used by `update_importance` for
/// recency-only decay (slower than the global score).
pub const IMPORTANCE_DECAY_PER_HOUR: f64 = 0.05;

/// Weight on the log(1 + access_count) term inside `score_entry`.
pub const ACCESS_COUNT_WEIGHT: f64 = 0.3;

/// Zstd compression level used when archiving. Level 3 matches Postgres'
/// TOAST compression tradeoff (fast + reasonable ratio).
pub const ARCHIVAL_ZSTD_LEVEL: i32 = 3;

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryRole {
    User,
    Assistant,
    System,
    Tool,
    Summary,
}

impl MemoryRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryRole::User => "user",
            MemoryRole::Assistant => "assistant",
            MemoryRole::System => "system",
            MemoryRole::Tool => "tool",
            MemoryRole::Summary => "summary",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryTier {
    Working,
    Episodic,
    Semantic,
    Archival,
}

impl MemoryTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryTier::Working => "working",
            MemoryTier::Episodic => "episodic",
            MemoryTier::Semantic => "semantic",
            MemoryTier::Archival => "archival",
        }
    }
}

/// In-memory mirror of a row in the `memory_entries` table.
/// Kept as plain data so scoring/promotion logic can be unit-tested without a pool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Uuid,
    pub session_id: Uuid,
    pub agent_id: String,
    pub role: MemoryRole,
    pub content: String,
    pub content_tokens: i32,
    pub importance: f64,
    pub tier: MemoryTier,
    pub summary: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_output: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
    pub access_count: i32,
    pub compressed: bool,
}

impl MemoryEntry {
    /// Age of the entry, in fractional hours, at the given "now".
    pub fn age_hours(&self, now: DateTime<Utc>) -> f64 {
        let delta = now - self.created_at;
        (delta.num_milliseconds() as f64) / (1000.0 * 3600.0)
    }

    /// Hours since last access.
    pub fn idle_hours(&self, now: DateTime<Utc>) -> f64 {
        let delta = now - self.accessed_at;
        (delta.num_milliseconds() as f64) / (1000.0 * 3600.0)
    }
}

/// Per-session ring buffer of working-tier entries.
///
/// Cloning the `WorkingTier` is cheap — it clones the `Arc` around the
/// inner DashMap, so all handles see the same buffer.
#[derive(Debug, Clone, Default)]
pub struct WorkingTier {
    inner: Arc<DashMap<Uuid, VecDeque<MemoryEntry>>>,
}

impl WorkingTier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push an entry into the working buffer for a session. Returns the
    /// list of entries that were evicted (FIFO) because the session
    /// exceeded `WORKING_CAPACITY`. These evictions are what the caller
    /// batch-INSERTs into the episodic tier in Postgres.
    pub fn push(&self, entry: MemoryEntry) -> Vec<MemoryEntry> {
        let mut evicted = Vec::new();
        let mut buf = self
            .inner
            .entry(entry.session_id)
            .or_insert_with(VecDeque::new);
        buf.push_back(entry);
        while buf.len() > WORKING_CAPACITY {
            if let Some(old) = buf.pop_front() {
                evicted.push(old);
            }
        }
        evicted
    }

    /// Number of entries currently held for a session (0 if unknown).
    pub fn len(&self, session_id: Uuid) -> usize {
        self.inner
            .get(&session_id)
            .map(|b| b.len())
            .unwrap_or(0)
    }

    /// Returns `true` when the session has no working-tier rows.
    pub fn is_empty(&self, session_id: Uuid) -> bool {
        self.len(session_id) == 0
    }

    /// Drain every working-tier entry for a session (used at session close).
    pub fn drain_session(&self, session_id: Uuid) -> Vec<MemoryEntry> {
        self.inner
            .remove(&session_id)
            .map(|(_, buf)| buf.into_iter().collect())
            .unwrap_or_default()
    }
}

/// Report returned by `promote_demote` so callers (and tests) can verify
/// how many rows moved across tiers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PromotionReport {
    pub working_flushed: usize,
    pub demoted_to_semantic: usize,
    pub demoted_to_archival: usize,
}

// ── Pure scoring functions (unit-testable without a DB) ─────────────────

/// Compute the retention score of an episodic entry at time `now`.
///
/// score = importance · exp(−decay · age_hours) + w · ln(1 + access_count)
///
/// Higher is better; the tier sweep demotes the *bottom* N%.
pub fn score_entry(entry: &MemoryEntry, now: DateTime<Utc>) -> f64 {
    let age = entry.age_hours(now).max(0.0);
    let decayed_importance = entry.importance * (-FORGETTING_DECAY_PER_HOUR * age).exp();
    let access_term = ACCESS_COUNT_WEIGHT * (1.0 + entry.access_count.max(0) as f64).ln();
    decayed_importance + access_term
}

/// Non-destructive update of importance based on fresh feedback.
///
/// * `recency_decay` halves roughly every `ln(2)/IMPORTANCE_DECAY_PER_HOUR ≈ 13.9h`.
/// * Each access contributes `+0.05` capped by the decay.
/// * `feedback_delta` lets upper layers inject a tuning signal (e.g. the
///   user clicked "pin this"). It is clamped to `[-0.5, 0.5]` before
///   being added so a single call cannot saturate the importance.
/// * Final importance is clamped to `[0.0, 1.0]`.
pub fn update_importance(
    entry: &MemoryEntry,
    now: DateTime<Utc>,
    feedback_delta: f64,
) -> f64 {
    let idle = entry.idle_hours(now).max(0.0);
    let decay = (-IMPORTANCE_DECAY_PER_HOUR * idle).exp();
    let freq_boost = 0.05 * (1.0 + entry.access_count.max(0) as f64).ln();
    let clamped_feedback = feedback_delta.clamp(-0.5, 0.5);
    let raw = entry.importance * decay + freq_boost + clamped_feedback;
    raw.clamp(0.0, 1.0)
}

/// Pure helper extracted from `promote_demote`: given a batch of episodic
/// entries, partition them into (kept, semantic, archival) buckets using
/// the score function. Returns *ids* (not full rows) because the caller
/// only needs to UPDATE by id.
///
/// The ordering is deterministic: ties are broken by `id` so tests are
/// reproducible.
pub fn plan_episodic_demotion(
    entries: &[MemoryEntry],
    now: DateTime<Utc>,
) -> (Vec<Uuid>, Vec<Uuid>) {
    let mut scored: Vec<(f64, Uuid)> = entries
        .iter()
        .map(|e| (score_entry(e, now), e.id))
        .collect();
    // Sort ascending so the *worst* entries come first.
    scored.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });

    let n = scored.len();
    let n_semantic = ((n as f64) * SEMANTIC_BOTTOM_FRACTION).floor() as usize;
    let n_archival = ((n as f64) * ARCHIVAL_BOTTOM_FRACTION).floor() as usize;

    // Archival rows are also semantic-eligible, but we want the archival
    // set to be the very bottom slice so they don't double-count.
    let archival_ids: Vec<Uuid> = scored.iter().take(n_archival).map(|(_, id)| *id).collect();
    let semantic_ids: Vec<Uuid> = scored
        .iter()
        .skip(n_archival)
        .take(n_semantic.saturating_sub(n_archival))
        .map(|(_, id)| *id)
        .collect();

    (semantic_ids, archival_ids)
}

/// Compress an archival entry's content in place using zstd at
/// `ARCHIVAL_ZSTD_LEVEL`. Returns the compressed byte vector.
pub fn compress_archival(content: &str) -> Vec<u8> {
    zstd::bulk::compress(content.as_bytes(), ARCHIVAL_ZSTD_LEVEL)
        .unwrap_or_else(|_| content.as_bytes().to_vec())
}

// ── DB-backed promotion/demotion ────────────────────────────────────────

/// Error type returned by the DB-touching `promote_demote` call.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Flush working-tier overflow and sweep the episodic tier, moving the
/// lowest-scoring rows to `semantic` and the very lowest to `archival`
/// (compressing their content with zstd on the way).
///
/// This function is a thin, testable orchestration around the pure
/// helpers above. All SQL is parameterised.
pub async fn promote_demote(
    working: &WorkingTier,
    session_id: Uuid,
    agent_id: &str,
    pool: &sqlx::PgPool,
) -> Result<PromotionReport, MemoryError> {
    let mut report = PromotionReport::default();

    // 1. Flush any working-tier overflow that a previous `push` left behind.
    //    (Callers normally flush on the return value of `push`; this is a
    //    belt-and-braces sweep so session-close can't leak rows.)
    let session_buf = working.drain_session(session_id);
    if !session_buf.is_empty() {
        report.working_flushed = session_buf.len();
        let mut tx = pool.begin().await?;
        for entry in &session_buf {
            sqlx::query(
                "INSERT INTO memory_entries
                    (id, session_id, agent_id, role, content, content_tokens,
                     importance, tier, summary, tool_name, tool_input, tool_output,
                     created_at, accessed_at, access_count, compressed)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, 'episodic',
                         $8, $9, $10, $11, $12, $13, $14, false)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(entry.id)
            .bind(entry.session_id)
            .bind(&entry.agent_id)
            .bind(entry.role.as_str())
            .bind(&entry.content)
            .bind(entry.content_tokens)
            .bind(entry.importance)
            .bind(&entry.summary)
            .bind(&entry.tool_name)
            .bind(&entry.tool_input)
            .bind(&entry.tool_output)
            .bind(entry.created_at)
            .bind(entry.accessed_at)
            .bind(entry.access_count)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
    }

    // 2. If the episodic tier is over-capacity for this agent, score every
    //    episodic row and demote the bottom N%.
    let episodic_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_entries
          WHERE agent_id = $1 AND tier = 'episodic'",
    )
    .bind(agent_id)
    .fetch_one(pool)
    .await?;

    if (episodic_count as usize) > EPISODIC_CAPACITY {
        // Fetch only the columns score_entry needs.
        let rows: Vec<(Uuid, f64, i32, DateTime<Utc>, String)> = sqlx::query_as(
            "SELECT id, importance, access_count, created_at, content
               FROM memory_entries
              WHERE agent_id = $1 AND tier = 'episodic'",
        )
        .bind(agent_id)
        .fetch_all(pool)
        .await?;

        let now = Utc::now();
        let entries: Vec<MemoryEntry> = rows
            .into_iter()
            .map(
                |(id, importance, access_count, created_at, content)| MemoryEntry {
                    id,
                    session_id: Uuid::nil(),
                    agent_id: agent_id.to_string(),
                    role: MemoryRole::Assistant,
                    content,
                    content_tokens: 0,
                    importance,
                    tier: MemoryTier::Episodic,
                    summary: None,
                    tool_name: None,
                    tool_input: None,
                    tool_output: None,
                    created_at,
                    accessed_at: created_at,
                    access_count,
                    compressed: false,
                },
            )
            .collect();

        let (semantic_ids, archival_ids) = plan_episodic_demotion(&entries, now);
        report.demoted_to_semantic = semantic_ids.len();
        report.demoted_to_archival = archival_ids.len();

        let mut tx = pool.begin().await?;

        if !semantic_ids.is_empty() {
            sqlx::query(
                "UPDATE memory_entries
                    SET tier = 'semantic'
                  WHERE id = ANY($1)",
            )
            .bind(&semantic_ids)
            .execute(&mut *tx)
            .await?;
        }

        for id in &archival_ids {
            // Compress content, then UPDATE the row with the compressed
            // bytes stored as base64 text so the existing TEXT column
            // stays strict-UTF8-safe without another schema change.
            let existing: Option<String> = sqlx::query_scalar(
                "SELECT content FROM memory_entries WHERE id = $1",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(content) = existing {
                let compressed = compress_archival(&content);
                // base64 keeps it TEXT-compatible. Decompression happens
                // on read via the symmetrical b64 → zstd pipeline.
                let b64 = base64_encode(&compressed);
                sqlx::query(
                    "UPDATE memory_entries
                        SET tier = 'archival',
                            content = $1,
                            compressed = true
                      WHERE id = $2",
                )
                .bind(b64)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;
    }

    metrics::counter!("ddb_agent_memory_working_flushed")
        .increment(report.working_flushed as u64);
    metrics::counter!("ddb_agent_memory_demoted_semantic")
        .increment(report.demoted_to_semantic as u64);
    metrics::counter!("ddb_agent_memory_demoted_archival")
        .increment(report.demoted_to_archival as u64);

    Ok(report)
}

/// Minimal base64 encoder so we don't drag in the full `base64` crate for
/// a single call site. Standard alphabet, no padding stripping.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= bytes.len() {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8) | (bytes[i + 2] as u32);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push(ALPHABET[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let n = (bytes[i] as u32) << 16;
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((bytes[i] as u32) << 16) | ((bytes[i + 1] as u32) << 8);
        out.push(ALPHABET[((n >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3F) as usize] as char);
        out.push(ALPHABET[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

// ── Unit tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_entry(importance: f64, age_hours: i64, access_count: i32) -> MemoryEntry {
        let now = Utc::now();
        let created = now - Duration::hours(age_hours);
        MemoryEntry {
            id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            agent_id: "test-agent".into(),
            role: MemoryRole::User,
            content: "hello world".into(),
            content_tokens: 2,
            importance,
            tier: MemoryTier::Episodic,
            summary: None,
            tool_name: None,
            tool_input: None,
            tool_output: None,
            created_at: created,
            accessed_at: created,
            access_count,
            compressed: false,
        }
    }

    #[test]
    fn role_and_tier_strings_round_trip_with_db_checks() {
        for r in [
            MemoryRole::User,
            MemoryRole::Assistant,
            MemoryRole::System,
            MemoryRole::Tool,
            MemoryRole::Summary,
        ] {
            // Every variant must match the CHECK constraint in the migration.
            assert!(["user", "assistant", "system", "tool", "summary"].contains(&r.as_str()));
        }
        for t in [
            MemoryTier::Working,
            MemoryTier::Episodic,
            MemoryTier::Semantic,
            MemoryTier::Archival,
        ] {
            assert!(["working", "episodic", "semantic", "archival"].contains(&t.as_str()));
        }
    }

    #[test]
    fn score_decays_with_age() {
        let now = Utc::now();
        let fresh = make_entry(1.0, 0, 0);
        let aged = make_entry(1.0, 10, 0);
        let ancient = make_entry(1.0, 100, 0);

        let s_fresh = score_entry(&fresh, now);
        let s_aged = score_entry(&aged, now);
        let s_ancient = score_entry(&ancient, now);

        assert!(s_fresh > s_aged, "fresh {} > aged {}", s_fresh, s_aged);
        assert!(
            s_aged > s_ancient,
            "aged {} > ancient {}",
            s_aged,
            s_ancient
        );
        // Ebbinghaus at 10h with decay 0.1: exp(-1.0) ≈ 0.3679.
        assert!((s_aged - (-1.0_f64).exp()).abs() < 1e-6);
    }

    #[test]
    fn access_count_boosts_score() {
        let now = Utc::now();
        let cold = make_entry(0.5, 5, 0);
        let hot = make_entry(0.5, 5, 100);
        let cold_score = score_entry(&cold, now);
        let hot_score = score_entry(&hot, now);
        assert!(hot_score > cold_score);
        // Access term should lift by exactly 0.3 * ln(101).
        let diff = hot_score - cold_score;
        let expected = ACCESS_COUNT_WEIGHT * 101f64.ln();
        assert!((diff - expected).abs() < 1e-9, "got {} want {}", diff, expected);
    }

    #[test]
    fn importance_stays_in_unit_interval() {
        let now = Utc::now();

        // Feedback far above the clamp should saturate at 1.0.
        let entry = make_entry(0.9, 1, 3);
        let up = update_importance(&entry, now, 10.0);
        assert!((0.0..=1.0).contains(&up));
        assert!(up >= 0.9);

        // Feedback far below the clamp should drive toward 0, never negative.
        let down = update_importance(&entry, now, -10.0);
        assert!((0.0..=1.0).contains(&down));
        assert!(down < entry.importance);
    }

    #[test]
    fn importance_decays_with_idleness() {
        let now = Utc::now();
        let recent = make_entry(0.8, 0, 0);
        let stale = {
            let mut e = make_entry(0.8, 0, 0);
            e.accessed_at = now - Duration::hours(48);
            e
        };
        let r = update_importance(&recent, now, 0.0);
        let s = update_importance(&stale, now, 0.0);
        assert!(r > s, "recent {} should exceed stale {}", r, s);
    }

    #[test]
    fn working_tier_evicts_fifo_at_capacity() {
        let working = WorkingTier::new();
        let session_id = Uuid::new_v4();
        let mut ids = Vec::new();

        // Push capacity + 5 entries; expect exactly 5 evictions and
        // they should be the first 5 inserted (FIFO).
        for i in 0..(WORKING_CAPACITY + 5) {
            let mut e = make_entry(0.5, 0, 0);
            e.session_id = session_id;
            e.content = format!("msg-{i}");
            ids.push(e.id);
            let evicted = working.push(e);
            if i < WORKING_CAPACITY {
                assert!(evicted.is_empty());
            } else {
                assert_eq!(evicted.len(), 1);
                assert_eq!(evicted[0].id, ids[i - WORKING_CAPACITY]);
            }
        }

        assert_eq!(working.len(session_id), WORKING_CAPACITY);
        let drained = working.drain_session(session_id);
        assert_eq!(drained.len(), WORKING_CAPACITY);
        assert!(working.is_empty(session_id));
    }

    #[test]
    fn plan_episodic_demotion_uses_correct_fractions() {
        let now = Utc::now();
        // 100 entries, varying importance → 20% semantic / 5% archival.
        let mut entries: Vec<MemoryEntry> = (0..100)
            .map(|i| make_entry((i as f64) / 100.0, 1, 0))
            .collect();
        // Shuffle ordering deterministically to simulate DB result order.
        entries.reverse();

        let (semantic, archival) = plan_episodic_demotion(&entries, now);
        assert_eq!(archival.len(), 5, "archival 5% of 100");
        // The plan returns semantic rows *excluding* archival ones, so the
        // set should contain 20 − 5 = 15 ids.
        assert_eq!(semantic.len(), 15, "semantic bucket = 20% − archival");

        // The archival rows must be disjoint from the semantic rows.
        let mut all: std::collections::HashSet<Uuid> = semantic.iter().copied().collect();
        for id in &archival {
            assert!(all.insert(*id), "archival id leaked into semantic");
        }
        assert_eq!(all.len(), 20);
    }

    #[test]
    fn plan_episodic_demotion_picks_lowest_scores() {
        let now = Utc::now();
        // Give entry 0 the highest score, entry 9 the lowest.
        let entries: Vec<MemoryEntry> = (0..10)
            .map(|i| {
                let importance = 1.0 - (i as f64) * 0.05; // 1.0, 0.95, … 0.55
                let age = 1 + i as i64 * 2; // older => worse
                let access_count = (10 - i) as i32; // fewer accesses => worse
                make_entry(importance, age, access_count)
            })
            .collect();

        // 10 × 20% = 2 semantic, 10 × 5% = 0 archival.
        let (semantic, archival) = plan_episodic_demotion(&entries, now);
        assert_eq!(archival.len(), 0);
        assert_eq!(semantic.len(), 2);

        // The two demoted ids must be the *worst-scoring* ones (last two).
        let worst_two = [entries[8].id, entries[9].id];
        for id in &semantic {
            assert!(worst_two.contains(id), "demoted non-worst entry: {id}");
        }
    }

    #[test]
    fn compress_archival_round_trips_via_zstd() {
        let original =
            "the quick brown fox jumps over the lazy dog and writes to darshjdb".repeat(20);
        let compressed = compress_archival(&original);
        assert!(!compressed.is_empty());
        let decompressed =
            zstd::bulk::decompress(&compressed, original.len() * 4).expect("decompress");
        assert_eq!(String::from_utf8(decompressed).unwrap(), original);
    }

    #[test]
    fn base64_encode_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn promotion_thresholds_are_consistent() {
        // Archival must be strictly included inside the semantic sweep
        // so plan_episodic_demotion never double-counts.
        assert!(ARCHIVAL_BOTTOM_FRACTION <= SEMANTIC_BOTTOM_FRACTION);
        assert!(ARCHIVAL_BOTTOM_FRACTION > 0.0);
        assert!(SEMANTIC_BOTTOM_FRACTION < 1.0);
        assert!(WORKING_CAPACITY > 0);
        assert!(EPISODIC_CAPACITY > WORKING_CAPACITY);
    }
}
