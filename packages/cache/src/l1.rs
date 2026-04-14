// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache :: L1 — minimal stub kept to satisfy the module tree that
// Slices 8-10 will flesh out. Slice 11 uses [`crate::DdbCache`] directly,
// so the real L1 implementation lands in an earlier slice.

use serde::{Deserialize, Serialize};

/// Kind of value stored in a single cache entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryKind {
    String,
    Hash,
    List,
    Set,
    ZSet,
    Stream,
    Bloom,
    HyperLogLog,
}

/// Stub entry — superseded by the real L1 implementation.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub key: String,
    pub kind: EntryKind,
}

/// Stub stats — superseded by the real L1 implementation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CacheStats {
    pub entries: u64,
    pub hits: u64,
    pub misses: u64,
}

/// Placeholder L1 cache type.
#[derive(Debug, Default)]
pub struct L1Cache;

impl L1Cache {
    pub fn new() -> Self {
        Self
    }
}
