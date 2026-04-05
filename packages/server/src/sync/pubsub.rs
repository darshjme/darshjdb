//! Pub/Sub engine for DarshanDB keyspace notifications.
//!
//! Provides Redis-style channel subscriptions with glob pattern matching.
//! Clients subscribe to channels like `entity:users:*` and receive events
//! when matching changes occur in the triple store.
//!
//! # Channel Patterns
//!
//! ```text
//! entity:*              — all entity changes
//! entity:users:*        — all user entity changes
//! entity:users:<uuid>   — specific entity changes
//! mutation:*            — all mutations
//! auth:*                — auth events (signup, signin, signout)
//! custom:<topic>        — user-defined channels
//! ```
//!
//! # Architecture
//!
//! The [`PubSubEngine`] maintains a set of active subscriptions keyed by
//! a subscriber-chosen ID. When a [`ChangeEvent`] arrives from the broadcaster,
//! the engine matches it against all active subscriptions and produces
//! [`PubSubEvent`] payloads for each match.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::broadcaster::ChangeEvent;

// ---------------------------------------------------------------------------
// Channel pattern matching
// ---------------------------------------------------------------------------

/// A parsed channel pattern used for subscription matching.
#[derive(Debug, Clone)]
pub struct ChannelPattern {
    /// The raw pattern string (e.g., `entity:users:*`).
    pub raw: String,
    /// Parsed segments split on `:`.
    segments: Vec<PatternSegment>,
}

#[derive(Debug, Clone)]
enum PatternSegment {
    /// Exact literal match.
    Literal(String),
    /// Wildcard `*` — matches any single segment or trailing rest.
    Wildcard,
}

impl ChannelPattern {
    /// Parse a channel pattern string into segments.
    ///
    /// # Examples
    ///
    /// ```
    /// # use darshandb_server::sync::pubsub::ChannelPattern;
    /// let p = ChannelPattern::parse("entity:users:*");
    /// assert!(p.matches("entity:users:abc-123"));
    /// assert!(!p.matches("entity:posts:abc-123"));
    /// ```
    pub fn parse(pattern: &str) -> Self {
        let segments = pattern
            .split(':')
            .map(|s| {
                if s == "*" {
                    PatternSegment::Wildcard
                } else {
                    PatternSegment::Literal(s.to_string())
                }
            })
            .collect();

        Self {
            raw: pattern.to_string(),
            segments,
        }
    }

    /// Test whether a concrete channel name matches this pattern.
    ///
    /// Matching rules:
    /// - Literal segments must match exactly.
    /// - A `*` segment matches any single segment.
    /// - A trailing `*` matches one or more remaining segments.
    pub fn matches(&self, channel: &str) -> bool {
        let parts: Vec<&str> = channel.split(':').collect();
        let pat = &self.segments;

        // Empty pattern matches nothing.
        if pat.is_empty() {
            return false;
        }

        let mut pi = 0; // pattern index
        let mut ci = 0; // channel index

        while pi < pat.len() && ci < parts.len() {
            match &pat[pi] {
                PatternSegment::Literal(lit) => {
                    if lit != parts[ci] {
                        return false;
                    }
                    pi += 1;
                    ci += 1;
                }
                PatternSegment::Wildcard => {
                    // If this is the last pattern segment, it matches all remaining.
                    if pi == pat.len() - 1 {
                        return true;
                    }
                    // Otherwise, match exactly one segment.
                    pi += 1;
                    ci += 1;
                }
            }
        }

        // Both must be fully consumed (unless trailing wildcard already returned).
        pi == pat.len() && ci == parts.len()
    }
}

// ---------------------------------------------------------------------------
// Pub/Sub event
// ---------------------------------------------------------------------------

/// An event emitted to pub/sub subscribers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubSubEvent {
    /// The concrete channel this event was published on.
    pub channel: String,
    /// The event kind (e.g., `created`, `updated`, `deleted`).
    pub event: String,
    /// Entity type, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    /// Entity ID, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// Attribute names that changed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// Transaction ID of the mutation that triggered this event.
    pub tx_id: i64,
    /// Custom payload for user-published events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

/// A single pub/sub subscription.
#[derive(Debug, Clone)]
pub struct Subscription {
    /// Subscriber-chosen ID for this subscription.
    pub id: String,
    /// The channel pattern being subscribed to.
    pub pattern: ChannelPattern,
    /// Opaque subscriber key (e.g., session ID or SSE connection ID).
    pub subscriber: String,
}

