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
        self.subscriptions.insert(
            sub_id,
            ActiveSubscription {
                query_hash,
                query_ast,
                last_result_hash: 0,
                last_tx: self.last_tx,
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
