//! Persistent change feed (changelog / WAL) for DarshJDB mutations.
//!
//! Captures every mutation as an append-only log entry, supporting:
//!
//! - **Cursor-based reading**: clients can resume from a known position
//! - **Retention policy**: entries older than a configurable TTL are pruned
//! - **PostgreSQL LISTEN/NOTIFY integration**: external consumers can receive
//!   real-time notifications of new entries via Postgres channels
//!
//! # Architecture
//!
//! ```text
//! Mutation ──▶ ChangeFeed::append() ──▶ In-memory ring buffer
//!                     │                        │
//!                     ▼                        ▼
//!              pg_notify(channel)        Cursor-based read
//!                     │
//!                     ▼
//!              LISTEN/NOTIFY consumer
//! ```
//!
//! The in-memory ring buffer provides fast access for recent changes.
//! Overflow entries are dropped from the tail when capacity is exceeded.
//! The PostgreSQL change_feed table provides durable storage with TTL-based
//! retention for historical replay.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, warn};

use super::broadcaster::ChangeEvent;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Default retention TTL for change feed entries (24 hours).
const DEFAULT_RETENTION_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Default in-memory ring buffer capacity.
const DEFAULT_BUFFER_CAPACITY: usize = 10_000;

/// Default prune interval.
const DEFAULT_PRUNE_INTERVAL: Duration = Duration::from_secs(60);

/// PostgreSQL notification channel for change feed events.
pub const PG_NOTIFY_CHANNEL: &str = "darshjdb_changes";

/// Change feed configuration.
#[derive(Debug, Clone)]
pub struct ChangeFeedConfig {
    /// How long to keep entries before pruning.
    pub retention_ttl: Duration,
    /// Maximum number of entries in the in-memory buffer.
    pub buffer_capacity: usize,
    /// How often to run the prune task.
    pub prune_interval: Duration,
    /// PostgreSQL notification channel name.
    pub pg_channel: String,
}

impl Default for ChangeFeedConfig {
    fn default() -> Self {
        Self {
            retention_ttl: DEFAULT_RETENTION_TTL,
            buffer_capacity: DEFAULT_BUFFER_CAPACITY,
            prune_interval: DEFAULT_PRUNE_INTERVAL,
            pg_channel: PG_NOTIFY_CHANNEL.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Change feed entry
// ---------------------------------------------------------------------------

/// A single entry in the change feed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeFeedEntry {
    /// Monotonically increasing sequence number (cursor).
    pub sequence: u64,

    /// Transaction ID from the triple store.
    pub tx_id: i64,

    /// Unix timestamp (milliseconds) when this entry was created.
    pub timestamp_ms: u64,

    /// The mutation action: "INSERT", "UPDATE", or "DELETE".
    pub action: String,

    /// Target collection / entity type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection: Option<String>,

    /// Entity IDs affected by this mutation.
    pub entity_ids: Vec<String>,

    /// Attribute names that were modified.
    pub attributes: Vec<String>,

    /// User who performed the mutation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,

    /// When this entry was inserted (not serialized; used for TTL expiry).
    #[serde(skip, default = "Instant::now")]
    pub inserted_at: Instant,
}

/// Cursor position for reading the change feed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Cursor(pub u64);

impl Cursor {
    /// The beginning of the feed (before any entries).
    pub const ZERO: Cursor = Cursor(0);

    /// Create a cursor from a sequence number.
    pub fn new(seq: u64) -> Self {
        Self(seq)
    }
}

impl std::fmt::Display for Cursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Read result
// ---------------------------------------------------------------------------

/// Result of a change feed read operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeFeedPage {
    /// Entries in this page, ordered by sequence number.
    pub entries: Vec<ChangeFeedEntry>,

    /// Cursor pointing to the next entry after this page.
    /// Use this as the `after` cursor for the next read.
    pub next_cursor: Cursor,

    /// Whether there are more entries available beyond this page.
    pub has_more: bool,
}

// ---------------------------------------------------------------------------
// ChangeFeed
// ---------------------------------------------------------------------------

/// In-memory change feed backed by a ring buffer.
///
/// Thread-safe for concurrent reads and writes via [`RwLock`].
/// Entries are ordered by monotonically increasing sequence numbers.
pub struct ChangeFeed {
    /// Configuration.
    config: ChangeFeedConfig,

