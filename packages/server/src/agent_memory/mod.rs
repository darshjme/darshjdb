// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)//
//! Agent memory subsystem for DarshJDB.
//!
//! Implements a tiered memory architecture for LLM agents — the same
//! pattern Mohini and OpenClaw use — but built directly on Postgres so
//! every DarshJDB deployment ships with first-class agent memory out of
//! the box.
//!
//! # Tier model
//!
//! | Tier      | Storage        | Recency           | Purpose                              |
//! |-----------|----------------|-------------------|--------------------------------------|
//! | `working` | DashMap (RAM)  | last N turns      | Hot scratch context for current chat |
//! | `episodic`| Postgres rows  | days–weeks        | Replayable conversation log          |
//! | `semantic`| Postgres + emb | summarised facts  | Long-term distilled knowledge        |
//!
//! # Slice scope
//!
//! - **Slice 12** (scaffolded here) — Schema bootstrap, working tier,
//!   message ingestion, embedding hooks, repository operations.
//! - **Slice 13** (this slice) — `ContextBuilder` token-budgeted prompt
//!   assembly + REST API exposing the full surface to clients.

pub mod context;
pub mod handlers;
pub mod repo;
pub mod schema;
pub mod tokens;
pub mod types;
pub mod working;

#[cfg(test)]
mod tests;

pub use context::{ContextBuildOptions, ContextBuilder, ContextBundle};
pub use handlers::{AgentMemoryState, agent_memory_routes};
pub use repo::AgentMemoryRepo;
pub use schema::ensure_agent_memory_schema;
pub use tokens::TiktokenCounter;
pub use types::{
    AgentFact, AgentSession, ContextMessage, MemoryEntry, MemoryRole, MemoryTier, SessionStats,
    TimelineFilter,
};
pub use working::WorkingMemory;
