// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-agent-memory: crate root. Four-tier agent memory
// (working / episodic / semantic / archival) with importance scoring.
//
// Slice 12/30 of the Grand Transformation.
//
// Tiers are progressed by `tiers::promote_demote` and rows are scored
// by `tiers::score_entry` using an Ebbinghaus-style forgetting curve
// plus a log-smoothed access count.

#![forbid(unsafe_code)]

pub mod summariser;
pub mod tiers;

pub use summariser::{
    build_llm_client_for_provider, build_llm_client_from_env, count_tokens, format_transcript,
    is_threshold_crossed, maybe_summarise_session, summarise_oldest_episodic, AnthropicClient,
    LlmClient, LlmError, LlmMessage, NoneClient, OpenAiClient, SummariserError,
    NO_LLM_FALLBACK_TEXT, SUMMARISER_BATCH_SIZE, SUMMARISER_IMPORTANCE, SUMMARISER_MAX_TOKENS,
    SUMMARISER_SYSTEM_PROMPT, SUMMARISER_THRESHOLDS,
};
pub use tiers::{
    score_entry, update_importance, MemoryEntry, MemoryRole, MemoryTier, PromotionReport,
    WorkingTier, ARCHIVAL_BOTTOM_FRACTION, EPISODIC_CAPACITY, SEMANTIC_BOTTOM_FRACTION,
    WORKING_CAPACITY,
};
