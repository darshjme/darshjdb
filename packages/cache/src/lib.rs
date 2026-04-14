// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache: crate root. Exposes:
//
//   * L1 DashMap cache (Slice 8) — typed entries, hash/list/zset/bloom/HLL.
//   * L2 Postgres persistent tier (Slice 9).
//   * Bytes-keyed L1/L2 scaffolds (Slice 10) — backing the unified layer.
//   * `PubSubEngine` — tokio broadcast fan-out (Slice 10).
//   * `DdbCache` — unified read-through / write-through L1+L2 composition
//     with Prometheus metrics (Slice 10).
//
// Author: Darshankumar Joshi

pub mod l1;
pub mod l1_bytes;
pub mod l2;
pub mod l2_bytes;
pub mod pubsub;
pub mod unified;

// Full L1/L2 public surface from Slices 8 & 9.
pub use l1::{CacheEntry, CacheError, CacheStats, EntryKind, L1Cache};
pub use l2::{L2Cache, L2Error, L2Result, StreamEntry};

// Slice 10 unified layer + its Bytes-keyed tiers and pub/sub.
pub use l1_bytes::BytesL1Cache;
pub use l2_bytes::BytesL2Cache;
pub use pubsub::{PubSubEngine, PubSubMessage};
pub use unified::DdbCache;
