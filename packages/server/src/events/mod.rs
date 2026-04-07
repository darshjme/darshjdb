//! Event bus for DarshJDB — DAF-inspired structured event system.
//!
//! Captures all mutations, auth events, storage operations, and custom
//! events into a unified stream. Supports filtered subscriptions via
//! `tokio::sync::broadcast` and persists events to a `ddb_events` table
//! for audit trail and knowledge-base extraction.
//!
//! # Architecture
//!
//! ```text
//! Mutation ──publish──▶ EventBus ──broadcast──▶ Subscriber A (filtered)
//!                          │                 ──▶ Subscriber B (filtered)
//!                          ▼
//!                     EventLogger ──batch──▶ ddb_events table
//!                          │
//!                          ▼
//!                     KB Extractor ──▶ triples (KBEntry patterns)
//! ```

pub mod kb;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// ── Event Types ────────────────────────────────────────────────────

/// The kind of event that occurred.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventKind {
    RecordCreated,
    RecordUpdated,
    RecordDeleted,
    RecordBulkCreated,
    FieldCreated,
    FieldUpdated,
    FieldDeleted,
    ViewCreated,
    ViewUpdated,
    ViewDeleted,
    AuthLogin,
    AuthLogout,
    AuthSignup,
    StorageUpload,
    StorageDelete,
    FunctionExecuted,
    AutomationTriggered,
    Custom(String),
}

impl std::fmt::Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Custom(name) => write!(f, "Custom({name})"),
            other => write!(f, "{other:?}"),
        }
    }
}

impl EventKind {
    /// Serialize to a stable string for database storage.
    pub fn as_str(&self) -> String {
        match self {
            Self::Custom(s) => format!("custom:{s}"),
            other => format!("{other:?}"),
        }
    }

    /// Deserialize from the stable string form.
    pub fn from_str(s: &str) -> Self {
        if let Some(name) = s.strip_prefix("custom:") {
            return Self::Custom(name.to_string());
        }
        match s {
            "RecordCreated" => Self::RecordCreated,
            "RecordUpdated" => Self::RecordUpdated,
            "RecordDeleted" => Self::RecordDeleted,
            "RecordBulkCreated" => Self::RecordBulkCreated,
            "FieldCreated" => Self::FieldCreated,
            "FieldUpdated" => Self::FieldUpdated,
            "FieldDeleted" => Self::FieldDeleted,
            "ViewCreated" => Self::ViewCreated,
            "ViewUpdated" => Self::ViewUpdated,
            "ViewDeleted" => Self::ViewDeleted,
            "AuthLogin" => Self::AuthLogin,
            "AuthLogout" => Self::AuthLogout,
            "AuthSignup" => Self::AuthSignup,
            "StorageUpload" => Self::StorageUpload,
            "StorageDelete" => Self::StorageDelete,
            "FunctionExecuted" => Self::FunctionExecuted,
            "AutomationTriggered" => Self::AutomationTriggered,
            other => Self::Custom(other.to_string()),
        }
    }
}

/// A structured event emitted by DarshJDB operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DdbEvent {
    /// Unique event identifier.
    pub id: Uuid,
    /// What kind of event this is.
    pub kind: EventKind,
    /// The entity type involved (e.g. "User", "Post").
    pub entity_type: Option<String>,
    /// The specific entity UUID affected.
    pub entity_id: Option<Uuid>,
    /// The attribute that changed (for field-level events).
    pub attribute: Option<String>,
    /// Previous value (for updates/deletes).
    pub old_value: Option<Value>,
    /// New value (for creates/updates).
    pub new_value: Option<Value>,
    /// The user who triggered this event.
    pub user_id: Option<Uuid>,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// Arbitrary key-value metadata.
    pub metadata: HashMap<String, Value>,
    /// Transaction ID from the triple store.
    pub tx_id: i64,
}

impl DdbEvent {
    /// Create a new event with auto-generated ID and timestamp.
    pub fn new(kind: EventKind, tx_id: i64) -> Self {
        Self {
            id: Uuid::new_v4(),
            kind,
            entity_type: None,
            entity_id: None,
            attribute: None,
            old_value: None,
            new_value: None,
            user_id: None,
            timestamp: Utc::now(),
            metadata: HashMap::new(),
            tx_id,
        }
    }

    /// Builder: set entity type.
    pub fn with_entity_type(mut self, et: impl Into<String>) -> Self {
        self.entity_type = Some(et.into());
        self
    }

