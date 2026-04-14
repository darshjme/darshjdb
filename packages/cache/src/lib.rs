// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache: crate root. Exposes the L1 DashMap cache (Slice 8) and the
// L2 Postgres-backed persistent tier (Slice 9). Later slices plug in the
// unified DdbCache engine and the RESP3 dispatcher.

pub mod l1;
pub mod l2;

pub use l1::{CacheEntry, CacheError, CacheStats, EntryKind, L1Cache};
pub use l2::{L2Cache, L2Error, L2Result, StreamEntry};