// ---------------------------------------------------------------------------
// Pub/Sub engine
// ---------------------------------------------------------------------------

/// Thread-safe pub/sub engine that manages subscriptions and event matching.
///
/// Each subscription is identified by a composite key of `(subscriber, id)`.
/// The engine is designed for concurrent read-heavy access with occasional
/// writes when clients subscribe or unsubscribe.
pub struct PubSubEngine {
    /// Active subscriptions keyed by `(subscriber_key, subscription_id)`.
    subscriptions: RwLock<HashMap<(String, String), Subscription>>,
    /// Broadcast sender for pub/sub events (used by SSE and custom publish).
    event_tx: tokio::sync::broadcast::Sender<PubSubEvent>,
}

impl PubSubEngine {
    /// Create a new pub/sub engine with the given broadcast channel capacity.
    pub fn new(capacity: usize) -> (Arc<Self>, tokio::sync::broadcast::Receiver<PubSubEvent>) {
        let (event_tx, event_rx) = tokio::sync::broadcast::channel(capacity);
        let engine = Arc::new(Self {
            subscriptions: RwLock::new(HashMap::new()),
            event_tx,
        });
        (engine, event_rx)
    }

    /// Subscribe to a channel pattern.
    ///
    /// Returns the parsed pattern for confirmation.
    pub fn subscribe(&self, subscriber: &str, id: &str, channel: &str) -> ChannelPattern {
        let pattern = ChannelPattern::parse(channel);
        let sub = Subscription {
            id: id.to_string(),
            pattern: pattern.clone(),
            subscriber: subscriber.to_string(),
        };
        let key = (subscriber.to_string(), id.to_string());

        let mut subs = self.subscriptions.write().expect("pubsub lock poisoned");
        subs.insert(key, sub);

        debug!(
            subscriber = subscriber,
            id = id,
            channel = channel,
            "pub/sub subscription added"
        );

        pattern
    }

    /// Unsubscribe from a specific subscription.
    ///
    /// Returns `true` if the subscription existed and was removed.
    pub fn unsubscribe(&self, subscriber: &str, id: &str) -> bool {
        let key = (subscriber.to_string(), id.to_string());
        let mut subs = self.subscriptions.write().expect("pubsub lock poisoned");
        let removed = subs.remove(&key).is_some();

        if removed {
            debug!(
                subscriber = subscriber,
                id = id,
                "pub/sub subscription removed"
            );
        }

        removed
    }

    /// Remove all subscriptions for a given subscriber (e.g., on disconnect).
    pub fn unsubscribe_all(&self, subscriber: &str) -> usize {
        let mut subs = self.subscriptions.write().expect("pubsub lock poisoned");
        let before = subs.len();
        subs.retain(|(sub, _), _| sub != subscriber);
        let removed = before - subs.len();

        if removed > 0 {
            debug!(
                subscriber = subscriber,
                removed = removed,
                "pub/sub subscriptions cleaned up"
            );
        }

        removed
    }

    /// Get a new broadcast receiver for pub/sub events.
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<PubSubEvent> {
        self.event_tx.subscribe()
    }

    /// Publish a custom event directly to a channel.
    ///
    /// This is used by the `POST /api/events/publish` endpoint for
    /// user-initiated events (webhooks, notifications, etc.).
    pub fn publish(&self, event: PubSubEvent) -> usize {
        self.event_tx.send(event).unwrap_or_default()
    }

    /// Process a [`ChangeEvent`] from the triple-store broadcaster.
    ///
    /// Converts the change event into concrete channel names, matches them
    /// against all active subscriptions, and broadcasts matching events.
    ///
    /// Returns the list of `(subscriber, subscription_id)` pairs that matched.
    pub fn process_change_event(&self, event: &ChangeEvent) -> Vec<(String, String, PubSubEvent)> {
        let mut matches = Vec::new();

        // Build the concrete channels this change event maps to.
        let channels = Self::change_event_to_channels(event);

        let subs = self.subscriptions.read().expect("pubsub lock poisoned");

        for ((subscriber, sub_id), sub) in subs.iter() {
            for (channel, pub_event) in &channels {
                if sub.pattern.matches(channel) {
                    matches.push((subscriber.clone(), sub_id.clone(), pub_event.clone()));
                    // One match per subscription is enough (don't double-fire).
                    break;
                }
            }
        }

        // Also broadcast through the event channel for SSE consumers.
        for (channel, pub_event) in &channels {
            // Only broadcast entity and mutation events (not per-subscriber).
            if channel.starts_with("entity:") || channel.starts_with("mutation:") {
                let _ = self.event_tx.send(pub_event.clone());
            }
        }

        matches
    }