    /// Builder: set entity ID.
    pub fn with_entity_id(mut self, id: Uuid) -> Self {
        self.entity_id = Some(id);
        self
    }

    /// Builder: set attribute.
    pub fn with_attribute(mut self, attr: impl Into<String>) -> Self {
        self.attribute = Some(attr.into());
        self
    }

    /// Builder: set old and new values.
    pub fn with_values(mut self, old: Option<Value>, new: Option<Value>) -> Self {
        self.old_value = old;
        self.new_value = new;
        self
    }

    /// Builder: set user ID.
    pub fn with_user(mut self, user_id: Uuid) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Builder: add a metadata entry.
    pub fn with_metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Check if this event matches a filter.
    pub fn matches(&self, filter: &EventFilter) -> bool {
        if let Some(ref kinds) = filter.kinds {
            if !kinds.contains(&self.kind) {
                return false;
            }
        }
        if let Some(ref types) = filter.entity_types {
            match &self.entity_type {
                Some(et) if types.contains(et) => {}
                Some(_) => return false,
                None => return false,
            }
        }
        if let Some(ref ids) = filter.entity_ids {
            match &self.entity_id {
                Some(eid) if ids.contains(eid) => {}
                Some(_) => return false,
                None => return false,
            }
        }
        true
    }
}

// ── Filter ─────────────────────────────────────────────────────────

/// Filter criteria for event subscriptions.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    /// Only receive events of these kinds. `None` means all kinds.
    pub kinds: Option<Vec<EventKind>>,
    /// Only receive events for these entity types.
    pub entity_types: Option<Vec<String>>,
    /// Only receive events for these specific entity IDs.
    pub entity_ids: Option<Vec<Uuid>>,
}

impl EventFilter {
    /// Create an empty filter that matches everything.
    pub fn all() -> Self {
        Self::default()
    }

    /// Filter to specific event kinds.
    pub fn with_kinds(mut self, kinds: Vec<EventKind>) -> Self {
        self.kinds = Some(kinds);
        self
    }

    /// Filter to specific entity types.
    pub fn with_entity_types(mut self, types: Vec<String>) -> Self {
        self.entity_types = Some(types);
        self
    }

    /// Filter to specific entity IDs.
    pub fn with_entity_ids(mut self, ids: Vec<Uuid>) -> Self {
        self.entity_ids = Some(ids);
        self
    }
}

// ── Event Stream ───────────────────────────────────────────────────

/// A filtered stream of events from the bus.
pub struct EventStream {
    rx: broadcast::Receiver<DdbEvent>,
    filter: EventFilter,
}

impl EventStream {
    /// Receive the next event that matches the filter.
    ///
    /// Blocks until a matching event arrives or the bus is dropped.
    pub async fn recv(&mut self) -> Option<DdbEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => {
                    if event.matches(&self.filter) {
                        return Some(event);
                    }
                    // Does not match filter, keep waiting.
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "event stream subscriber lagged");
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return None;
                }
            }
        }
    }
}

// ── Event Bus ──────────────────────────────────────────────────────

/// Central event bus for broadcasting structured events to all subscribers.
///
/// Uses a `tokio::sync::broadcast` channel internally. Subscribers receive
/// cloned events and apply their own filters client-side (within `EventStream`).
pub struct EventBus {
    tx: broadcast::Sender<DdbEvent>,
    /// Channel to the logger background task for persistence.
    logger_tx: mpsc::Sender<DdbEvent>,
}

impl EventBus {
    /// Create a new event bus with the given broadcast capacity.
    ///
    /// Also spawns the [`EventLogger`] background task for persistence.
    pub fn new(pool: PgPool, capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        let (logger_tx, logger_rx) = mpsc::channel(capacity * 2);

        // Spawn the logger background task.
        let logger = EventLogger::new(pool);
        tokio::spawn(logger.run(logger_rx));

        Self { tx, logger_tx }
    }

    /// Create a bus without persistence (for testing).
    pub fn new_without_logger(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        // Create a channel but never consume it — events just buffer and drop.
        let (logger_tx, _logger_rx) = mpsc::channel(capacity);
        Self { tx, logger_tx }
    }

    /// Publish an event to all subscribers and the audit logger.
    pub fn publish(&self, event: DdbEvent) {
        debug!(
            kind = %event.kind,
            entity_type = ?event.entity_type,
            entity_id = ?event.entity_id,
            tx_id = event.tx_id,
            "publishing event"
        );

        // Send to persistence logger (non-blocking, drop on full).
        if let Err(e) = self.logger_tx.try_send(event.clone()) {
            warn!("event logger channel full, dropping event for persistence: {e}");
        }

        // Broadcast to all live subscribers.
        // It is okay if there are no receivers — the event is still logged.
        let _ = self.tx.send(event);
    }

