// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-agent-memory: crate root. Four-tier agent memory
// (working / episodic / semantic / archival) with importance scoring,
// plus pluggable embedding providers (Slice 16) and a background worker
// that fills `embedding` / `content_tokens` columns on `memory_entries`
// and `agent_facts` rows.
//
// Slices 12-16 of the Grand Transformation.
//
// Tiers are progressed by `tiers::promote_demote` and rows are scored
// by `tiers::score_entry` using an Ebbinghaus-style forgetting curve
// plus a log-smoothed access count.

#![forbid(unsafe_code)]

//! DarshJDB agent memory — tiered memory store + pluggable embeddings + LLM summariser.

/// Four-tier memory store (working / episodic / semantic / archival) with
/// importance scoring and promotion/demotion.
pub mod tiers;

/// Pluggable embedding providers (OpenAI, Ollama, Anthropic, None).
pub mod embedder;

/// Background worker that fills `embedding` + `content_tokens` columns.
pub mod worker;

/// Episodic-to-semantic summariser with pluggable LLM clients (Slice 15).
pub mod summariser;

pub use summariser::{
    build_llm_client_for_provider, build_llm_client_from_env, count_tokens, format_transcript,
    is_threshold_crossed, maybe_summarise_session, summarise_oldest_episodic, AnthropicClient,
    LlmClient, LlmError, LlmMessage, NoneClient, OpenAiClient, SummariserError,
    NO_LLM_FALLBACK_TEXT, SUMMARISER_BATCH_SIZE, SUMMARISER_IMPORTANCE, SUMMARISER_MAX_TOKENS,
    SUMMARISER_SYSTEM_PROMPT, SUMMARISER_THRESHOLDS,
};
pub use tiers::{
    ARCHIVAL_BOTTOM_FRACTION, EPISODIC_CAPACITY, MemoryEntry, MemoryRole, MemoryTier,
    PromotionReport, SEMANTIC_BOTTOM_FRACTION, WORKING_CAPACITY, WorkingTier, score_entry,
    update_importance,
};
pub use embedder::{
    AnthropicEmbeddingProvider, EmbeddingProvider, NoneProvider, OllamaEmbeddingProvider,
    OpenAIEmbeddingProvider, from_env,
};
pub use worker::{EmbeddingWorkerHandle, embedding_worker, spawn_embedding_worker};
