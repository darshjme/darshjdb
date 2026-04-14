// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache: crate root. Exposes the L1 stub (to be filled in by sibling
// slices) and the unified [`DdbCache`] engine that powers Slice 11's
// RESP3 protocol server and HTTP REST cache API.

pub mod ddb_cache;
pub mod l1;

pub use ddb_cache::{
    DdbCache, DdbCacheStats, KeyType, PubSubMessage, StreamEntry, glob_match,
};
pub use l1::{CacheEntry, CacheStats, EntryKind, L1Cache};
