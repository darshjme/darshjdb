// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Token-budgeted prompt assembly — the slice 13 deliverable.
//!
//! [`ContextBuilder`] walks the tier hierarchy in priority order:
//!
//! 1. The session's system prompt (from `agent_sessions.metadata.system`).
//! 2. The hot working window (RAM, reverse-chron).
//! 3. The episodic Postgres tier (reverse-chron).
//! 4. (optional) Semantic recall — top-K rows from `memory_entries` whose
//!    content matches the current query, injected as a synthetic
//!    `[RECALLED MEMORY]` system block.
//! 5. (optional) Agent facts — `[AGENT KNOWLEDGE]` system block.
//!
//! Every step subtracts from a shared token budget; assembly stops as
//! soon as the budget is exhausted. The final output is an OpenAI-style
//! `messages[]` slice with provenance preserved in each entry's `tier`
//! field so callers can debug retrieval offline.

use serde_json::Value;
use uuid::Uuid;

use super::repo::AgentMemoryRepo;
use super::tokens::TiktokenCounter;
use super::types::{ContextMessage, MemoryEntry, MemoryRole};
use super::working::WorkingMemory;

/// Caller-tunable knobs for a single [`ContextBuilder::build`] call.
#[derive(Debug, Clone)]
pub struct ContextBuildOptions {
    /// Hard upper bound on total tokens (prompt + recalled + facts).
    pub max_tokens: usize,
    /// Optional user query — when present, semantic recall runs after
    /// the working / episodic tiers and tries to fill any remaining
    /// budget with relevant historical context.
    pub current_query: Option<String>,
    /// Whether to inject the `[AGENT KNOWLEDGE]` facts block.
    pub include_facts: bool,
    /// Top-K limit for the semantic recall step.
    pub recall_top_k: i64,
    /// How many episodic rows to consider before the budget gate.
    pub episodic_limit: i64,
}

impl Default for ContextBuildOptions {
    fn default() -> Self {
        Self {
            max_tokens: 4096,
            current_query: None,
            include_facts: false,
            recall_top_k: 5,
            episodic_limit: 200,
        }
    }
}

/// Result of a context build — the messages plus a tally of how the
/// budget was actually spent.
#[derive(Debug, Clone)]
pub struct ContextBundle {
    /// OpenAI-style messages, in chronological-ish prompt order
    /// (system prompt first, then working/episodic, then recalled,
    /// then facts, finishing with the most recent assistant turn).
    pub messages: Vec<ContextMessage>,
    /// Sum of `token_count` across `messages`.
    pub total_tokens: usize,
    /// `max_tokens - total_tokens`.
    pub budget_remaining: usize,
}

/// Stateful builder bound to a session — clone is cheap.
#[derive(Clone)]
pub struct ContextBuilder {
    session_id: Uuid,
    counter: TiktokenCounter,
    repo: AgentMemoryRepo,
    working: WorkingMemory,
    /// Resolved owning user — populated from the session row.
    user_id: Option<Uuid>,
    /// Resolved agent identifier — populated from the session row.
    agent_id: Option<String>,
}

impl ContextBuilder {
    /// Construct a context builder for a session.
    pub fn new(
        session_id: Uuid,
        counter: TiktokenCounter,
        repo: AgentMemoryRepo,
        working: WorkingMemory,
    ) -> Self {
        Self {
            session_id,
            counter,
            repo,
            working,
            user_id: None,
            agent_id: None,
        }
    }

