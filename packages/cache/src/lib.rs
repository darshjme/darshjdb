// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache: crate root. Exposes the L1 DashMap cache and (in later slices)
// L2/L3 tiers, the unified DdbCache engine, and the RESP3 dispatcher.

pub mod l1;

pub use l1::{CacheEntry, CacheError, CacheStats, EntryKind, L1Cache};