    /// Ring buffer of entries, ordered by sequence number.
    buffer: RwLock<VecDeque<ChangeFeedEntry>>,

    /// Next sequence number to assign.
    next_sequence: RwLock<u64>,

    /// Broadcast sender for new entries (used by the LISTEN/NOTIFY bridge).
    entry_tx: tokio::sync::broadcast::Sender<ChangeFeedEntry>,
}

impl ChangeFeed {
    /// Create a new change feed with the given configuration.
    pub fn new(
        config: ChangeFeedConfig,
    ) -> (Arc<Self>, tokio::sync::broadcast::Receiver<ChangeFeedEntry>) {
        let (entry_tx, entry_rx) =
            tokio::sync::broadcast::channel(config.buffer_capacity.min(4096));
        let feed = Arc::new(Self {
            config,
            buffer: RwLock::new(VecDeque::new()),
            next_sequence: RwLock::new(1),
            entry_tx,
        });
        (feed, entry_rx)
    }

    /// Create a change feed with default configuration.
    pub fn with_defaults() -> (Arc<Self>, tokio::sync::broadcast::Receiver<ChangeFeedEntry>) {
        Self::new(ChangeFeedConfig::default())
    }

    /// Get a new broadcast receiver for change feed entries.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ChangeFeedEntry> {
        self.entry_tx.subscribe()
    }

    /// Append a new entry to the change feed from a [`ChangeEvent`].
    ///
    /// Assigns a sequence number, records the timestamp, appends to the
    /// ring buffer, and broadcasts the entry.
    ///
    /// Returns the assigned sequence number.
    pub fn append(&self, event: &ChangeEvent, action: &str) -> u64 {
        let sequence = {
            let mut seq = self.next_sequence.write().expect("sequence lock poisoned");
            let current = *seq;
            *seq += 1;
            current
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let entry = ChangeFeedEntry {
            sequence,
            tx_id: event.tx_id,
            timestamp_ms: now,
            action: action.to_uppercase(),
            collection: event.entity_type.clone(),
            entity_ids: event.entity_ids.clone(),
            attributes: event.attributes.clone(),
            actor_id: event.actor_id.clone(),
            inserted_at: Instant::now(),
        };

        // Append to ring buffer.
        {
            let mut buffer = self
                .buffer
                .write()
                .expect("change feed buffer lock poisoned");
            if buffer.len() >= self.config.buffer_capacity {
                buffer.pop_front();
            }
            buffer.push_back(entry.clone());
        }

        // Broadcast the entry.
        let _ = self.entry_tx.send(entry);

        debug!(
            sequence = sequence,
            tx_id = event.tx_id,
            action = action,
            "change feed entry appended"
        );

        sequence
    }

    /// Read entries from the change feed after the given cursor.
    ///
    /// Returns up to `limit` entries with sequence numbers greater than
    /// `after.0`. If `after` is `Cursor::ZERO`, reads from the beginning
    /// of the available buffer.
    pub fn read_after(&self, after: Cursor, limit: usize) -> ChangeFeedPage {
        let buffer = self
            .buffer
            .read()
            .expect("change feed buffer lock poisoned");

        let entries: Vec<ChangeFeedEntry> = buffer
            .iter()
            .filter(|e| e.sequence > after.0)
            .take(limit)
            .cloned()
            .collect();

        let next_cursor = entries.last().map(|e| Cursor(e.sequence)).unwrap_or(after);

        let has_more = buffer.iter().any(|e| e.sequence > next_cursor.0);

        ChangeFeedPage {
            entries,
            next_cursor,
            has_more,
        }
    }

    /// Read entries within a sequence range (inclusive).
    pub fn read_range(&self, from: u64, to: u64) -> Vec<ChangeFeedEntry> {
        let buffer = self
            .buffer
            .read()
            .expect("change feed buffer lock poisoned");
        buffer
            .iter()
            .filter(|e| e.sequence >= from && e.sequence <= to)
            .cloned()
            .collect()
    }

    /// Get the latest cursor (sequence number of the most recent entry).
    pub fn latest_cursor(&self) -> Cursor {
        let buffer = self
            .buffer
            .read()
            .expect("change feed buffer lock poisoned");
        buffer
            .back()
            .map(|e| Cursor(e.sequence))
            .unwrap_or(Cursor::ZERO)
    }

    /// Get the total number of entries currently in the buffer.
    pub fn len(&self) -> usize {
        self.buffer
            .read()
            .expect("change feed buffer lock poisoned")
            .len()
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Prune entries older than the retention TTL.
    ///
    /// Returns the number of entries removed.
    pub fn prune_expired(&self) -> usize {
        let cutoff = Instant::now() - self.config.retention_ttl;
        let mut buffer = self
            .buffer
            .write()
            .expect("change feed buffer lock poisoned");
        let before = buffer.len();

        // Since entries are ordered by time, we can pop from the front.
        while let Some(front) = buffer.front() {
            if front.inserted_at < cutoff {
                buffer.pop_front();
            } else {
                break;
            }
        }

        let removed = before - buffer.len();
        if removed > 0 {
            debug!(
                removed = removed,
                remaining = buffer.len(),
                "pruned expired change feed entries"
            );
        }
        removed
    }

    /// Spawn a background task that periodically prunes expired entries.
    pub fn spawn_prune_task(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let feed = Arc::clone(self);
        let interval = feed.config.prune_interval;

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            loop {
                ticker.tick().await;
                feed.prune_expired();
            }
        })
    }