    /// Run the full assembly pipeline. Returns the messages plus a
    /// budget report.
    pub async fn build(
        &mut self,
        options: &ContextBuildOptions,
    ) -> Result<ContextBundle, sqlx::Error> {
        let mut messages: Vec<ContextMessage> = Vec::new();
        let mut budget = options.max_tokens;

        // ----- Session metadata -----------------------------------------
        // Resolve session metadata so we can extract the system prompt
        // *and* remember the owning user for the facts step.
        let session_metadata: Option<Value> = self.fetch_session_metadata().await?;

        // Step A — system prompt -----------------------------------------
        if let Some(system_prompt) = session_metadata
            .as_ref()
            .and_then(|m| m.get("system"))
            .and_then(|v| v.as_str())
            && !system_prompt.is_empty()
        {
            let toks = self.counter.count(system_prompt);
            if toks <= budget {
                budget -= toks;
                messages.push(ContextMessage {
                    role: "system".into(),
                    content: system_prompt.to_string(),
                    token_count: toks as i32,
                    tier: "system_prompt".into(),
                });
            }
        }

        // Step B — working tier (RAM, reverse-chron) ---------------------
        let mut working_msgs: Vec<ContextMessage> = Vec::new();
        let mut working = self.working.snapshot(self.session_id);
        working.reverse(); // newest first
        for entry in working {
            let toks = entry_token_count(&entry, &self.counter);
            if toks > budget {
                break;
            }
            budget -= toks;
            working_msgs.push(entry_to_message(entry, "working", toks));
        }
        // Restore chronological order at the end.
        working_msgs.reverse();

        // Step C — episodic tier (Postgres, reverse-chron) ---------------
        let mut episodic_msgs: Vec<ContextMessage> = Vec::new();
        if budget > 0 {
            let episodic = self
                .repo
                .recent_in_tier(
                    self.session_id,
                    super::types::MemoryTier::Episodic,
                    options.episodic_limit,
                )
                .await?;
            for entry in episodic {
                let toks = entry_token_count(&entry, &self.counter);
                if toks > budget {
                    break;
                }
                budget -= toks;
                episodic_msgs.push(entry_to_message(entry, "episodic", toks));
            }
            episodic_msgs.reverse();
        }

        // Push working/episodic in chronological order: episodic first
        // (older), then working (newer).
        messages.extend(episodic_msgs);
        messages.extend(working_msgs);

        // Step D — semantic recall ---------------------------------------
        if budget > 0
            && let Some(query) = options.current_query.as_deref()
            && !query.trim().is_empty()
        {
            let recalled = self
                .repo
                .semantic_recall(self.session_id, query, options.recall_top_k)
                .await?;
            if !recalled.is_empty() {
                let mut block = String::from("[RECALLED MEMORY]\n");
                for entry in &recalled {
                    block.push_str("- ");
                    block.push_str(&entry.content);
                    block.push('\n');
                }
                let toks = self.counter.count(&block);
                if toks <= budget {
                    budget -= toks;
                    messages.push(ContextMessage {
                        role: "system".into(),
                        content: block,
                        token_count: toks as i32,
                        tier: "recalled".into(),
                    });
                }
            }
        }

        // Step E — agent facts -------------------------------------------
        if options.include_facts && budget > 0 {
            let facts = self.fetch_facts().await?;
            if !facts.is_empty() {
                let mut block = String::from("[AGENT KNOWLEDGE]\n");
                for fact in &facts {
                    block.push_str(&format!("- {} = {}\n", fact.key, fact.value));
                }
                let toks = self.counter.count(&block);
                if toks <= budget {
                    budget -= toks;
                    messages.push(ContextMessage {
                        role: "system".into(),
                        content: block,
                        token_count: toks as i32,
                        tier: "facts".into(),
                    });
                }
            }
        }

        let total_tokens = options.max_tokens.saturating_sub(budget);
        Ok(ContextBundle {
            messages,
            total_tokens,
            budget_remaining: budget,
        })
    }

    /// Convenience wrapper for the REST handler.
    pub async fn build_default(
        &mut self,
        max_tokens: usize,
        current_query: Option<String>,
        include_facts: bool,
    ) -> Result<ContextBundle, sqlx::Error> {
        self.build(&ContextBuildOptions {
            max_tokens,
            current_query,
            include_facts,
            ..ContextBuildOptions::default()
        })
        .await
    }

    // -------------------------------------------------------------------
    // Internal helpers
    // -------------------------------------------------------------------

    async fn fetch_session_metadata(&mut self) -> Result<Option<Value>, sqlx::Error> {
        // The repo `get_session` requires the user_id; we don't have it
        // yet on first invocation, so query the metadata directly. We
        // capture the owner so subsequent passes can use the typed API.
        use sqlx::Row;
        let row = sqlx::query(
            r#"
            SELECT user_id, agent_id, metadata
            FROM agent_sessions
            WHERE id = $1
            "#,
        )
        .bind(self.session_id)
        .fetch_optional(self.repo.pool())
        .await?;

        if let Some(row) = row {
            self.user_id = row.try_get("user_id").ok();
            self.agent_id = row.try_get("agent_id").ok();
            let meta: sqlx::types::Json<Value> = row
                .try_get("metadata")
                .unwrap_or_else(|_| sqlx::types::Json(serde_json::json!({})));
            Ok(Some(meta.0))
        } else {
            Ok(None)
        }
    }

    async fn fetch_facts(&self) -> Result<Vec<super::types::AgentFact>, sqlx::Error> {
        let (Some(uid), Some(aid)) = (self.user_id, self.agent_id.as_deref()) else {
            return Ok(Vec::new());
        };
        self.repo.list_facts(aid, uid, None).await
    }
}

fn entry_token_count(entry: &MemoryEntry, counter: &TiktokenCounter) -> usize {
    if entry.token_count > 0 {
        entry.token_count as usize
    } else {
        counter.count(&entry.content)
    }
}

fn entry_to_message(entry: MemoryEntry, tier: &str, token_count: usize) -> ContextMessage {
    ContextMessage {
        role: role_str(entry.role),
        content: entry.content,
        token_count: token_count as i32,
        tier: tier.into(),
    }
}

fn role_str(role: MemoryRole) -> String {
    role.as_str().to_string()
}