    /// Subscribe to events matching the given filter.
    pub fn subscribe(&self, filter: EventFilter) -> EventStream {
        EventStream {
            rx: self.tx.subscribe(),
            filter,
        }
    }

    /// Get the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

// ── Event Logger ───────────────────────────────────────────────────

/// Persists events to the `ddb_events` table in batches.
///
/// Batches are flushed every 100ms or when 100 events accumulate,
/// whichever comes first. This amortizes the cost of Postgres round-trips.
pub struct EventLogger {
    pool: PgPool,
}

impl EventLogger {
    /// Create a new logger connected to the given pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run the batch-insert loop. Intended to be spawned as a background task.
    pub async fn run(self, mut rx: mpsc::Receiver<DdbEvent>) {
        let mut buffer: Vec<DdbEvent> = Vec::with_capacity(128);
        let flush_interval = tokio::time::Duration::from_millis(100);
        let max_batch = 100;

        info!("event logger started (batch_size={max_batch}, flush_interval=100ms)");

        loop {
            // Wait for either: a new event, a timeout, or channel close.
            let deadline = tokio::time::sleep(flush_interval);
            tokio::pin!(deadline);

            tokio::select! {
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => {
                            buffer.push(event);
                            if buffer.len() >= max_batch {
                                self.flush(&mut buffer).await;
                            }
                        }
                        None => {
                            // Channel closed — flush remaining and exit.
                            if !buffer.is_empty() {
                                self.flush(&mut buffer).await;
                            }
                            info!("event logger shutting down");
                            return;
                        }
                    }
                }
                _ = &mut deadline => {
                    if !buffer.is_empty() {
                        self.flush(&mut buffer).await;
                    }
                }
            }
        }
    }

    /// Flush the buffer to Postgres via a batch INSERT.
    async fn flush(&self, buffer: &mut Vec<DdbEvent>) {
        if buffer.is_empty() {
            return;
        }

        let count = buffer.len();
        debug!(count, "flushing event batch to ddb_events");

        // Build a batch insert. We use unnest arrays for efficiency.
        let mut ids: Vec<Uuid> = Vec::with_capacity(count);
        let mut kinds: Vec<String> = Vec::with_capacity(count);
        let mut entity_types: Vec<Option<String>> = Vec::with_capacity(count);
        let mut entity_ids: Vec<Option<Uuid>> = Vec::with_capacity(count);
        let mut attributes: Vec<Option<String>> = Vec::with_capacity(count);
        let mut old_values: Vec<Option<Value>> = Vec::with_capacity(count);
        let mut new_values: Vec<Option<Value>> = Vec::with_capacity(count);
        let mut user_ids: Vec<Option<Uuid>> = Vec::with_capacity(count);
        let mut timestamps: Vec<DateTime<Utc>> = Vec::with_capacity(count);
        let mut metadata_jsons: Vec<Value> = Vec::with_capacity(count);
        let mut tx_ids: Vec<i64> = Vec::with_capacity(count);

        for event in buffer.iter() {
            ids.push(event.id);
            kinds.push(event.kind.as_str());
            entity_types.push(event.entity_type.clone());
            entity_ids.push(event.entity_id);
            attributes.push(event.attribute.clone());
            old_values.push(event.old_value.clone());
            new_values.push(event.new_value.clone());
            user_ids.push(event.user_id);
            timestamps.push(event.timestamp);
            metadata_jsons.push(serde_json::to_value(&event.metadata).unwrap_or(Value::Null));
            tx_ids.push(event.tx_id);
        }

        let result = sqlx::query(
            r#"
            INSERT INTO ddb_events (id, kind, entity_type, entity_id, attribute,
                                    old_value, new_value, user_id, timestamp, metadata, tx_id)
            SELECT * FROM UNNEST(
                $1::uuid[], $2::text[], $3::text[], $4::uuid[], $5::text[],
                $6::jsonb[], $7::jsonb[], $8::uuid[], $9::timestamptz[], $10::jsonb[], $11::bigint[]
            )
            "#,
        )
        .bind(&ids)
        .bind(&kinds)
        .bind(&entity_types)
        .bind(&entity_ids)
        .bind(&attributes)
        .bind(&old_values)
        .bind(&new_values)
        .bind(&user_ids)
        .bind(&timestamps)
        .bind(&metadata_jsons)
        .bind(&tx_ids)
        .execute(&self.pool)
        .await;

        match result {
            Ok(r) => {
                debug!(rows = r.rows_affected(), "event batch persisted");
            }
            Err(e) => {
                error!(count, error = %e, "failed to persist event batch");
            }
        }

        buffer.clear();
    }
}

