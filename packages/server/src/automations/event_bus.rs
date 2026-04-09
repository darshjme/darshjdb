//! Internal event bus for the DarshJDB automation engine.
//!
//! The event bus is the connective tissue between triple-store mutations
//! and automation triggers. Mutations emit [`DdbEvent`]s onto the bus,
//! the [`TriggerEvaluator`] listens for matching events, and fires
//! the corresponding workflows.
//!
//! Architecture inspired by DAF's conversation logging and event replay.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use tracing::{debug, warn};
use uuid::Uuid;

// ── Event types ───────────────────────────────────────────────────

/// Internal event emitted by the DarshJDB data layer.
///
/// These events flow through the [`EventBus`] and are matched against
/// automation triggers by the [`TriggerEvaluator`](super::trigger::TriggerEvaluator).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum DdbEvent {
    /// A new record was created.
    RecordCreated {
        entity_id: Uuid,
        entity_type: String,
        attributes: HashMap<String, serde_json::Value>,
        tx_id: i64,
    },

    /// An existing record was updated.
    RecordUpdated {
        entity_id: Uuid,
        entity_type: String,
        attributes: HashMap<String, serde_json::Value>,
        changed_fields: Vec<String>,
        tx_id: i64,
    },

    /// A record was deleted (all triples retracted).
    RecordDeleted {
        entity_id: Uuid,
        entity_type: String,
        tx_id: i64,
    },

    /// A specific field changed on a record.
    FieldChanged {
        entity_id: Uuid,
        entity_type: String,
        field_name: String,
        old_value: Option<serde_json::Value>,
        new_value: serde_json::Value,
        attributes: HashMap<String, serde_json::Value>,
        tx_id: i64,
    },

    /// An authentication event occurred (login, signup, etc.).
    AuthEvent {
        user_id: String,
        action: String,
        metadata: HashMap<String, serde_json::Value>,
    },

    /// A storage event occurred (upload, delete, etc.).
    StorageEvent {
        path: String,
        action: String,
        size_bytes: Option<u64>,
    },

    /// Custom application-level event.
    Custom {
        kind: String,
        data: serde_json::Value,
    },
}

impl DdbEvent {
    /// Extract the entity type from the event, if applicable.
    pub fn entity_type(&self) -> Option<&str> {
        match self {
            DdbEvent::RecordCreated { entity_type, .. }
            | DdbEvent::RecordUpdated { entity_type, .. }
            | DdbEvent::RecordDeleted { entity_type, .. }
            | DdbEvent::FieldChanged { entity_type, .. } => Some(entity_type),
            _ => None,
        }
    }

    /// Extract the entity ID from the event, if applicable.
    pub fn entity_id(&self) -> Option<Uuid> {
        match self {
            DdbEvent::RecordCreated { entity_id, .. }
            | DdbEvent::RecordUpdated { entity_id, .. }
            | DdbEvent::RecordDeleted { entity_id, .. }
            | DdbEvent::FieldChanged { entity_id, .. } => Some(*entity_id),
            _ => None,
        }
    }

    /// Extract the transaction ID from the event, if applicable.
    pub fn tx_id(&self) -> Option<i64> {
        match self {
            DdbEvent::RecordCreated { tx_id, .. }
            | DdbEvent::RecordUpdated { tx_id, .. }
            | DdbEvent::RecordDeleted { tx_id, .. }
            | DdbEvent::FieldChanged { tx_id, .. } => Some(*tx_id),
            _ => None,
        }
    }
}

// ── Event log entry ───────────────────────────────────────────────

/// A logged event with timestamp and metadata, used for replay and
/// knowledge-base extraction (inspired by DAF's conversation logging).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventLogEntry {
    /// Unique log entry ID.
    pub id: Uuid,
    /// The event itself.
    pub event: DdbEvent,
    /// When the event was emitted.
    pub timestamp: DateTime<Utc>,
    /// Whether this event triggered any automations.
    pub triggered_automations: Vec<Uuid>,
}

// ── Event bus ─────────────────────────────────────────────────────

