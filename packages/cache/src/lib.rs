// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache: crate root. Exposes:
//
//   * L1 DashMap cache (Slice 8) — typed entries, hash/list/zset/bloom/HLL.
//   * L2 Postgres persistent tier (Slice 9).
//   * Bytes-keyed L1/L2 scaffolds + `DdbUnifiedCache` (Slice 10).
//   * `PubSubEngine` — tokio broadcast fan-out (Slice 10).
//   * `DdbCache` — Slice 11 unified in-process cache engine powering the
//     RESP3 protocol server (ddb-cache-server) and the HTTP REST cache API,
//     with STRING/HASH/LIST/ZSET/STREAM/BLOOM/HLL/pub-sub support.
//
// Author: Darshankumar Joshi

pub mod ddb_cache;
pub mod l1;
pub mod l1_bytes;
pub mod l2;
pub mod l2_bytes;
pub mod pubsub;
pub mod unified;

// Full L1/L2 public surface from Slices 8 & 9.
pub use l1::{CacheEntry, CacheError, EntryKind, L1Cache};
pub use l2::{L2Cache, L2Error, L2Result};

// Slice 10 unified read-through layer + its Bytes-keyed tiers and pub/sub.
pub use l1_bytes::BytesL1Cache;
pub use l2_bytes::BytesL2Cache;
pub use pubsub::{PubSubEngine, PubSubMessage as UnifiedPubSubMessage};
pub use unified::DdbUnifiedCache;

// Slice 11 RESP3 + HTTP cache engine: this is the process-wide `DdbCache`
// the server's `/api/cache/*` router and the RESP3 dispatcher both share.
pub use ddb_cache::{DdbCache, DdbCacheStats, KeyType, PubSubMessage, StreamEntry, glob_match};
