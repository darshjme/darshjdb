//! Change broadcaster for reactive query subscriptions.
//!
//! Listens for triple-store mutations via a [`tokio::sync::broadcast`] channel,
//! determines which active queries are affected, re-executes them with the
//! subscriber's permission context, computes diffs against cached results,
//! and pushes deltas to each subscribed WebSocket session.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, warn};

use super::diff::{QueryDiff, compute_diff, hash_result_set};
use super::registry::{SubscriptionHandle, SubscriptionRegistry};
use super::session::{SessionId, SessionManager, SubId};

/// A mutation event from the triple store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEvent {
    /// Transaction ID of the mutation.
    pub tx_id: i64,

    /// Entity IDs that were modified.
    pub entity_ids: Vec<String>,

    /// Attribute names that were touched (for dependency matching).
    pub attributes: Vec<String>,

    /// The type/collection of affected entities.
    pub entity_type: Option<String>,

    /// User ID that performed the mutation (for permission filtering).
    pub actor_id: Option<String>,
}

/// Outbound message to be sent to a specific session's WebSocket.
#[derive(Debug, Clone)]
pub struct OutboundDiff {
    /// Target session.
    pub session_id: SessionId,
    /// Subscription that matched.
    pub sub_id: SubId,
    /// The computed diff.
    pub diff: QueryDiff,
    /// Transaction ID this diff brings the client up to.
    pub tx_id: i64,
}

/// Trait for executing queries with a permission context.
/// Implemented by the query engine; abstracted here for testability.
pub trait QueryExecutor: Send + Sync + 'static {
    /// Execute a query AST and return the result set.
    /// `user_id` is used for permission-scoped execution.
    fn execute(
        &self,
        query_ast: &Value,
        user_id: Option<&str>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Value>, String>> + Send + '_>>;
}

/// Trait for tracking which query hashes are affected by a given change.
/// Implemented by the query dependency tracker.
pub trait DependencyTracker: Send + Sync + 'static {
    /// Given a change event, return the set of query hashes that might be affected.
    fn affected_queries(&self, event: &ChangeEvent) -> Vec<u64>;
}

/// The broadcaster coordinates change propagation from the triple store
/// to subscribed WebSocket clients.
pub struct Broadcaster {
    /// Shared session manager.
    sessions: Arc<SessionManager>,

    /// Shared subscription registry.
    registry: Arc<SubscriptionRegistry>,

    /// Channel for sending diffs to WebSocket handlers.
    outbound_tx: mpsc::Sender<OutboundDiff>,

    /// Cached last-result per (session_id, sub_id) for diff computation.
    /// Keyed by (session_id, sub_id) -> last result set.
    result_cache: dashmap::DashMap<(SessionId, SubId), Vec<Value>>,
}

impl Broadcaster {
    /// Create a new broadcaster.
    ///
    /// # Arguments
    ///
    /// * `sessions` - Shared session manager for looking up user context.
    /// * `registry` - Shared subscription registry for fan-out.
    /// * `outbound_tx` - Channel for delivering diffs to WebSocket writers.
    pub fn new(
        sessions: Arc<SessionManager>,
        registry: Arc<SubscriptionRegistry>,
        outbound_tx: mpsc::Sender<OutboundDiff>,
    ) -> Self {
        Self {
            sessions,
            registry,
            outbound_tx,
            result_cache: dashmap::DashMap::new(),
        }
    }

    /// Seed the result cache for a subscription (called on initial subscribe).
    pub fn cache_initial_result(&self, session_id: SessionId, sub_id: SubId, results: Vec<Value>) {
        self.result_cache.insert((session_id, sub_id), results);
    }

    /// Remove cached results for a subscription (called on unsubscribe).
    pub fn evict_cache(&self, session_id: SessionId, sub_id: SubId) {
        self.result_cache.remove(&(session_id, sub_id));
    }

