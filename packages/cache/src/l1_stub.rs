// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// l1_stub: temporary placeholder for the L1 DashMap cache (Slice 8).
//
// This stub exists only so the L2 module on Slice 9 can build and run its
// integration tests without depending on the unmerged Slice 8 work. The final
// merge will delete this file and re-export the real `l1` module.

use std::time::Duration;

/// Marker representing the entry kind tag stored alongside cached values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    String,
    Hash,
    List,
    ZSet,
    Stream,
}

impl EntryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::String => "string",
            EntryKind::Hash => "hash",
            EntryKind::List => "list",
            EntryKind::ZSet => "zset",
            EntryKind::Stream => "stream",
        }
    }
}

/// A single cache entry value (string-only in the stub).
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub value: Vec<u8>,
    pub kind: EntryKind,
    pub ttl: Option<Duration>,
}

/// Minimal cache stats placeholder.
#[derive(Debug, Default, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub keys: u64,
}

/// L1 stub — intentionally inert. The real Slice 8 implementation will replace it.
#[derive(Debug, Default)]
pub struct L1Cache;

impl L1Cache {
    pub fn new() -> Self {
        Self
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats::default()
    }
}