    /// Convert a [`ChangeEvent`] into a set of concrete channel names + events.
    fn change_event_to_channels(event: &ChangeEvent) -> Vec<(String, PubSubEvent)> {
        let mut channels = Vec::new();

        let event_kind = if event.tx_id == 0 {
            "deleted"
        } else if event.entity_ids.len() == 1 {
            // Heuristic: if only one entity was touched, it's likely a create or update.
            // A more precise determination would require inspecting the mutation ops.
            "updated"
        } else {
            "updated"
        };

        // For each affected entity, emit entity-level channels.
        if let Some(ref entity_type) = event.entity_type {
            for entity_id in &event.entity_ids {
                let pub_event = PubSubEvent {
                    channel: format!("entity:{entity_type}:{entity_id}"),
                    event: event_kind.to_string(),
                    entity_type: Some(entity_type.clone()),
                    entity_id: Some(entity_id.clone()),
                    changed: event.attributes.clone(),
                    tx_id: event.tx_id,
                    payload: None,
                };

                // Specific entity channel: entity:users:<uuid>
                channels.push((
                    format!("entity:{entity_type}:{entity_id}"),
                    pub_event.clone(),
                ));

                // Type-level channel: entity:users:*  (matched by pattern)
                // Already covered by the specific channel since patterns match.
            }

            // Type-level event for the whole collection.
            let type_event = PubSubEvent {
                channel: format!("entity:{entity_type}"),
                event: event_kind.to_string(),
                entity_type: Some(entity_type.clone()),
                entity_id: None,
                changed: event.attributes.clone(),
                tx_id: event.tx_id,
                payload: None,
            };
            channels.push((format!("entity:{entity_type}"), type_event));
        }

        // Mutation-level channel: mutation:<event_kind> (e.g., mutation:updated).
        let mut_channel = format!("mutation:{event_kind}");
        let mut_event = PubSubEvent {
            channel: mut_channel.clone(),
            event: event_kind.to_string(),
            entity_type: event.entity_type.clone(),
            entity_id: None,
            changed: event.attributes.clone(),
            tx_id: event.tx_id,
            payload: None,
        };
        channels.push((mut_channel, mut_event));

        channels
    }

    /// Get the current subscription count.
    pub fn subscription_count(&self) -> usize {
        self.subscriptions
            .read()
            .expect("pubsub lock poisoned")
            .len()
    }

