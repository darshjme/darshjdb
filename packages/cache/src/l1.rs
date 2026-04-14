// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache::l1 — L1 in-memory DashMap cache (Slice 8 scaffold).
//
// This is a minimal scaffold used while Slices 8 (full L1) and 10 (this
// unified layer) land in parallel. Once Slice 8 merges, this file will be
// replaced by the full implementation (hash/list/zset/bloom/HLL, TTLs,
// eviction, compression). The public API below is deliberately small and
// matches the subset that Slice 10 depends on, so the merge is mechanical.
//
// Author: Darshankumar Joshi

use bytes::Bytes;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// L1 in-memory cache tier. Thread-safe via `DashMap`.
#[derive(Debug)]
pub struct L1Cache {
    inner: DashMap<String, Bytes>,
    approx_bytes: AtomicU64,
}

impl Default for L1Cache {
    fn default() -> Self {
        Self {
            inner: DashMap::new(),
            approx_bytes: AtomicU64::new(0),
        }
    }
}

impl L1Cache {
    /// Create a new empty L1 cache wrapped in an `Arc` for cheap sharing.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Create a new empty L1 cache with an expected capacity hint.
    pub fn with_capacity(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            inner: DashMap::with_capacity(capacity),
            approx_bytes: AtomicU64::new(0),
        })
    }

    /// Read a value from the L1 cache. Returns `None` on miss.
    pub async fn get(&self, key: &str) -> Option<Bytes> {
        self.inner.get(key).map(|entry| entry.value().clone())
    }

    /// Write a value into the L1 cache.
    pub async fn set(&self, key: &str, value: Bytes) {
        let added = value.len() as u64;
        if let Some(old) = self.inner.insert(key.to_string(), value) {
            self.approx_bytes
                .fetch_sub(old.len() as u64, Ordering::Relaxed);
        }
        self.approx_bytes.fetch_add(added, Ordering::Relaxed);
    }

    /// Delete a key from the L1 cache. Returns `true` if the key was present.
    pub async fn delete(&self, key: &str) -> bool {
        if let Some((_, old)) = self.inner.remove(key) {
            self.approx_bytes
                .fetch_sub(old.len() as u64, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// Current number of entries.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` if no entries are present.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Approximate resident bytes (sum of value lengths).
    pub fn memory_bytes(&self) -> u64 {
        self.approx_bytes.load(Ordering::Relaxed)
    }

    /// Remove all entries.
    pub async fn clear(&self) {
        self.inner.clear();
        self.approx_bytes.store(0, Ordering::Relaxed);
    }
}
