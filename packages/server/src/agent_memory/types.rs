// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Shared agent-memory types used by the repo, context builder, and REST layer.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Tier classification for a stored memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryTier {
    /// Hot RAM-only working set for the current conversation.
    Working,
    /// Postgres-backed durable conversation log.
    Episodic,
    /// Distilled / summarised long-term knowledge.
    Semantic,
}

impl MemoryTier {
    /// String label used in the database `tier` column and JSON payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
        }
    }

    /// Parse a database/JSON label into a tier value.
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "working" => Some(Self::Working),
            "episodic" => Some(Self::Episodic),
            "semantic" => Some(Self::Semantic),
            _ => None,
        }
    }
}

/// OpenAI-style chat role for a memory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryRole {
    /// System-level instruction (persona, rules, recalled context blocks).
    System,
    /// End-user turn.
    User,
    /// Assistant / agent turn.
    Assistant,
    /// Tool / function call result.
    Tool,
}

impl MemoryRole {
    /// String label used in the database `role` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }

    /// Parse a database/JSON label into a role value.
    pub fn parse(input: &str) -> Option<Self> {
        match input {
            "system" => Some(Self::System),
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

/// Persistent agent session record (one row in `agent_sessions`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    /// Stable identifier for this conversation thread.
    pub id: Uuid,
    /// Owner — always set from the authenticated `AuthContext`.
    pub user_id: Uuid,
    /// Logical agent identifier (e.g., "support-bot", "code-reviewer").
    pub agent_id: String,
    /// Optional model hint (used by the token counter to pick a BPE).
    pub model: Option<String>,
    /// Free-form metadata blob — system prompt lives under `metadata.system`.
    pub metadata: serde_json::Value,
    /// Wall-clock creation time.
    pub created_at: DateTime<Utc>,
    /// Last activity timestamp (updated on every message insert).
    pub updated_at: DateTime<Utc>,
    /// Optional final summary written when the session is closed.
    pub final_summary: Option<String>,
}

/// A single memory entry — either a chat message or a distilled fact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Primary key.
    pub id: Uuid,
    /// Owning session.
    pub session_id: Uuid,
    /// Tier this entry currently lives in.
    pub tier: MemoryTier,
    /// Chat role.
    pub role: MemoryRole,
    /// Raw textual content.
    pub content: String,
    /// Cached token count (computed on insert).
    pub token_count: i32,
    /// Optional metadata (tool name, retrieval source, etc.).
    pub metadata: serde_json::Value,
    /// Insertion timestamp.
    pub created_at: DateTime<Utc>,
}

/// A piece of long-lived agent knowledge keyed by `(agent_id, user_id, key)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentFact {
    /// Primary key.
    pub id: Uuid,
    /// Logical agent the fact belongs to.
    pub agent_id: String,
    /// User the fact is about.
    pub user_id: Uuid,
    /// Stable key (e.g., `"timezone"`, `"preferences.theme"`).
    pub key: String,
    /// Free-form value (string, JSON-encoded structure, etc.).
    pub value: String,
    /// Optional confidence in `[0, 1]`.
    pub confidence: f32,
    /// Source of the fact (`"explicit"`, `"inferred"`, `"summary"`).
    pub source: String,
    /// Last-updated timestamp.
    pub updated_at: DateTime<Utc>,
}

/// OpenAI chat-completion-shaped message ready to be sent to a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    /// Chat role (`"system"`, `"user"`, `"assistant"`, `"tool"`).
    pub role: String,
    /// Textual content.
    pub content: String,
    /// Cached token count for this message.
    #[serde(default)]
    pub token_count: i32,
    /// Provenance tier (`"working"`, `"episodic"`, `"semantic"`,
    /// `"recalled"`, `"facts"`, or `"system_prompt"`).
    #[serde(default)]
    pub tier: String,
}

/// Aggregate counts returned by the `/stats` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStats {
    /// Session being reported.
    pub session_id: Uuid,
    /// Total number of messages in the working tier (RAM).
    pub working_messages: usize,
    /// Total number of messages in the episodic tier (Postgres).
    pub episodic_messages: usize,
    /// Total number of messages in the semantic tier (Postgres).
    pub semantic_messages: usize,
    /// Sum of `token_count` across every tier.
    pub total_tokens: i64,
    /// Wall-clock creation time of the session.
    pub created_at: DateTime<Utc>,
    /// Most recent activity time.
    pub updated_at: DateTime<Utc>,
}

/// Filter specification for the `/timeline` query.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TimelineFilter {
    /// Optional inclusive lower-bound timestamp.
    pub from: Option<DateTime<Utc>>,
    /// Optional inclusive upper-bound timestamp.
    pub to: Option<DateTime<Utc>>,
    /// Optional tier filter.
    pub tier: Option<String>,
    /// Maximum rows to return (defaults to 100, hard-capped at 1000).
    pub limit: Option<usize>,
}