    /// List all subscriptions for a given subscriber.
    pub fn list_subscriptions(&self, subscriber: &str) -> Vec<(String, String)> {
        self.subscriptions
            .read()
            .expect("pubsub lock poisoned")
            .iter()
            .filter(|((sub, _), _)| sub == subscriber)
            .map(|((_, id), sub)| (id.clone(), sub.pattern.raw.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ChannelPattern matching
    // -----------------------------------------------------------------------

    #[test]
    fn exact_channel_match() {
        let p = ChannelPattern::parse("entity:users:abc-123");
        assert!(p.matches("entity:users:abc-123"));
        assert!(!p.matches("entity:users:def-456"));
        assert!(!p.matches("entity:posts:abc-123"));
    }

    #[test]
    fn trailing_wildcard_matches_one() {
        let p = ChannelPattern::parse("entity:users:*");
        assert!(p.matches("entity:users:abc-123"));
        assert!(p.matches("entity:users:def-456"));
        assert!(!p.matches("entity:posts:abc-123"));
        assert!(!p.matches("entity:users")); // too short
    }

    #[test]
    fn trailing_wildcard_matches_multiple() {
        let p = ChannelPattern::parse("entity:*");
        assert!(p.matches("entity:users"));
        assert!(p.matches("entity:users:abc-123"));
        assert!(p.matches("entity:posts:def:nested"));
        assert!(!p.matches("mutation:insert"));
    }

    #[test]
    fn middle_wildcard_matches_one_segment() {
        let p = ChannelPattern::parse("entity:*:abc-123");
        assert!(p.matches("entity:users:abc-123"));
        assert!(p.matches("entity:posts:abc-123"));
        assert!(!p.matches("entity:users:def-456"));
    }

    #[test]
    fn all_wildcard() {
        let p = ChannelPattern::parse("*");
        assert!(p.matches("anything"));
        assert!(p.matches("entity:users:abc"));
        assert!(p.matches("mutation:insert"));
    }

    #[test]
    fn mutation_channel() {
        let p = ChannelPattern::parse("mutation:*");
        assert!(p.matches("mutation:insert"));
        assert!(p.matches("mutation:delete"));
        assert!(!p.matches("entity:users:abc"));
    }

    #[test]
    fn auth_channel() {
        let p = ChannelPattern::parse("auth:*");
        assert!(p.matches("auth:signup"));
        assert!(p.matches("auth:signin"));
        assert!(p.matches("auth:signout"));
        assert!(!p.matches("entity:users:abc"));
    }

    #[test]
    fn empty_pattern_matches_nothing() {
        let p = ChannelPattern::parse("");
        assert!(!p.matches("anything"));
    }

    // -----------------------------------------------------------------------
    // PubSubEngine subscribe / unsubscribe
    // -----------------------------------------------------------------------

    #[test]
    fn subscribe_and_unsubscribe() {
        let (engine, _rx) = PubSubEngine::new(64);
        engine.subscribe("session-1", "ps-1", "entity:users:*");
        assert_eq!(engine.subscription_count(), 1);

        assert!(engine.unsubscribe("session-1", "ps-1"));
        assert_eq!(engine.subscription_count(), 0);
    }

    #[test]
    fn unsubscribe_nonexistent_returns_false() {
        let (engine, _rx) = PubSubEngine::new(64);
        assert!(!engine.unsubscribe("session-1", "ps-1"));
    }

    #[test]
    fn unsubscribe_all_cleans_up() {
        let (engine, _rx) = PubSubEngine::new(64);
        engine.subscribe("session-1", "ps-1", "entity:users:*");
        engine.subscribe("session-1", "ps-2", "entity:posts:*");
        engine.subscribe("session-2", "ps-1", "mutation:*");

        let removed = engine.unsubscribe_all("session-1");
        assert_eq!(removed, 2);
        assert_eq!(engine.subscription_count(), 1);
    }

    #[test]
    fn list_subscriptions_for_subscriber() {
        let (engine, _rx) = PubSubEngine::new(64);
        engine.subscribe("session-1", "ps-1", "entity:users:*");
        engine.subscribe("session-1", "ps-2", "auth:*");
        engine.subscribe("session-2", "ps-1", "mutation:*");

        let subs = engine.list_subscriptions("session-1");
        assert_eq!(subs.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Change event processing
    // -----------------------------------------------------------------------

    #[test]
    fn process_change_event_matches_entity_subscription() {
        let (engine, _rx) = PubSubEngine::new(64);
        engine.subscribe("session-1", "ps-1", "entity:users:*");
        engine.subscribe("session-2", "ps-2", "entity:posts:*");

        let event = ChangeEvent {
            tx_id: 42,
            entity_ids: vec!["uuid-1".to_string()],
            attributes: vec!["name".to_string(), "email".to_string()],
            entity_type: Some("users".to_string()),
            actor_id: Some("actor-1".to_string()),
        };

        let matches = engine.process_change_event(&event);

        // session-1 should match (entity:users:*), session-2 should not.
        let session_1_matches: Vec<_> = matches
            .iter()
            .filter(|(sub, _, _)| sub == "session-1")
            .collect();
        let session_2_matches: Vec<_> = matches
            .iter()
            .filter(|(sub, _, _)| sub == "session-2")
            .collect();

        assert!(
            !session_1_matches.is_empty(),
            "session-1 should match entity:users:*"
        );
        assert!(
            session_2_matches.is_empty(),
            "session-2 should not match entity:posts:*"
        );
    }

    #[test]
    fn process_change_event_matches_mutation_subscription() {
        let (engine, _rx) = PubSubEngine::new(64);
        engine.subscribe("session-1", "ps-1", "mutation:*");

        let event = ChangeEvent {
            tx_id: 10,
            entity_ids: vec!["uuid-1".to_string()],
            attributes: vec!["status".to_string()],
            entity_type: Some("orders".to_string()),
            actor_id: None,
        };

        let matches = engine.process_change_event(&event);
        assert!(
            !matches.is_empty(),
            "mutation:* should match any change event"
        );
    }

    #[test]
    fn process_change_event_specific_entity() {
        let (engine, _rx) = PubSubEngine::new(64);
        engine.subscribe("session-1", "ps-1", "entity:users:uuid-1");
        engine.subscribe("session-1", "ps-2", "entity:users:uuid-2");

        let event = ChangeEvent {
            tx_id: 5,
            entity_ids: vec!["uuid-1".to_string()],
            attributes: vec!["name".to_string()],
            entity_type: Some("users".to_string()),
            actor_id: None,
        };

        let matches = engine.process_change_event(&event);
        let ps1: Vec<_> = matches.iter().filter(|(_, id, _)| id == "ps-1").collect();
        let ps2: Vec<_> = matches.iter().filter(|(_, id, _)| id == "ps-2").collect();

        assert!(!ps1.is_empty(), "ps-1 should match entity:users:uuid-1");
        assert!(ps2.is_empty(), "ps-2 should not match entity:users:uuid-2");
    }

    // -----------------------------------------------------------------------
    // Custom publish
    // -----------------------------------------------------------------------

    #[test]
    fn publish_custom_event() {
        let (engine, mut rx) = PubSubEngine::new(64);

        let event = PubSubEvent {
            channel: "custom:notifications".to_string(),
            event: "new-message".to_string(),
            entity_type: None,
            entity_id: None,
            changed: vec![],
            tx_id: 0,
            payload: Some(serde_json::json!({ "message": "hello" })),
        };

        let receivers = engine.publish(event.clone());
        assert_eq!(receivers, 1); // The rx we hold.

        let received = rx.try_recv().unwrap();
        assert_eq!(received.channel, "custom:notifications");
        assert_eq!(received.event, "new-message");
    }

    // -----------------------------------------------------------------------
    // PubSubEvent serialization
    // -----------------------------------------------------------------------

    #[test]
    fn pubsub_event_json_serialization() {
        let event = PubSubEvent {
            channel: "entity:users:uuid-1".to_string(),
            event: "updated".to_string(),
            entity_type: Some("users".to_string()),
            entity_id: Some("uuid-1".to_string()),
            changed: vec!["name".to_string(), "email".to_string()],
            tx_id: 42,
            payload: None,
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["channel"], "entity:users:uuid-1");
        assert_eq!(json["event"], "updated");
        assert_eq!(json["entity_type"], "users");
        assert_eq!(json["entity_id"], "uuid-1");
        assert_eq!(json["tx_id"], 42);
        // payload should be absent (skip_serializing_if = None).
        assert!(json.get("payload").is_none());
    }

    #[test]
    fn pubsub_event_with_payload_serialization() {
        let event = PubSubEvent {
            channel: "custom:webhook".to_string(),
            event: "triggered".to_string(),
            entity_type: None,
            entity_id: None,
            changed: vec![],
            tx_id: 0,
            payload: Some(serde_json::json!({ "url": "https://example.com" })),
        };

        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("payload").is_some());
        assert!(json.get("entity_type").is_none()); // skipped
        assert!(json.get("entity_id").is_none()); // skipped
    }

    #[test]
    fn change_event_to_channels_builds_correct_channels() {
        let event = ChangeEvent {
            tx_id: 100,
            entity_ids: vec!["id-1".to_string(), "id-2".to_string()],
            attributes: vec!["name".to_string()],
            entity_type: Some("users".to_string()),
            actor_id: None,
        };

        let channels = PubSubEngine::change_event_to_channels(&event);
        let channel_names: Vec<&str> = channels.iter().map(|(c, _)| c.as_str()).collect();

        assert!(channel_names.contains(&"entity:users:id-1"));
        assert!(channel_names.contains(&"entity:users:id-2"));
        assert!(channel_names.contains(&"entity:users"));
        assert!(channel_names.contains(&"mutation:updated"));
    }

    #[test]
    fn change_event_without_entity_type_emits_mutation_only() {
        let event = ChangeEvent {
            tx_id: 1,
            entity_ids: vec!["id-1".to_string()],
            attributes: vec![],
            entity_type: None,
            actor_id: None,
        };

        let channels = PubSubEngine::change_event_to_channels(&event);
        assert_eq!(channels.len(), 1);
        assert!(channels[0].0.starts_with("mutation:"));
    }
}
