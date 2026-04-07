//! Real-time sync engine for DarshJDB.
//!
//! Provides live query subscriptions, change broadcasting with delta diffs,
//! presence tracking, and session management over WebSocket connections.
//!
//! # Architecture
//!
//! ```text
//! Client ──WebSocket──▶ Session ──subscribe──▶ Registry
//!                          │                       │
//!                          ├── LIVE SELECT ──▶ LiveQueryManager
//!                          │                       │
//!                          ▼                       ▼
//!                      Presence            Broadcaster
//!                                              │
//!                                         DiffEngine
//!                                              │
//!                                         ChangeFeed ──▶ PgNotifyBridge
//! ```
//!
//! - **Session**: Per-connection state including active subscriptions and tx cursor.
//! - **Registry**: Global query-hash to session-set mapping for fan-out deduplication.
//! - **Broadcaster**: Listens for triple-store mutations, identifies affected queries,
//!   re-executes with permission context, and pushes diffs.
//! - **Diff**: Computes minimal delta patches between query result snapshots.
//! - **Presence**: Ephemeral per-room user state with auto-expiry and rate limiting.
//! - **LiveQueryManager**: SurrealDB-style LIVE SELECT with filter evaluation and push.
//! - **ChangeFeed**: Append-only mutation log with cursor-based replay and TTL retention.
//! - **PgNotifyBridge**: PostgreSQL LISTEN/NOTIFY for cluster-wide change propagation.

pub mod broadcaster;
pub mod change_feed;
pub mod diff;
pub mod live_query;
pub mod presence;
pub mod pubsub;
pub mod registry;
pub mod session;

pub use broadcaster::{Broadcaster, ChangeEvent};
pub use change_feed::{ChangeFeed, ChangeFeedConfig, ChangeFeedEntry, Cursor, PgNotifyBridge};
pub use diff::{EntityPatch, QueryDiff, compute_diff};
pub use live_query::{
    FilterPredicate, LiveAction, LiveEvent, LiveQueryId, LiveQueryManager, LiveSelectFields,
    ParsedLiveSelect, parse_live_select,
};
pub use presence::{PresenceManager, PresenceRoom};
pub use pubsub::{PubSubEngine, PubSubEvent};
pub use registry::SubscriptionRegistry;
pub use session::{ActiveSubscription, SessionManager, SyncSession};
