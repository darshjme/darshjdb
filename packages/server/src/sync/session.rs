//! Session management for WebSocket connections.
//!
//! Each connected client gets a [`SyncSession`] that tracks its identity,
//! active query subscriptions, transaction cursor, and connection metadata.
//! The [`SessionManager`] provides concurrent access via [`DashMap`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Opaque session identifier, assigned on WebSocket upgrade.
pub type SessionId = uuid::Uuid;

/// Opaque subscription identifier, assigned per subscribe request.
pub type SubId = uuid::Uuid;

/// A single active query subscription within a session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSubscription {
    /// Hash of the normalized query AST for deduplication across sessions.
    pub query_hash: u64,

    /// The original query AST so we can re-execute on change.
    pub query_ast: Value,

    /// Hash of the last result set sent to this client, used for diff detection.
    pub last_result_hash: u64,

    /// Transaction ID of the last result sent, for cursor-based catch-up.
    pub last_tx: i64,

    /// When this subscription was created (epoch millis for serde compatibility).
    pub created_at_ms: u64,
}

/// Per-connection session state.
#[derive(Debug, Clone)]
pub struct SyncSession {
    /// Unique session identifier.
    pub id: SessionId,

    /// Authenticated user ID. `None` until auth completes.
    pub user_id: Option<String>,

    /// Active subscriptions keyed by subscription ID.
    pub subscriptions: HashMap<SubId, ActiveSubscription>,

    /// Highest transaction ID this session has been synced to.
    pub last_tx: i64,

    /// When this session connected.
    pub connected_at: Instant,

    /// Remote peer address for logging and rate limiting.
    pub peer_addr: Option<SocketAddr>,
}

impl SyncSession {
    /// Create a new unauthenticated session.
    pub fn new(id: SessionId, peer_addr: Option<SocketAddr>) -> Self {
        Self {
            id,
            user_id: None,
            subscriptions: HashMap::new(),
            last_tx: 0,
            connected_at: Instant::now(),
            peer_addr,
        }
    }

    /// Mark this session as authenticated.
    pub fn authenticate(&mut self, user_id: String) {
        self.user_id = Some(user_id);
    }

    /// Returns `true` if the session has completed authentication.
    pub fn is_authenticated(&self) -> bool {
        self.user_id.is_some()
    }

    /// Add a subscription. Returns the assigned [`SubId`].
    pub fn add_subscription(&mut self, query_hash: u64, query_ast: Value) -> SubId {
        let sub_id = SubId::new_v4();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.subscriptions.insert(
            sub_id,
            ActiveSubscription {
                query_hash,
                query_ast,
                last_result_hash: 0,
                last_tx: self.last_tx,
                created_at_ms: now_ms,
            },
        );
        sub_id
    }

    /// Remove a subscription by ID. Returns the removed subscription if it existed.
    pub fn remove_subscription(&mut self, sub_id: &SubId) -> Option<ActiveSubscription> {
        self.subscriptions.remove(sub_id)
    }

    /// Update the result hash and tx cursor for a subscription after delivering a diff.
    pub fn update_subscription_cursor(
        &mut self,
        sub_id: &SubId,
        result_hash: u64,
        tx: i64,
    ) -> bool {
        if let Some(sub) = self.subscriptions.get_mut(sub_id) {
            sub.last_result_hash = result_hash;
            sub.last_tx = tx;
            true
        } else {
            false
        }
    }
}

/// Thread-safe manager for all active sessions.
///
/// Uses [`DashMap`] for lock-free concurrent reads and fine-grained write locks.
#[derive(Debug)]
pub struct SessionManager {
    sessions: DashMap<SessionId, SyncSession>,
}