    /// Remove all cached results for a session (called on disconnect).
    pub fn evict_session_cache(&self, session_id: &SessionId) {
        self.result_cache.retain(|(sid, _), _| sid != session_id);
    }

    /// Run the broadcast loop. This is the main event loop that should be
    /// spawned as a long-lived tokio task.
    ///
    /// Listens for [`ChangeEvent`]s on the broadcast receiver, determines
    /// affected subscriptions, re-executes queries, and pushes diffs.
    pub async fn run<E, D>(
        self: Arc<Self>,
        mut change_rx: broadcast::Receiver<ChangeEvent>,
        executor: Arc<E>,
        dep_tracker: Arc<D>,
    ) where
        E: QueryExecutor,
        D: DependencyTracker,
    {
        loop {
            let event = match change_rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        skipped = n,
                        "broadcaster lagged behind; some clients may receive full refreshes"
                    );
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("change broadcast channel closed, broadcaster shutting down");
                    return;
                }
            };

            let affected_hashes = dep_tracker.affected_queries(&event);

            if affected_hashes.is_empty() {
                continue;
            }

            // Collect all subscription handles that need updates.
            let mut handles_by_hash: HashMap<u64, Vec<SubscriptionHandle>> = HashMap::new();
            for hash in &affected_hashes {
                let subscribers = self.registry.subscribers_for(*hash);
                if !subscribers.is_empty() {
                    handles_by_hash.insert(*hash, subscribers);
                }
            }

            // For each unique (session, sub) pair, re-execute the query and diff.
            for (_query_hash, handles) in &handles_by_hash {
                for handle in handles {
                    self.process_subscription(handle, &event, executor.as_ref())
                        .await;
                }
            }
        }
    }

    /// Process a single subscription: re-execute query, compute diff, send.
    async fn process_subscription<E: QueryExecutor>(
        &self,
        handle: &SubscriptionHandle,
        event: &ChangeEvent,
        executor: &E,
    ) {
        // Look up the query AST and user context.
        let (query_ast, user_id) = match self.sessions.with_session(&handle.session_id, |s| {
            let sub = s.subscriptions.get(&handle.sub_id)?;
            Some((sub.query_ast.clone(), s.user_id.clone()))
        }) {
            Some(Some(data)) => data,
            _ => {
                debug!(
                    session_id = %handle.session_id,
                    sub_id = %handle.sub_id,
                    "subscription no longer active, skipping"
                );
                return;
            }
        };

        // Re-execute the query with the user's permission context.
        let new_results = match executor.execute(&query_ast, user_id.as_deref()).await {
            Ok(results) => results,
            Err(e) => {
                error!(
                    session_id = %handle.session_id,
                    sub_id = %handle.sub_id,
                    error = %e,
                    "failed to re-execute query for subscription"
                );
                return;
            }
        };

        // Compute diff against cached results.
        let cache_key = (handle.session_id, handle.sub_id);
        let old_results = self
            .result_cache
            .get(&cache_key)
            .map(|r| r.value().clone())
            .unwrap_or_default();

        let new_hash = hash_result_set(&new_results);
        let old_hash = hash_result_set(&old_results);

        if new_hash == old_hash {
            // No visible change for this subscriber.
            return;
        }

        let diff = compute_diff(&old_results, &new_results);

        if diff.is_empty() {
            return;
        }

        // Update cache.
        self.result_cache.insert(cache_key, new_results);

        // Update session cursor.
        self.sessions.with_session_mut(&handle.session_id, |s| {
            s.update_subscription_cursor(&handle.sub_id, new_hash, event.tx_id);
        });

        // Send the diff to the WebSocket writer task.
        let outbound = OutboundDiff {
            session_id: handle.session_id,
            sub_id: handle.sub_id,
            diff,
            tx_id: event.tx_id,
        };

        if let Err(e) = self.outbound_tx.try_send(outbound) {
            warn!(
                session_id = %handle.session_id,
                sub_id = %handle.sub_id,
                "failed to send diff to WebSocket writer: {e}"
            );
        }
    }
}