/// Broadcast-based event bus for internal DarshJDB events.
///
/// All triple-store mutations should emit events here. The automation
/// engine subscribes to the bus and evaluates triggers against incoming
/// events.
///
/// The bus also maintains a bounded event log for debugging, replay,
/// and knowledge extraction.
pub struct EventBus {
    /// Broadcast sender for event distribution.
    tx: broadcast::Sender<DdbEvent>,
    /// Bounded event log (most recent N events).
    log: Arc<RwLock<EventLog>>,
}

/// Bounded ring-buffer event log.
struct EventLog {
    entries: Vec<EventLogEntry>,
    max_size: usize,
}

impl EventLog {
    fn new(max_size: usize) -> Self {
        Self {
            entries: Vec::with_capacity(max_size),
            max_size,
        }
    }

    fn push(&mut self, entry: EventLogEntry) {
        if self.entries.len() >= self.max_size {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    fn recent(&self, limit: usize) -> Vec<EventLogEntry> {
        let start = self.entries.len().saturating_sub(limit);
        self.entries[start..].to_vec()
    }

    fn find_by_entity(&self, entity_id: Uuid) -> Vec<EventLogEntry> {
        self.entries
            .iter()
            .filter(|e| e.event.entity_id() == Some(entity_id))
            .cloned()
            .collect()
    }
}

impl EventBus {
    /// Create a new event bus with the given channel capacity and log size.
    ///
    /// - `channel_capacity`: how many events can be buffered before slow
    ///   subscribers lag.
    /// - `log_size`: maximum number of events to keep in the replay log.
    pub fn new(channel_capacity: usize, log_size: usize) -> Self {
        let (tx, _) = broadcast::channel(channel_capacity);
        Self {
            tx,
            log: Arc::new(RwLock::new(EventLog::new(log_size))),
        }
    }

    /// Create a bus with sensible defaults (1024 channel, 10_000 log).
    pub fn default_capacity() -> Self {
        Self::new(1024, 10_000)
    }

    /// Emit an event onto the bus.
    ///
    /// The event is broadcast to all active subscribers and logged.
    /// Returns the number of active subscribers that received the event.
    pub async fn emit(&self, event: DdbEvent) -> usize {
        // Log the event.
        let entry = EventLogEntry {
            id: Uuid::new_v4(),
            event: event.clone(),
            timestamp: Utc::now(),
            triggered_automations: Vec::new(),
        };

        {
            let mut log = self.log.write().await;
            log.push(entry);
        }

        // Broadcast to subscribers.
        match self.tx.send(event) {
            Ok(n) => {
                debug!(subscribers = n, "event emitted to bus");
                n
            }
            Err(_) => {
                // No active subscribers — that's fine, the event is still logged.
                0
            }
        }
    }

    /// Create a new subscriber that receives all events.
    pub fn subscribe(&self) -> EventSubscriber {
        EventSubscriber {
            rx: self.tx.subscribe(),
            filter: None,
        }
    }

    /// Create a filtered subscriber that only receives events for a
    /// specific entity type.
    pub fn subscribe_entity_type(&self, entity_type: impl Into<String>) -> EventSubscriber {
        EventSubscriber {
            rx: self.tx.subscribe(),
            filter: Some(EventFilter::EntityType(entity_type.into())),
        }
    }

    /// Get the most recent events from the log.
    pub async fn recent_events(&self, limit: usize) -> Vec<EventLogEntry> {
        let log = self.log.read().await;
        log.recent(limit)
    }

    /// Get all logged events for a specific entity.
    pub async fn events_for_entity(&self, entity_id: Uuid) -> Vec<EventLogEntry> {
        let log = self.log.read().await;
        log.find_by_entity(entity_id)
    }

    /// Get the raw broadcast sender for direct integration.
    pub fn sender(&self) -> broadcast::Sender<DdbEvent> {
        self.tx.clone()
    }

    /// Current number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

// ── Event subscriber ──────────────────────────────────────────────

/// Async event stream with optional filtering.
pub struct EventSubscriber {
    rx: broadcast::Receiver<DdbEvent>,
    filter: Option<EventFilter>,
}

/// Filter for event subscribers.
#[derive(Debug, Clone)]
enum EventFilter {
    EntityType(String),
}

impl EventSubscriber {
    /// Receive the next event, blocking until one arrives.
    ///
    /// Returns `None` if the channel is closed.
    /// Automatically skips events that don't match the filter.
    pub async fn recv(&mut self) -> Option<DdbEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    if self.matches(&event) {
                        return Some(event);
                    }
                    // Filtered out — continue waiting.
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "event subscriber lagged behind");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    }

    /// Check if an event matches this subscriber's filter.
    fn matches(&self, event: &DdbEvent) -> bool {
        match &self.filter {
            None => true,
            Some(EventFilter::EntityType(et)) => event.entity_type().is_some_and(|t| t == et),
        }
    }
}

// ── Bridge from ChangeEvent to DdbEvent ───────────────────────────

/// Convert a triple-store [`ChangeEvent`](crate::sync::broadcaster::ChangeEvent)
/// into one or more [`DdbEvent`]s for the automation bus.
///
/// This function should be called in the mutation pipeline after the
/// triple store write succeeds.
pub fn change_event_to_ddb_events(
    change: &crate::sync::broadcaster::ChangeEvent,
    entity_attributes: &HashMap<Uuid, HashMap<String, serde_json::Value>>,
    is_create: bool,
) -> Vec<DdbEvent> {
    let entity_type = change.entity_type.clone().unwrap_or_default();
    let mut events = Vec::new();

    for entity_id_str in &change.entity_ids {
        let entity_id = match Uuid::parse_str(entity_id_str) {
            Ok(id) => id,
            Err(_) => continue,
        };

        let attributes = entity_attributes
            .get(&entity_id)
            .cloned()
            .unwrap_or_default();

        if is_create {
            events.push(DdbEvent::RecordCreated {
                entity_id,
                entity_type: entity_type.clone(),
                attributes,
                tx_id: change.tx_id,
            });
        } else {
            events.push(DdbEvent::RecordUpdated {
                entity_id,
                entity_type: entity_type.clone(),
                attributes,
                changed_fields: change.attributes.clone(),
                tx_id: change.tx_id,
            });

            // Emit per-field events for field-change triggers.
            for field in &change.attributes {
                events.push(DdbEvent::FieldChanged {
                    entity_id,
                    entity_type: entity_type.clone(),
                    field_name: field.clone(),
                    old_value: None,
                    new_value: serde_json::Value::Null,
                    attributes: HashMap::new(),
                    tx_id: change.tx_id,
                });
            }
        }
    }

    events
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn event_bus_emit_and_receive() {
        let _bus_init = EventBus::new(16, 100);
        let _sub_init = _bus_init.subscribe();

        let event = DdbEvent::RecordCreated {
            entity_id: Uuid::new_v4(),
            entity_type: "users".to_string(),
            attributes: HashMap::new(),
            tx_id: 1,
        };

        // Emit in a separate task so recv doesn't block forever.
        let event_clone = event.clone();
        tokio::spawn({ async move { drop(event_clone) } });

        // Use a channel-based approach instead.
        let bus = Arc::new(EventBus::new(16, 100));
        let mut sub = bus.subscribe();

        let bus2 = bus.clone();
        let eid = Uuid::new_v4();
        tokio::spawn(async move {
            bus2.emit(DdbEvent::RecordCreated {
                entity_id: eid,
                entity_type: "users".to_string(),
                attributes: HashMap::new(),
                tx_id: 1,
            })
            .await;
        });

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .unwrap();

        assert!(received.is_some());
        let received = received.unwrap();
        assert_eq!(received.entity_id(), Some(eid));
    }

    #[tokio::test]
    async fn event_bus_filtered_subscriber() {
        let bus = Arc::new(EventBus::new(16, 100));
        let mut sub = bus.subscribe_entity_type("orders");

        let bus2 = bus.clone();
        let oid = Uuid::new_v4();
        tokio::spawn(async move {
            // Emit a users event (should be filtered out).
            bus2.emit(DdbEvent::RecordCreated {
                entity_id: Uuid::new_v4(),
                entity_type: "users".to_string(),
                attributes: HashMap::new(),
                tx_id: 1,
            })
            .await;

            // Emit an orders event (should pass filter).
            bus2.emit(DdbEvent::RecordCreated {
                entity_id: oid,
                entity_type: "orders".to_string(),
                attributes: HashMap::new(),
                tx_id: 2,
            })
            .await;
        });

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
            .await
            .unwrap();

        assert!(received.is_some());
        assert_eq!(received.unwrap().entity_type(), Some("orders"));
    }

    #[tokio::test]
    async fn event_bus_log_persists() {
        let bus = EventBus::new(16, 100);
        let eid = Uuid::new_v4();

        bus.emit(DdbEvent::RecordCreated {
            entity_id: eid,
            entity_type: "users".to_string(),
            attributes: HashMap::new(),
            tx_id: 1,
        })
        .await;

        bus.emit(DdbEvent::RecordUpdated {
            entity_id: eid,
            entity_type: "users".to_string(),
            attributes: HashMap::new(),
            changed_fields: vec!["name".to_string()],
            tx_id: 2,
        })
        .await;

        let recent = bus.recent_events(10).await;
        assert_eq!(recent.len(), 2);

        let entity_events = bus.events_for_entity(eid).await;
        assert_eq!(entity_events.len(), 2);
    }

    #[tokio::test]
    async fn event_bus_log_bounded() {
        let bus = EventBus::new(16, 3); // Max 3 entries.

        for i in 0..5 {
            bus.emit(DdbEvent::Custom {
                kind: "test".to_string(),
                data: json!(i),
            })
            .await;
        }

        let recent = bus.recent_events(10).await;
        assert_eq!(recent.len(), 3); // Oldest two were evicted.
    }

    #[test]
    fn ddb_event_accessors() {
        let event = DdbEvent::RecordCreated {
            entity_id: Uuid::nil(),
            entity_type: "tasks".to_string(),
            attributes: HashMap::new(),
            tx_id: 42,
        };

        assert_eq!(event.entity_type(), Some("tasks"));
        assert_eq!(event.entity_id(), Some(Uuid::nil()));
        assert_eq!(event.tx_id(), Some(42));
    }

    #[test]
    fn ddb_event_accessors_non_entity() {
        let event = DdbEvent::AuthEvent {
            user_id: "u1".to_string(),
            action: "login".to_string(),
            metadata: HashMap::new(),
        };

        assert!(event.entity_type().is_none());
        assert!(event.entity_id().is_none());
        assert!(event.tx_id().is_none());
    }

    #[test]
    fn ddb_event_serde_roundtrip() {
        let event = DdbEvent::FieldChanged {
            entity_id: Uuid::new_v4(),
            entity_type: "orders".to_string(),
            field_name: "status".to_string(),
            old_value: Some(json!("pending")),
            new_value: json!("shipped"),
            attributes: HashMap::new(),
            tx_id: 10,
        };

        let json = serde_json::to_string(&event).unwrap();
        let restored: DdbEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.entity_type(), Some("orders"));
    }

