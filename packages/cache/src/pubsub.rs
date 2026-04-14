// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache::pubsub — channel-based pub/sub for keyspace notifications.
//
// Slice 10 (Phase 1.3): minimal, dependency-free pub/sub built on
// `tokio::sync::broadcast`. Subscribers register per-channel and receive
// every message published after subscription. When all receivers for a
// channel are dropped the channel is garbage-collected on next publish.
//
// Author: Darshankumar Joshi

use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Default capacity for each per-channel broadcast buffer.
///
/// Chosen large enough to absorb short publisher bursts without slow
/// subscribers triggering `RecvError::Lagged`.
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

/// A message delivered to subscribers of a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubSubMessage {
    /// The channel the message was published on.
    pub channel: String,
    /// The opaque payload.
    pub payload: bytes::Bytes,
}

/// Channel-based pub/sub engine.
///
/// Used by the unified `DdbCache` to fan out keyspace notifications
/// (`__keyspace@0__:<key>`, invalidation broadcasts, etc.) to any number
/// of in-process subscribers. Cross-node delivery is out of scope for
/// this slice — a network bridge layer may wrap this engine later.
#[derive(Debug)]
pub struct PubSubEngine {
    channels: DashMap<String, broadcast::Sender<PubSubMessage>>,
    capacity: usize,
}

impl Default for PubSubEngine {
    fn default() -> Self {
        Self {
            channels: DashMap::new(),
            capacity: DEFAULT_CHANNEL_CAPACITY,
        }
    }
}

impl PubSubEngine {
    /// Create a new pub/sub engine with per-channel buffer `capacity`,
    /// wrapped in an `Arc` for cheap sharing across async tasks.
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            channels: DashMap::new(),
            capacity: capacity.max(1),
        })
    }

    /// Create a default-capacity pub/sub engine wrapped in `Arc`.
    pub fn new_default() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Subscribe to `channel`, returning a receiver that yields every
    /// message published after this call.
    pub fn subscribe(&self, channel: &str) -> broadcast::Receiver<PubSubMessage> {
        if let Some(tx) = self.channels.get(channel) {
            return tx.subscribe();
        }
        let (tx, rx) = broadcast::channel(self.capacity);
        self.channels.insert(channel.to_string(), tx);
        rx
    }

    /// Publish `payload` on `channel`. Returns the number of active
    /// subscribers that received the message (0 if none).
    pub fn publish(&self, channel: &str, payload: bytes::Bytes) -> usize {
        let Some(entry) = self.channels.get(channel) else {
            return 0;
        };
        let msg = PubSubMessage {
            channel: channel.to_string(),
            payload,
        };
        entry.send(msg).unwrap_or(0)
    }

    /// Drop the broadcast sender for `channel`, which causes all current
    /// receivers to observe a `RecvError::Closed` on their next `recv`.
    ///
    /// Returns `true` if the channel was present.
    pub fn unsubscribe_all(&self, channel: &str) -> bool {
        self.channels.remove(channel).is_some()
    }

    /// Number of active channels (upper bound — a channel with zero
    /// live receivers is GC'd lazily on next publish).
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}