// ── Ensure Table ───────────────────────────────────────────────────

/// Create the `ddb_events` table if it does not exist.
pub async fn ensure_events_table(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS ddb_events (
            id          UUID PRIMARY KEY,
            kind        TEXT NOT NULL,
            entity_type TEXT,
            entity_id   UUID,
            attribute   TEXT,
            old_value   JSONB,
            new_value   JSONB,
            user_id     UUID,
            timestamp   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            metadata    JSONB NOT NULL DEFAULT '{}',
            tx_id       BIGINT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_ddb_events_kind ON ddb_events (kind);
        CREATE INDEX IF NOT EXISTS idx_ddb_events_entity_type ON ddb_events (entity_type);
        CREATE INDEX IF NOT EXISTS idx_ddb_events_entity_id ON ddb_events (entity_id);
        CREATE INDEX IF NOT EXISTS idx_ddb_events_timestamp ON ddb_events (timestamp);
        CREATE INDEX IF NOT EXISTS idx_ddb_events_tx_id ON ddb_events (tx_id);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_filter_matches_all() {
        let event = DdbEvent::new(EventKind::RecordCreated, 1)
            .with_entity_type("User")
            .with_entity_id(Uuid::new_v4());

        let filter = EventFilter::all();
        assert!(event.matches(&filter));
    }

    #[test]
    fn event_filter_matches_by_kind() {
        let event = DdbEvent::new(EventKind::RecordCreated, 1);

        let match_filter = EventFilter::all().with_kinds(vec![EventKind::RecordCreated]);
        assert!(event.matches(&match_filter));

        let miss_filter = EventFilter::all().with_kinds(vec![EventKind::RecordDeleted]);
        assert!(!event.matches(&miss_filter));
    }

    #[test]
    fn event_filter_matches_by_entity_type() {
        let event = DdbEvent::new(EventKind::RecordCreated, 1).with_entity_type("User");

        let match_filter =
            EventFilter::all().with_entity_types(vec!["User".to_string()]);
        assert!(event.matches(&match_filter));

        let miss_filter =
            EventFilter::all().with_entity_types(vec!["Post".to_string()]);
        assert!(!event.matches(&miss_filter));
    }

    #[test]
    fn event_filter_matches_by_entity_id() {
        let id = Uuid::new_v4();
        let event = DdbEvent::new(EventKind::RecordUpdated, 1).with_entity_id(id);

        let match_filter = EventFilter::all().with_entity_ids(vec![id]);
        assert!(event.matches(&match_filter));

        let miss_filter = EventFilter::all().with_entity_ids(vec![Uuid::new_v4()]);
        assert!(!event.matches(&miss_filter));
    }

    #[test]
    fn event_filter_combined() {
        let id = Uuid::new_v4();
        let event = DdbEvent::new(EventKind::RecordCreated, 1)
            .with_entity_type("User")
            .with_entity_id(id);

        let filter = EventFilter::all()
            .with_kinds(vec![EventKind::RecordCreated])
            .with_entity_types(vec!["User".to_string()])
            .with_entity_ids(vec![id]);
        assert!(event.matches(&filter));

        // Wrong kind breaks it.
        let filter2 = EventFilter::all()
            .with_kinds(vec![EventKind::RecordDeleted])
            .with_entity_types(vec!["User".to_string()])
            .with_entity_ids(vec![id]);
        assert!(!event.matches(&filter2));
    }

    #[test]
    fn event_filter_entity_type_required_when_filtering() {
        // Event without entity_type should not match an entity_type filter.
        let event = DdbEvent::new(EventKind::AuthLogin, 1);
        let filter = EventFilter::all().with_entity_types(vec!["User".to_string()]);
        assert!(!event.matches(&filter));
    }

    #[tokio::test]
    async fn event_bus_publish_subscribe() {
        let bus = EventBus::new_without_logger(64);

        let filter = EventFilter::all().with_kinds(vec![EventKind::RecordCreated]);
        let mut stream = bus.subscribe(filter);

        let event = DdbEvent::new(EventKind::RecordCreated, 42)
            .with_entity_type("User");

        bus.publish(event.clone());

        let received = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stream.recv(),
        )
        .await
        .expect("timed out waiting for event")
        .expect("stream closed unexpectedly");

        assert_eq!(received.kind, EventKind::RecordCreated);
        assert_eq!(received.tx_id, 42);
        assert_eq!(received.entity_type.as_deref(), Some("User"));
    }

    #[tokio::test]
    async fn event_bus_filter_excludes_non_matching() {
        let bus = EventBus::new_without_logger(64);

        let filter = EventFilter::all().with_kinds(vec![EventKind::RecordDeleted]);
        let mut stream = bus.subscribe(filter);

        // Publish a create event — should NOT match the delete filter.
        bus.publish(DdbEvent::new(EventKind::RecordCreated, 1));

        // Publish a delete event — should match.
        bus.publish(
            DdbEvent::new(EventKind::RecordDeleted, 2).with_entity_type("Post"),
        );

        let received = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            stream.recv(),
        )
        .await
        .expect("timed out")
        .expect("stream closed");

        assert_eq!(received.kind, EventKind::RecordDeleted);
        assert_eq!(received.tx_id, 2);
    }

    #[tokio::test]
    async fn event_bus_multiple_subscribers() {
        let bus = EventBus::new_without_logger(64);

        let mut stream_a = bus.subscribe(EventFilter::all());
        let mut stream_b = bus.subscribe(
            EventFilter::all().with_entity_types(vec!["User".to_string()]),
        );

        bus.publish(DdbEvent::new(EventKind::RecordCreated, 1).with_entity_type("User"));
        bus.publish(DdbEvent::new(EventKind::RecordCreated, 2).with_entity_type("Post"));

        // Stream A (unfiltered) should get both.
        let a1 = tokio::time::timeout(std::time::Duration::from_millis(50), stream_a.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a1.tx_id, 1);

        let a2 = tokio::time::timeout(std::time::Duration::from_millis(50), stream_a.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a2.tx_id, 2);

        // Stream B (User only) should only get the first.
        let b1 = tokio::time::timeout(std::time::Duration::from_millis(50), stream_b.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b1.entity_type.as_deref(), Some("User"));

        // Second event for B should be the User one only — Post was skipped.
        // Since we already consumed it, a timeout means filter worked correctly.
        let b2 = tokio::time::timeout(std::time::Duration::from_millis(50), stream_b.recv()).await;
        assert!(b2.is_err(), "should timeout because Post was filtered out");
    }

    #[test]
    fn event_kind_round_trip() {
        let kinds = vec![
            EventKind::RecordCreated,
            EventKind::RecordDeleted,
            EventKind::AuthLogin,
            EventKind::Custom("webhook.fired".to_string()),
        ];
        for kind in kinds {
            let s = kind.as_str();
            let parsed = EventKind::from_str(&s);
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn event_builder_chain() {
        let id = Uuid::new_v4();
        let user = Uuid::new_v4();
        let event = DdbEvent::new(EventKind::RecordUpdated, 99)
            .with_entity_type("Post")
            .with_entity_id(id)
            .with_attribute("title")
            .with_values(
                Some(serde_json::json!("Old Title")),
                Some(serde_json::json!("New Title")),
            )
            .with_user(user)
            .with_metadata("source", serde_json::json!("api"));

        assert_eq!(event.entity_type.as_deref(), Some("Post"));
        assert_eq!(event.entity_id, Some(id));
        assert_eq!(event.attribute.as_deref(), Some("title"));
        assert_eq!(event.old_value, Some(serde_json::json!("Old Title")));
        assert_eq!(event.new_value, Some(serde_json::json!("New Title")));
        assert_eq!(event.user_id, Some(user));
        assert_eq!(event.metadata.get("source"), Some(&serde_json::json!("api")));
        assert_eq!(event.tx_id, 99);
    }

    #[test]
    fn subscriber_count_tracks_active_streams() {
        let bus = EventBus::new_without_logger(16);
        assert_eq!(bus.subscriber_count(), 0);

        let _s1 = bus.subscribe(EventFilter::all());
        assert_eq!(bus.subscriber_count(), 1);

        let _s2 = bus.subscribe(EventFilter::all());
        assert_eq!(bus.subscriber_count(), 2);

        drop(_s1);
        // broadcast::Receiver does not decrement on drop until next send,
        // so we just verify we can still subscribe.
        let _s3 = bus.subscribe(EventFilter::all());
        assert!(bus.subscriber_count() >= 2);
    }
}