    #[test]
    fn change_event_to_ddb_events_create() {
        let change = crate::sync::broadcaster::ChangeEvent {
            tx_id: 1,
            entity_ids: vec![Uuid::new_v4().to_string()],
            attributes: vec!["name".to_string()],
            entity_type: Some("users".to_string()),
            actor_id: None,
        };

        let events = change_event_to_ddb_events(&change, &HashMap::new(), true);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], DdbEvent::RecordCreated { .. }));
    }

    #[test]
    fn change_event_to_ddb_events_update() {
        let eid = Uuid::new_v4();
        let change = crate::sync::broadcaster::ChangeEvent {
            tx_id: 2,
            entity_ids: vec![eid.to_string()],
            attributes: vec!["status".to_string(), "updated_at".to_string()],
            entity_type: Some("orders".to_string()),
            actor_id: None,
        };

        let events = change_event_to_ddb_events(&change, &HashMap::new(), false);
        // 1 RecordUpdated + 2 FieldChanged
        assert_eq!(events.len(), 3);
        assert!(matches!(events[0], DdbEvent::RecordUpdated { .. }));
        assert!(matches!(events[1], DdbEvent::FieldChanged { .. }));
        assert!(matches!(events[2], DdbEvent::FieldChanged { .. }));
    }
}
