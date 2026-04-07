//! Comments, activity log, and notifications for DarshJDB.
//!
//! Provides record-level collaboration features:
//!
//! - [`comments`] — Threaded comments on any entity, stored as EAV triples.
//! - [`activity`] — Full audit/activity trail recording who changed what and when.
//! - [`notifications`] — In-app notification lifecycle (mentions, replies, assignments).
//! - [`handlers`] — Axum HTTP handlers wiring the above to REST endpoints.
//!
//! # Storage
//!
//! Comments are stored as EAV triples using the `comment:{uuid}` entity
//! namespace with attributes like `comment/entity`, `comment/user`,
//! `comment/content`, etc. Activity entries and notifications use
//! dedicated Postgres tables for efficient querying and indexing.

pub mod activity;
pub mod comments;
pub mod handlers;
pub mod notifications;

pub use activity::{
    Action, ActivityEntry, FieldChange, get_activity, get_table_activity, get_user_activity,
    record_activity,
};
pub use comments::{Comment, CommentId, ThreadedComment};
pub use notifications::{Notification, NotificationKind};