impl SessionManager {
    /// Create an empty session manager.
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
        }
    }

    /// Register a new session. Returns the assigned [`SessionId`].
    pub fn create_session(&self, peer_addr: Option<SocketAddr>) -> SessionId {
        let id = SessionId::new_v4();
        let session = SyncSession::new(id, peer_addr);
        self.sessions.insert(id, session);
        id
    }

    /// Remove a session and return it, or `None` if not found.
    pub fn remove_session(&self, id: &SessionId) -> Option<SyncSession> {
        self.sessions.remove(id).map(|(_, s)| s)
    }

    /// Execute a closure with immutable access to a session.
    /// Returns `None` if the session does not exist.
    pub fn with_session<F, R>(&self, id: &SessionId, f: F) -> Option<R>
    where
        F: FnOnce(&SyncSession) -> R,
    {
        self.sessions.get(id).map(|s| f(&s))
    }

    /// Execute a closure with mutable access to a session.
    /// Returns `None` if the session does not exist.
    pub fn with_session_mut<F, R>(&self, id: &SessionId, f: F) -> Option<R>
    where
        F: FnOnce(&mut SyncSession) -> R,
    {
        self.sessions.get_mut(id).map(|mut s| f(&mut s))
    }

    /// Number of active sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Iterate over all session IDs (snapshot).
    pub fn session_ids(&self) -> Vec<SessionId> {
        self.sessions.iter().map(|entry| *entry.key()).collect()
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn session_creation_defaults() {
        let id = SessionId::new_v4();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
        let session = SyncSession::new(id, Some(addr));

        assert_eq!(session.id, id);
        assert!(session.user_id.is_none());
        assert!(session.subscriptions.is_empty());
        assert_eq!(session.last_tx, 0);
        assert_eq!(session.peer_addr, Some(addr));
        assert!(!session.is_authenticated());
    }

    #[test]
    fn session_authentication() {
        let mut session = SyncSession::new(SessionId::new_v4(), None);
        assert!(!session.is_authenticated());

        session.authenticate("user-42".into());
        assert!(session.is_authenticated());
        assert_eq!(session.user_id.as_deref(), Some("user-42"));
    }

    #[test]
    fn add_and_remove_subscription() {
        let mut session = SyncSession::new(SessionId::new_v4(), None);
        let sub_id = session.add_subscription(123, json!({"select": "*"}));

        assert_eq!(session.subscriptions.len(), 1);
        let sub = &session.subscriptions[&sub_id];
        assert_eq!(sub.query_hash, 123);
        assert_eq!(sub.last_result_hash, 0);
        assert_eq!(sub.last_tx, 0);

        let removed = session.remove_subscription(&sub_id);
        assert!(removed.is_some());
        assert!(session.subscriptions.is_empty());
    }

    #[test]
    fn remove_nonexistent_subscription_returns_none() {
        let mut session = SyncSession::new(SessionId::new_v4(), None);
        let fake_id = SubId::new_v4();
        assert!(session.remove_subscription(&fake_id).is_none());
    }

    #[test]
    fn update_subscription_cursor() {
        let mut session = SyncSession::new(SessionId::new_v4(), None);
        let sub_id = session.add_subscription(100, json!({}));

        assert!(session.update_subscription_cursor(&sub_id, 999, 42));
        let sub = &session.subscriptions[&sub_id];
        assert_eq!(sub.last_result_hash, 999);
        assert_eq!(sub.last_tx, 42);
    }

    #[test]
    fn update_cursor_nonexistent_returns_false() {
        let mut session = SyncSession::new(SessionId::new_v4(), None);
        let fake_id = SubId::new_v4();
        assert!(!session.update_subscription_cursor(&fake_id, 1, 1));
    }

    #[test]
    fn session_manager_create_and_remove() {
        let mgr = SessionManager::new();
        assert_eq!(mgr.session_count(), 0);

        let id = mgr.create_session(None);
        assert_eq!(mgr.session_count(), 1);

        let removed = mgr.remove_session(&id);
        assert!(removed.is_some());
        assert_eq!(mgr.session_count(), 0);
    }

    #[test]
    fn session_manager_remove_nonexistent() {
        let mgr = SessionManager::new();
        let fake = SessionId::new_v4();
        assert!(mgr.remove_session(&fake).is_none());
    }

    #[test]
    fn session_manager_with_session() {
        let mgr = SessionManager::new();
        let id = mgr.create_session(None);

        let result = mgr.with_session(&id, |s| s.id);
        assert_eq!(result, Some(id));

        let fake = SessionId::new_v4();
        let result = mgr.with_session(&fake, |s| s.id);
        assert!(result.is_none());
    }

    #[test]
    fn session_manager_with_session_mut() {
        let mgr = SessionManager::new();
        let id = mgr.create_session(None);

        mgr.with_session_mut(&id, |s| {
            s.authenticate("alice".into());
        });

        let is_auth = mgr.with_session(&id, |s| s.is_authenticated());
        assert_eq!(is_auth, Some(true));
    }

    #[test]
    fn session_manager_session_ids() {
        let mgr = SessionManager::new();
        let id1 = mgr.create_session(None);
        let id2 = mgr.create_session(None);

        let ids = mgr.session_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn multiple_subscriptions_per_session() {
        let mut session = SyncSession::new(SessionId::new_v4(), None);
        let sub1 = session.add_subscription(100, json!({"a": 1}));
        let sub2 = session.add_subscription(200, json!({"b": 2}));
        let sub3 = session.add_subscription(100, json!({"a": 1})); // same hash, different sub

        assert_eq!(session.subscriptions.len(), 3);
        assert_ne!(sub1, sub2);
        assert_ne!(sub1, sub3);

        session.remove_subscription(&sub1);
        assert_eq!(session.subscriptions.len(), 2);
    }
}
