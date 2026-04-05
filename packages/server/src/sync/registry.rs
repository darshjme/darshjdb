//! Subscription registry for fan-out deduplication.
//!
//! Maps `query_hash` to the set of [`SessionId`]s subscribed to that query.
//! When a mutation arrives, the broadcaster looks up affected query hashes
//! and fans out to exactly the sessions that care -- executing the query
//! only once per unique hash rather than once per subscriber.

use std::collections::HashSet;

use dashmap::DashMap;

use super::session::{SessionId, SubId};

/// Key combining session and subscription for precise tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionHandle {
    pub session_id: SessionId,
    pub sub_id: SubId,
}

/// Global registry mapping query hashes to subscribing sessions.
///
/// Thread-safe via [`DashMap`]; supports concurrent register/unregister
/// from multiple WebSocket handler tasks.
#[derive(Debug)]
pub struct SubscriptionRegistry {
    /// query_hash -> set of (session_id, sub_id) handles.
    by_query: DashMap<u64, HashSet<SubscriptionHandle>>,

    /// Reverse index: session_id -> set of query_hashes, for fast cleanup on disconnect.
    by_session: DashMap<SessionId, HashSet<u64>>,
}

impl SubscriptionRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            by_query: DashMap::new(),
            by_session: DashMap::new(),
        }
    }

    /// Register a subscription. Deduplication happens at the query level:
    /// multiple sessions subscribing to the same query_hash share one entry
    /// in `by_query`, but each gets their own handle for targeted delivery.
    pub fn register(&self, query_hash: u64, session_id: SessionId, sub_id: SubId) {
        let handle = SubscriptionHandle { session_id, sub_id };

        self.by_query
            .entry(query_hash)
            .or_insert_with(HashSet::new)
            .insert(handle);

        self.by_session
            .entry(session_id)
            .or_insert_with(HashSet::new)
            .insert(query_hash);
    }

    /// Unregister a single subscription.
    pub fn unregister(&self, query_hash: u64, session_id: SessionId, sub_id: SubId) {
        let handle = SubscriptionHandle { session_id, sub_id };

        if let Some(mut entry) = self.by_query.get_mut(&query_hash) {
            entry.remove(&handle);
            // Clean up empty sets to prevent memory leaks.
            if entry.is_empty() {
                drop(entry);
                self.by_query.remove(&query_hash);
            }
        }

        if let Some(mut entry) = self.by_session.get_mut(&session_id) {
            // Only remove the query_hash from the session's set if no other
            // subscriptions from this session reference it.
            let still_has = self
                .by_query
                .get(&query_hash)
                .map(|set| set.iter().any(|h| h.session_id == session_id))
                .unwrap_or(false);

            if !still_has {
                entry.remove(&query_hash);
            }

            if entry.is_empty() {
                drop(entry);
                self.by_session.remove(&session_id);
            }
        }
    }

    /// Remove all subscriptions for a session (on disconnect).
    /// Returns the list of query hashes that were unsubscribed.
    pub fn unregister_session(&self, session_id: &SessionId) -> Vec<u64> {
        let hashes = match self.by_session.remove(session_id) {
            Some((_, hashes)) => hashes,
            None => return Vec::new(),
        };

        let mut removed = Vec::with_capacity(hashes.len());

        for query_hash in &hashes {
            if let Some(mut entry) = self.by_query.get_mut(query_hash) {
                entry.retain(|h| h.session_id != *session_id);
                if entry.is_empty() {
                    drop(entry);
                    self.by_query.remove(query_hash);
                }
            }
            removed.push(*query_hash);
        }

        removed
    }

    /// Get all subscription handles for a given query hash.
    /// Returns an empty vec if no sessions are subscribed.
    pub fn subscribers_for(&self, query_hash: u64) -> Vec<SubscriptionHandle> {
        self.by_query
            .get(&query_hash)
            .map(|set| set.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Returns `true` if at least one session is subscribed to this query hash.
    pub fn has_subscribers(&self, query_hash: u64) -> bool {
        self.by_query
            .get(&query_hash)
            .map(|set| !set.is_empty())
            .unwrap_or(false)
    }

    /// Total number of unique query hashes with active subscriptions.
    pub fn unique_query_count(&self) -> usize {
        self.by_query.len()
    }

    /// Total number of active sessions with at least one subscription.
    pub fn active_session_count(&self) -> usize {
        self.by_session.len()
    }
}

impl Default for SubscriptionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
