//! Connector plugin system for DarshJDB.
//!
//! Connectors receive entity-level change events from the triple store's
//! broadcast channel and synchronise external systems: search indices,
//! caches, webhooks, audit logs, etc.
//!
//! Inspired by Ontotext GraphDB's automatic search-index sync, the
//! [`ConnectorManager`] listens on the existing [`ChangeEvent`] broadcast,
//! hydrates each event into a full [`EntityChangeEvent`] by reading the
//! current entity state from the triple store, and fans out to every
//! registered [`Connector`].
//!
//! # Built-in connectors
//!
//! - [`log::LogConnector`]       — Logs every change via `tracing::info` (dev/debug).
//! - [`webhook::WebhookConnector`] — POSTs entity payloads to a configured URL.

pub mod log;
pub mod webhook;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::error::Result;
use crate::sync::ChangeEvent;
use crate::triple_store::{PgTripleStore, TripleStore};

// ---------------------------------------------------------------------------
// Entity-level change event (hydrated from ChangeEvent + triple store)
// ---------------------------------------------------------------------------

/// A fully hydrated entity change event delivered to connectors.
///
/// Unlike the raw [`ChangeEvent`] (which lists entity IDs and touched
/// attributes), this struct carries the *current* attribute map so
/// connectors can index / forward the complete entity without hitting
/// the database themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityChangeEvent {
    /// The entity that changed.
    pub entity_id: Uuid,
    /// The collection / type of the entity (e.g. `"users"`).
    pub entity_type: String,
    /// All current (non-retracted) attributes of the entity, keyed by
    /// attribute name.
    pub attributes: HashMap<String, serde_json::Value>,
    /// Transaction ID of the mutation that triggered this event.
    pub tx_id: i64,
    /// Which attribute names were touched in this specific mutation.
    pub changed_attributes: Vec<String>,
}

// ---------------------------------------------------------------------------
// Connector trait  (dyn-compatible via Pin<Box<dyn Future>>)
// ---------------------------------------------------------------------------

/// A connector receives entity-level change events and syncs to an
/// external system.
///
/// Implementors must be `Send + Sync` so the manager can fan-out across
/// connectors concurrently.
///
/// Methods return boxed futures for dyn-compatibility (Rust async fn in
/// trait is not object-safe).
pub trait Connector: Send + Sync {
    /// Human-readable name used in log spans and metrics.
    fn name(&self) -> &str;

    /// Called when an entity is created or updated.
    ///
    /// Receives the full entity (all current attributes) so connectors
    /// never need to query the triple store directly.
    fn on_entity_changed(
        &self,
        event: EntityChangeEvent,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Called when an entity is deleted (all triples retracted).
    fn on_entity_deleted(
        &self,
        entity_id: Uuid,
        entity_type: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Called once on startup. Connectors can use this for initial sync,
    /// connection checks, or index creation.
    fn initialize(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }
}

// ---------------------------------------------------------------------------
// ConnectorManager
// ---------------------------------------------------------------------------

/// Manages registered connectors and bridges the triple-store broadcast
/// channel to the connector fan-out loop.
pub struct ConnectorManager {
    connectors: Vec<Box<dyn Connector>>,
    triple_store: Arc<PgTripleStore>,
}

impl ConnectorManager {
    /// Create a new manager with the given connectors and triple-store handle.
    pub fn new(connectors: Vec<Box<dyn Connector>>, triple_store: Arc<PgTripleStore>) -> Self {
        Self {
            connectors,
            triple_store,
        }
    }

    /// Initialise every registered connector (called once at startup).
    pub async fn initialize_all(&self) {
        for connector in &self.connectors {
            match connector.initialize().await {
                Ok(()) => info!(connector = connector.name(), "connector initialized"),
                Err(e) => error!(
                    connector = connector.name(),
                    error = %e,
                    "connector failed to initialize"
                ),
            }
        }
    }

    /// Run the connector fan-out loop.
    ///
    /// Listens on `change_rx` for [`ChangeEvent`]s, hydrates each into
    /// one or more [`EntityChangeEvent`]s (one per affected entity), and
    /// dispatches to every connector.
    ///
    /// This method runs forever (or until the broadcast channel closes)
    /// and should be spawned as a background tokio task.
    pub async fn run(self: Arc<Self>, mut change_rx: broadcast::Receiver<ChangeEvent>) {
        info!(
            count = self.connectors.len(),
            "connector manager started, listening for change events"
        );

        loop {
            let event = match change_rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        skipped = n,
                        "connector manager lagged behind; some events were missed"
                    );
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("change broadcast channel closed, connector manager shutting down");
                    return;
                }
            };

            self.handle_change_event(event).await;
        }
    }

    /// Process a single [`ChangeEvent`], hydrating entities and dispatching.
    async fn handle_change_event(&self, event: ChangeEvent) {
        let entity_type = event.entity_type.clone().unwrap_or_default();

        for entity_id_str in &event.entity_ids {
            let entity_id = match Uuid::parse_str(entity_id_str) {
                Ok(id) => id,
                Err(e) => {
                    warn!(
                        entity_id = %entity_id_str,
                        error = %e,
                        "skipping non-UUID entity id in change event"
                    );
                    continue;
                }
            };

            // Fetch current entity state from the triple store.
            let triples = match self.triple_store.get_entity(entity_id).await {
                Ok(t) => t,
                Err(e) => {
                    error!(
                        entity_id = %entity_id,
                        error = %e,
                        "failed to fetch entity for connector dispatch"
                    );
                    continue;
                }
            };

            if triples.is_empty() {
                // Entity was fully deleted (all triples retracted).
                for connector in &self.connectors {
                    if let Err(e) = connector.on_entity_deleted(entity_id, &entity_type).await {
                        error!(
                            connector = connector.name(),
                            entity_id = %entity_id,
                            error = %e,
                            "connector on_entity_deleted failed"
                        );
                    }
                }
            } else {
                // Build the attribute map from current triples.
                let mut attributes: HashMap<String, serde_json::Value> = HashMap::new();
                for triple in &triples {
                    attributes.insert(triple.attribute.clone(), triple.value.clone());
                }

                let change_event = EntityChangeEvent {
                    entity_id,
                    entity_type: entity_type.clone(),
                    attributes,
                    tx_id: event.tx_id,
                    changed_attributes: event.attributes.clone(),
                };

                for connector in &self.connectors {
                    if let Err(e) = connector.on_entity_changed(change_event.clone()).await {
                        error!(
                            connector = connector.name(),
                            entity_id = %entity_id,
                            error = %e,
                            "connector on_entity_changed failed"
                        );
                    }
                }
            }
        }
    }

    /// Returns the number of registered connectors.
    pub fn connector_count(&self) -> usize {
        self.connectors.len()
    }
}
