// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache::l2 — L2 durable cache tier (Slice 9 scaffold).
//
// Slice 9 delivers a Postgres-backed L2 tier (`kv_store` table) with
// bincode-encoded payloads and WAL-backed durability. While Slice 9 is
// in-flight, this file provides an in-memory stub behind the same async
// API that Slice 10's unified layer consumes. When Slice 9 lands the
// storage backend switches to Postgres without touching the public API.
//
// Author: Darshankumar Joshi

use bytes::Bytes;
use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// L2 durable cache tier. The production backend is Postgres (Slice 9);
/// the scaffolded backend is a DashMap so the unified layer compiles and
/// can be tested independently of the Postgres merge.
#[derive(Debug)]
pub struct BytesL2Cache {
    inner: DashMap<String, Bytes>,
    approx_bytes: AtomicU64,
}

impl Default for BytesL2Cache {
    fn default() -> Self {
        Self {
            inner: DashMap::new(),
            approx_bytes: AtomicU64::new(0),
        }
    }
}

impl BytesL2Cache {
    /// Create an L2 cache with the in-memory scaffold backend.
    ///
    /// When Slice 9 lands, callers will switch to `BytesL2Cache::with_pool(pool)`
    /// which will return a `Result<Arc<Self>>` backed by Postgres. The
    /// in-memory constructor is retained for tests and bootstrap.
    pub fn new_in_memory() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Read a value from L2. Returns `None` on miss.
    pub async fn get(&self, key: &str) -> Option<Bytes> {
        self.inner.get(key).map(|entry| entry.value().clone())
    }

    /// Persist a value into L2.
    pub async fn set(&self, key: &str, value: Bytes) {
        let added = value.len() as u64;
        if let Some(old) = self.inner.insert(key.to_string(), value) {
            self.approx_bytes
                .fetch_sub(old.len() as u64, Ordering::Relaxed);
        }
        self.approx_bytes.fetch_add(added, Ordering::Relaxed);
    }

    /// Delete a key from L2. Returns `true` if the key was present.
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
}