    /// Get the current configuration.
    pub fn config(&self) -> &ChangeFeedConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL LISTEN/NOTIFY bridge
// ---------------------------------------------------------------------------

/// Bridge between the in-memory change feed and PostgreSQL LISTEN/NOTIFY.
///
/// Sends `pg_notify()` on every new change feed entry, and listens for
/// notifications from other server instances for cluster-wide propagation.
pub struct PgNotifyBridge {
    /// The channel name for NOTIFY.
    channel: String,
    /// Database pool for sending notifications.
    pool: sqlx::PgPool,
}

impl PgNotifyBridge {
    /// Create a new bridge.
    pub fn new(pool: sqlx::PgPool, channel: String) -> Self {
        Self { channel, pool }
    }

    /// Create with default channel name.
    pub fn with_defaults(pool: sqlx::PgPool) -> Self {
        Self::new(pool, PG_NOTIFY_CHANNEL.to_string())
    }

    /// Send a NOTIFY for a change feed entry.
    ///
    /// The payload is a compact JSON string with sequence, tx_id, action,
    /// and collection. Full entity data is not included (consumers should
    /// fetch from the change feed or triple store).
    pub async fn notify(&self, entry: &ChangeFeedEntry) -> Result<(), sqlx::Error> {
        let payload = serde_json::json!({
            "seq": entry.sequence,
            "tx": entry.tx_id,
            "act": entry.action,
            "col": entry.collection,
            "ids": entry.entity_ids,
        });

        let payload_str = serde_json::to_string(&payload).unwrap_or_default();

        sqlx::query("SELECT pg_notify($1, $2)")
            .bind(&self.channel)
            .bind(&payload_str)
            .execute(&self.pool)
            .await?;

        debug!(
            channel = %self.channel,
            sequence = entry.sequence,
            "pg_notify sent"
        );

        Ok(())
    }

