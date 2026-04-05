//! Real-time sync engine for DarshanDB.
//!
//! Provides live query subscriptions, change broadcasting with delta diffs,
//! presence tracking, and session management over WebSocket connections.
//!
//! # Architecture
//!
//! ```text
//! Client ──WebSocket──▶ Session ──subscribe──▶ Registry
//!                          │                       │
//!                          ▼                       ▼
//!                      Presence            Broadcaster
//!                                              │
//!                                         DiffEngine
//! ```
//!
//! - **Session**: Per-connection state including active subscriptions and tx cursor.
//! - **Registry**: Global query-hash to session-set mapping for fan-out deduplication.
//! - **Broadcaster**: Listens for triple-store mutations, identifies affected queries,
//!   re-executes with permission context, and pushes diffs.
//! - **Diff**: Computes minimal delta patches between query result snapshots.
//! - **Presence**: Ephemeral per-room user state with auto-expiry and rate limiting.

pub mod broadcaster;
pub mod diff;
pub mod presence;
pub mod pubsub;
pub mod registry;
pub mod session;

pub use broadcaster::{Broadcaster, ChangeEvent};
pub use diff::{EntityPatch, QueryDiff, compute_diff};
pub use presence::{PresenceManager, PresenceRoom};
pub use pubsub::{PubSubEngine, PubSubEvent};
pub use registry::SubscriptionRegistry;
pub use session::{ActiveSubscription, SessionManager, SyncSession};
