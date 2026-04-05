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

        self.by_query.entry(query_hash).or_default().insert(handle);

        self.by_session
            .entry(session_id)
            .or_default()
            .insert(query_hash);
    }

    /// Unregister a single subscription.
    ///
    /// Uses `remove_if` to atomically check-and-remove empty sets, avoiding
    /// TOCTOU races between the emptiness check and the removal.
    pub fn unregister(&self, query_hash: u64, session_id: SessionId, sub_id: SubId) {
        let handle = SubscriptionHandle { session_id, sub_id };

        // Remove the handle from by_query. Use remove_if to atomically
        // clean up empty sets without a drop-then-remove race.
        let still_has_session = if let Some(mut entry) = self.by_query.get_mut(&query_hash) {
            entry.remove(&handle);
            let still_has = entry.iter().any(|h| h.session_id == session_id);
            if entry.is_empty() {
                // Drop the mutable ref and atomically remove if still empty.
                drop(entry);
                self.by_query
                    .remove_if(&query_hash, |_, set| set.is_empty());
            }
            still_has
        } else {
            false
        };

        // Only remove query_hash from session's reverse index if no other
        // subscriptions from this session reference it.
        if !still_has_session && let Some(mut entry) = self.by_session.get_mut(&session_id) {
            entry.remove(&query_hash);
            if entry.is_empty() {
                drop(entry);
                self.by_session
                    .remove_if(&session_id, |_, set| set.is_empty());
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
                    self.by_query.remove_if(query_hash, |_, set| set.is_empty());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sid() -> SessionId {
        SessionId::new_v4()
    }

    fn sub() -> SubId {
        SubId::new_v4()
    }

    #[test]
    fn register_and_lookup() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let sub1 = sub();

        reg.register(100, s1, sub1);

        let subs = reg.subscribers_for(100);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].session_id, s1);
        assert_eq!(subs[0].sub_id, sub1);
        assert!(reg.has_subscribers(100));
        assert!(!reg.has_subscribers(200));
    }

    #[test]
    fn multiple_sessions_same_query() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let s2 = sid();
        let sub1 = sub();
        let sub2 = sub();

        reg.register(100, s1, sub1);
        reg.register(100, s2, sub2);

        assert_eq!(reg.subscribers_for(100).len(), 2);
        assert_eq!(reg.unique_query_count(), 1);
        assert_eq!(reg.active_session_count(), 2);
    }

    #[test]
    fn deduplication_same_session_different_subs() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let sub1 = sub();
        let sub2 = sub();

        reg.register(100, s1, sub1);
        reg.register(100, s1, sub2);

        // Two handles for the same query from the same session.
        assert_eq!(reg.subscribers_for(100).len(), 2);
        // But only 1 unique session.
        assert_eq!(reg.active_session_count(), 1);
    }

    #[test]
    fn unregister_single() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let sub1 = sub();
        let sub2 = sub();

        reg.register(100, s1, sub1);
        reg.register(100, s1, sub2);

        reg.unregister(100, s1, sub1);
        assert_eq!(reg.subscribers_for(100).len(), 1);
        assert_eq!(reg.subscribers_for(100)[0].sub_id, sub2);
        // Session still active because sub2 remains.
        assert_eq!(reg.active_session_count(), 1);
    }

    #[test]
    fn unregister_last_cleans_up() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let sub1 = sub();

        reg.register(100, s1, sub1);
        reg.unregister(100, s1, sub1);

        assert!(!reg.has_subscribers(100));
        assert_eq!(reg.unique_query_count(), 0);
        assert_eq!(reg.active_session_count(), 0);
    }

    #[test]
    fn unregister_session_removes_all() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let sub1 = sub();
        let sub2 = sub();

        reg.register(100, s1, sub1);
        reg.register(200, s1, sub2);

        let removed = reg.unregister_session(&s1);
        assert_eq!(removed.len(), 2);
        assert!(!reg.has_subscribers(100));
        assert!(!reg.has_subscribers(200));
        assert_eq!(reg.active_session_count(), 0);
        assert_eq!(reg.unique_query_count(), 0);
    }

    #[test]
    fn unregister_session_leaves_other_sessions() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let s2 = sid();
        let sub1 = sub();
        let sub2 = sub();

        reg.register(100, s1, sub1);
        reg.register(100, s2, sub2);

        reg.unregister_session(&s1);
        assert_eq!(reg.subscribers_for(100).len(), 1);
        assert_eq!(reg.subscribers_for(100)[0].session_id, s2);
        assert_eq!(reg.active_session_count(), 1);
    }

    #[test]
    fn unregister_nonexistent_is_noop() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();
        let sub1 = sub();

        // Unregister before registering.
        reg.unregister(100, s1, sub1);
        assert_eq!(reg.unique_query_count(), 0);

        // Unregister session that was never registered.
        let removed = reg.unregister_session(&s1);
        assert!(removed.is_empty());
    }

    #[test]
    fn subscribers_for_empty_query() {
        let reg = SubscriptionRegistry::new();
        assert!(reg.subscribers_for(999).is_empty());
    }

    #[test]
    fn register_across_multiple_queries() {
        let reg = SubscriptionRegistry::new();
        let s1 = sid();

        reg.register(100, s1, sub());
        reg.register(200, s1, sub());
        reg.register(300, s1, sub());

        assert_eq!(reg.unique_query_count(), 3);
        assert_eq!(reg.active_session_count(), 1);
    }
}
