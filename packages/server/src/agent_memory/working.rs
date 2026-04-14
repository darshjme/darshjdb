// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! In-memory L1 "working" tier.
//!
//! Stores the most recent N messages per session in a [`DashMap`] keyed
//! by `session_id`. When the working window overflows, the oldest entry
//! is evicted and surfaced to the caller so the repo layer can promote
//! it to the episodic tier.
//!
//! This struct is intentionally cheap to clone — every field is wrapped
//! in [`Arc`] so the `WorkingMemory` can sit on `AgentMemoryState`
//! alongside the Postgres pool and be handed to every handler with no
//! allocation.

use std::collections::VecDeque;
use std::sync::Arc;

use dashmap::DashMap;
use uuid::Uuid;

use super::types::{MemoryEntry, MemoryTier};

/// Default maximum messages held in the working tier per session.
pub const DEFAULT_WORKING_WINDOW: usize = 32;

/// Lock-free per-session working memory store.
#[derive(Clone)]
pub struct WorkingMemory {
    inner: Arc<DashMap<Uuid, VecDeque<MemoryEntry>>>,
    capacity: usize,
}

impl Default for WorkingMemory {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_WORKING_WINDOW)
    }
}

impl WorkingMemory {
    /// Create a working memory with a custom per-session window size.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            capacity: capacity.max(1),
        }
    }

    /// Per-session window size.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Push a new entry into a session's working window. If the window
    /// overflows, the oldest entry is returned so the caller can promote
    /// it to the episodic tier.
    pub fn push(&self, session_id: Uuid, mut entry: MemoryEntry) -> Option<MemoryEntry> {
        entry.tier = MemoryTier::Working;
        let mut slot = self.inner.entry(session_id).or_default();
        slot.push_back(entry);
        if slot.len() > self.capacity {
            slot.pop_front()
        } else {
            None
        }
    }

    /// Snapshot the current working window in chronological order.
    pub fn snapshot(&self, session_id: Uuid) -> Vec<MemoryEntry> {
        self.inner
            .get(&session_id)
            .map(|q| q.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Number of messages currently in the window for a session.
    pub fn len(&self, session_id: Uuid) -> usize {
        self.inner.get(&session_id).map(|q| q.len()).unwrap_or(0)
    }

    /// Total token count across the working window for a session.
    pub fn total_tokens(&self, session_id: Uuid) -> i64 {
        self.inner
            .get(&session_id)
            .map(|q| q.iter().map(|e| e.token_count as i64).sum())
            .unwrap_or(0)
    }

    /// Drop the working window for a session entirely (used on
    /// `DELETE /sessions/:id`).
    pub fn clear(&self, session_id: Uuid) {
        self.inner.remove(&session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::MemoryRole;
    use chrono::Utc;
    use serde_json::json;

    fn entry(content: &str) -> MemoryEntry {
        MemoryEntry {
            id: Uuid::new_v4(),
            session_id: Uuid::nil(),
            tier: MemoryTier::Working,
            role: MemoryRole::User,
            content: content.into(),
            token_count: content.len() as i32,
            metadata: json!({}),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn push_and_snapshot_round_trip() {
        let wm = WorkingMemory::with_capacity(4);
        let sid = Uuid::new_v4();
        for i in 0..3 {
            wm.push(sid, entry(&format!("m{i}")));
        }
        let snap = wm.snapshot(sid);
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].content, "m0");
        assert_eq!(snap[2].content, "m2");
    }

    #[test]
    fn overflow_returns_oldest() {
        let wm = WorkingMemory::with_capacity(2);
        let sid = Uuid::new_v4();
        assert!(wm.push(sid, entry("a")).is_none());
        assert!(wm.push(sid, entry("b")).is_none());
        let evicted = wm.push(sid, entry("c")).expect("should evict");
        assert_eq!(evicted.content, "a");
        assert_eq!(wm.snapshot(sid).len(), 2);
    }
}