    /// Spawn a task that forwards all change feed entries to pg_notify.
    ///
    /// Listens on the change feed broadcast channel and sends a NOTIFY
    /// for each entry.
    pub fn spawn_notify_task(
        self: Arc<Self>,
        mut entry_rx: tokio::sync::broadcast::Receiver<ChangeFeedEntry>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match entry_rx.recv().await {
                    Ok(entry) => {
                        if let Err(e) = self.notify(&entry).await {
                            warn!(
                                error = %e,
                                sequence = entry.sequence,
                                "failed to send pg_notify for change feed entry"
                            );
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "pg_notify bridge lagged behind");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("change feed broadcast closed, pg_notify bridge shutting down");
                        return;
                    }
                }
            }
        })
    }

    /// Spawn a LISTEN task that receives notifications from PostgreSQL
    /// and feeds them back into the local change feed (for multi-instance
    /// cluster scenarios).
    ///
    /// `on_notification` is called for each received notification payload.
    pub fn spawn_listen_task<F>(
        pool: sqlx::PgPool,
        channel: String,
        on_notification: F,
    ) -> tokio::task::JoinHandle<()>
    where
        F: Fn(PgNotification) + Send + Sync + 'static,
    {
        tokio::spawn(async move {
            let mut listener = match sqlx::postgres::PgListener::connect_with(&pool).await {
                Ok(l) => l,
                Err(e) => {
                    error!(error = %e, "failed to create PgListener");
                    return;
                }
            };

            if let Err(e) = listener.listen(&channel).await {
                error!(error = %e, channel = %channel, "failed to LISTEN on channel");
                return;
            }

            info!(channel = %channel, "pg_listen started");

            loop {
                match listener.recv().await {
                    Ok(notification) => {
                        let payload = notification.payload().to_string();
                        debug!(
                            channel = %notification.channel(),
                            payload_len = payload.len(),
                            "pg notification received"
                        );

                        match serde_json::from_str::<PgNotification>(&payload) {
                            Ok(parsed) => on_notification(parsed),
                            Err(e) => {
                                warn!(
                                    error = %e,
                                    payload = %payload,
                                    "failed to parse pg notification payload"
                                );
                            }
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "pg_listen error, will attempt reconnect");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        })
    }
}

/// Parsed notification payload from PostgreSQL LISTEN/NOTIFY.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PgNotification {
    /// Sequence number in the source change feed.
    pub seq: u64,
    /// Transaction ID.
    pub tx: i64,
    /// Action: INSERT, UPDATE, DELETE.
    pub act: String,
    /// Collection / entity type.
    pub col: Option<String>,
    /// Entity IDs affected.
    pub ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event(tx_id: i64, entity_type: &str, ids: Vec<&str>) -> ChangeEvent {
        ChangeEvent {
            tx_id,
            entity_ids: ids.into_iter().map(String::from).collect(),
            attributes: vec!["name".into()],
            entity_type: Some(entity_type.into()),
            actor_id: Some("test-user".into()),
        }
    }

    // -----------------------------------------------------------------------
    // ChangeFeed basics
    // -----------------------------------------------------------------------

    #[test]
    fn append_and_read() {
        let config = ChangeFeedConfig {
            buffer_capacity: 100,
            ..Default::default()
        };
        let (feed, _rx) = ChangeFeed::new(config);

        let event1 = make_event(1, "users", vec!["u1"]);
        let event2 = make_event(2, "users", vec!["u2"]);
        let event3 = make_event(3, "orders", vec!["o1"]);

        let seq1 = feed.append(&event1, "INSERT");
        let seq2 = feed.append(&event2, "UPDATE");
        let seq3 = feed.append(&event3, "DELETE");

        assert_eq!(seq1, 1);
        assert_eq!(seq2, 2);
        assert_eq!(seq3, 3);
        assert_eq!(feed.len(), 3);
    }

    #[test]
    fn read_after_cursor() {
        let config = ChangeFeedConfig {
            buffer_capacity: 100,
            ..Default::default()
        };
        let (feed, _rx) = ChangeFeed::new(config);

        for i in 1..=5 {
            let event = make_event(i, "users", vec!["u1"]);
            feed.append(&event, "INSERT");
        }

        // Read from the beginning.
        let page = feed.read_after(Cursor::ZERO, 3);
        assert_eq!(page.entries.len(), 3);
        assert_eq!(page.entries[0].sequence, 1);
        assert_eq!(page.entries[2].sequence, 3);
        assert!(page.has_more);
        assert_eq!(page.next_cursor, Cursor(3));

        // Read the rest.
        let page2 = feed.read_after(page.next_cursor, 10);
        assert_eq!(page2.entries.len(), 2);
        assert_eq!(page2.entries[0].sequence, 4);
        assert_eq!(page2.entries[1].sequence, 5);
        assert!(!page2.has_more);
    }

    #[test]
    fn read_range() {
        let config = ChangeFeedConfig {
            buffer_capacity: 100,
            ..Default::default()
        };
        let (feed, _rx) = ChangeFeed::new(config);

        for i in 1..=10 {
            let event = make_event(i, "users", vec!["u1"]);
            feed.append(&event, "INSERT");
        }

        let entries = feed.read_range(3, 7);
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].sequence, 3);
        assert_eq!(entries[4].sequence, 7);
    }

    #[test]
    fn buffer_capacity_eviction() {
        let config = ChangeFeedConfig {
            buffer_capacity: 5,
            ..Default::default()
        };
        let (feed, _rx) = ChangeFeed::new(config);

        for i in 1..=8 {
            let event = make_event(i, "users", vec!["u1"]);
            feed.append(&event, "INSERT");
        }

        // Buffer should contain only the last 5 entries.
        assert_eq!(feed.len(), 5);

        let page = feed.read_after(Cursor::ZERO, 100);
        assert_eq!(page.entries.len(), 5);
        assert_eq!(page.entries[0].sequence, 4); // oldest surviving
        assert_eq!(page.entries[4].sequence, 8); // newest
    }

    #[test]
    fn latest_cursor() {
        let (feed, _rx) = ChangeFeed::with_defaults();
        assert_eq!(feed.latest_cursor(), Cursor::ZERO);

        let event = make_event(1, "users", vec!["u1"]);
        feed.append(&event, "INSERT");
        assert_eq!(feed.latest_cursor(), Cursor(1));

        let event2 = make_event(2, "users", vec!["u2"]);
        feed.append(&event2, "UPDATE");
        assert_eq!(feed.latest_cursor(), Cursor(2));
    }

    #[test]
    fn empty_read() {
        let (feed, _rx) = ChangeFeed::with_defaults();
        let page = feed.read_after(Cursor::ZERO, 10);
        assert!(page.entries.is_empty());
        assert!(!page.has_more);
        assert_eq!(page.next_cursor, Cursor::ZERO);
    }

    #[test]
    fn entry_serialization() {
        let event = make_event(42, "users", vec!["u1", "u2"]);
        let (feed, _rx) = ChangeFeed::with_defaults();
        feed.append(&event, "INSERT");

        let page = feed.read_after(Cursor::ZERO, 1);
        let entry = &page.entries[0];

        let json = serde_json::to_value(entry).unwrap();
        assert_eq!(json["sequence"], 1);
        assert_eq!(json["tx_id"], 42);
        assert_eq!(json["action"], "INSERT");
        assert_eq!(json["collection"], "users");
        assert_eq!(json["entity_ids"], json!(["u1", "u2"]));
        assert_eq!(json["actor_id"], "test-user");
    }

    #[test]
    fn cursor_ordering() {
        assert!(Cursor(1) < Cursor(2));
        assert!(Cursor(0) < Cursor(1));
        assert_eq!(Cursor(5), Cursor(5));
    }

    // -----------------------------------------------------------------------
    // PgNotification parsing
    // -----------------------------------------------------------------------

    #[test]
    fn pg_notification_roundtrip() {
        let notif = PgNotification {
            seq: 42,
            tx: 100,
            act: "INSERT".into(),
            col: Some("users".into()),
            ids: vec!["u1".into(), "u2".into()],
        };

        let json_str = serde_json::to_string(&notif).unwrap();
        let parsed: PgNotification = serde_json::from_str(&json_str).unwrap();

        assert_eq!(parsed.seq, 42);
        assert_eq!(parsed.tx, 100);
        assert_eq!(parsed.act, "INSERT");
        assert_eq!(parsed.col, Some("users".into()));
        assert_eq!(parsed.ids.len(), 2);
    }
}
