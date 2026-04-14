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

pub mod tiers;

pub use tiers::{
    score_entry, update_importance, MemoryEntry, MemoryRole, MemoryTier, PromotionReport,
    WorkingTier, ARCHIVAL_BOTTOM_FRACTION, EPISODIC_CAPACITY, SEMANTIC_BOTTOM_FRACTION,
    WORKING_CAPACITY,
};
