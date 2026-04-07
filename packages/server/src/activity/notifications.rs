//! In-app notification system for DarshJDB.
//!
//! Notifications are generated automatically when:
//!
//! - A user is `@mentioned` in a comment
//! - Someone replies to a user's comment
//! - A record is assigned to a user
//! - A record is shared with a user
//! - A system alert needs to be surfaced
//!
//! Each notification has a read/unread state and links back to the
//! resource that triggered it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};

// ── Types ──────────────────────────────────────────────────────────

/// The kind of event that triggered the notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// The user was mentioned in a comment.
    Mention,
    /// Someone replied to the user's comment.
    Reply,
    /// A record was assigned to the user.
    Assignment,
    /// A record was shared with the user.
    Share,
    /// A system-level alert (e.g. schema migration, quota warning).
    SystemAlert,
}

impl NotificationKind {
    /// Convert from a string representation.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "mention" => Some(Self::Mention),
            "reply" => Some(Self::Reply),
            "assignment" => Some(Self::Assignment),
            "share" => Some(Self::Share),
            "system_alert" => Some(Self::SystemAlert),
            _ => None,
        }
    }

    /// Canonical string form for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mention => "mention",
            Self::Reply => "reply",
            Self::Assignment => "assignment",
            Self::Share => "share",
            Self::SystemAlert => "system_alert",
        }
    }
}

/// A single in-app notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    /// Unique notification identifier.
    pub id: Uuid,
    /// The user who should see this notification.
    pub user_id: Uuid,
    /// What triggered the notification.
    pub kind: NotificationKind,
    /// Short title (e.g. "New mention in Order #1234").
    pub title: String,
    /// Longer body text with context.
    pub body: String,
    /// The type of resource linked (e.g. "comment", "order", "user").
    pub resource_type: String,
    /// The specific resource's UUID.
    pub resource_id: Uuid,
    /// Whether the notification has been read.
    pub read: bool,
    /// When the notification was created.
    pub created_at: DateTime<Utc>,
}

/// Input for creating a notification (id, read, created_at auto-assigned).
#[derive(Debug, Clone)]
pub struct CreateNotificationInput {
    /// The user who should see this notification.
    pub user_id: Uuid,
    /// What triggered the notification.
    pub kind: NotificationKind,
    /// Short title.
    pub title: String,
    /// Longer body text.
    pub body: String,
    /// The type of resource linked.
    pub resource_type: String,
    /// The specific resource's UUID.
    pub resource_id: Uuid,
}

// ── Schema ─────────────────────────────────────────────────────────

/// Create the `notifications` table if it does not exist.
pub async fn ensure_notifications_schema(pool: &PgPool) -> std::result::Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS notifications (
            id             UUID PRIMARY KEY,
            user_id        UUID NOT NULL,
            kind           TEXT NOT NULL,
            title          TEXT NOT NULL,
            body           TEXT NOT NULL DEFAULT '',
            resource_type  TEXT NOT NULL DEFAULT '',
            resource_id    UUID NOT NULL,
            read           BOOLEAN NOT NULL DEFAULT false,
            created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        CREATE INDEX IF NOT EXISTS idx_notifications_user_unread
            ON notifications (user_id, created_at DESC) WHERE NOT read;
        CREATE INDEX IF NOT EXISTS idx_notifications_user_all
            ON notifications (user_id, created_at DESC);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ── Core operations ────────────────────────────────────────────────

/// Create a new notification.
pub async fn create_notification(
    pool: &PgPool,
    input: CreateNotificationInput,
) -> Result<Notification> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query(
        r#"
        INSERT INTO notifications (id, user_id, kind, title, body, resource_type, resource_id, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(id)
    .bind(input.user_id)
    .bind(input.kind.as_str())
    .bind(&input.title)
    .bind(&input.body)
    .bind(&input.resource_type)
    .bind(input.resource_id)
    .bind(now)
    .execute(pool)
    .await
    .map_err(DarshJError::Database)?;

    Ok(Notification {
        id,
        user_id: input.user_id,
        kind: input.kind,
        title: input.title,
        body: input.body,
        resource_type: input.resource_type,
        resource_id: input.resource_id,
        read: false,
        created_at: now,
    })
}

/// Get notifications for a user, optionally filtering to unread only.
///
/// Returns in reverse chronological order (newest first), limited to
/// 100 entries by default. For pagination, use `created_at` cursors.
pub async fn get_notifications(
    pool: &PgPool,
    user_id: Uuid,
    unread_only: bool,
) -> Result<Vec<Notification>> {
    let query = if unread_only {
        r#"
        SELECT id, user_id, kind, title, body, resource_type, resource_id, read, created_at
        FROM notifications
        WHERE user_id = $1 AND NOT read
        ORDER BY created_at DESC
        LIMIT 100
        "#
    } else {
        r#"
        SELECT id, user_id, kind, title, body, resource_type, resource_id, read, created_at
        FROM notifications
        WHERE user_id = $1
        ORDER BY created_at DESC
        LIMIT 100
        "#
    };

    let rows: Vec<NotificationRow> = sqlx::query_as(query)
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(DarshJError::Database)?;

    Ok(rows.into_iter().map(|r| r.into_notification()).collect())
}

/// Mark a single notification as read.
pub async fn mark_read(pool: &PgPool, notification_id: Uuid) -> Result<()> {
    let result = sqlx::query("UPDATE notifications SET read = true WHERE id = $1 AND NOT read")
        .bind(notification_id)
        .execute(pool)
        .await
        .map_err(DarshJError::Database)?;

    if result.rows_affected() == 0 {
        return Err(DarshJError::EntityNotFound(notification_id));
    }

    Ok(())
}

/// Mark all notifications as read for a user.
pub async fn mark_all_read(pool: &PgPool, user_id: Uuid) -> Result<u64> {
    let result = sqlx::query("UPDATE notifications SET read = true WHERE user_id = $1 AND NOT read")
        .bind(user_id)
        .execute(pool)
        .await
        .map_err(DarshJError::Database)?;

    Ok(result.rows_affected())
}

/// Get the count of unread notifications for a user.
pub async fn unread_count(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM notifications WHERE user_id = $1 AND NOT read")
            .bind(user_id)
            .fetch_one(pool)
            .await
            .map_err(DarshJError::Database)?;

    Ok(row.0)
}

// ── Auto-generation helpers ────────────────────────────────────────

/// Generate mention notifications for all users mentioned in a comment.
///
/// Call this after creating a comment. Skips the comment author
/// (no self-notification).
pub async fn notify_mentions(
    pool: &PgPool,
    comment_author_id: Uuid,
    comment_id: Uuid,
    entity_id: Uuid,
    mentioned_user_ids: &[Uuid],
    comment_preview: &str,
) -> Result<Vec<Notification>> {
    let mut notifications = Vec::new();
    let preview = truncate(comment_preview, 100);

    for &user_id in mentioned_user_ids {
        if user_id == comment_author_id {
            continue; // Don't notify yourself.
        }

        let notif = create_notification(
            pool,
            CreateNotificationInput {
                user_id,
                kind: NotificationKind::Mention,
                title: "You were mentioned in a comment".into(),
                body: preview.clone(),
                resource_type: "comment".into(),
                resource_id: comment_id,
            },
        )
        .await?;
        notifications.push(notif);
    }

    // Also record activity for the mention.
    super::record_activity(
        pool,
        super::activity::RecordActivityInput {
            entity_type: "comment".into(),
            entity_id,
            action: super::Action::Commented,
            user_id: comment_author_id,
            changes: vec![],
            metadata: Some(serde_json::json!({
                "comment_id": comment_id,
                "mentions": mentioned_user_ids,
            })),
        },
    )
    .await?;

    Ok(notifications)
}

/// Generate a reply notification for the parent comment's author.
///
/// Call this after creating a reply comment. Skips if the replier is
/// the same as the parent author.
pub async fn notify_reply(
    pool: &PgPool,
    replier_id: Uuid,
    reply_comment_id: Uuid,
    parent_author_id: Uuid,
    comment_preview: &str,
) -> Result<Option<Notification>> {
    if replier_id == parent_author_id {
        return Ok(None); // Don't notify yourself.
    }

    let preview = truncate(comment_preview, 100);

    let notif = create_notification(
        pool,
        CreateNotificationInput {
            user_id: parent_author_id,
            kind: NotificationKind::Reply,
            title: "Someone replied to your comment".into(),
            body: preview,
            resource_type: "comment".into(),
            resource_id: reply_comment_id,
        },
    )
    .await?;

    Ok(Some(notif))
}

/// Generate a share notification.
pub async fn notify_share(
    pool: &PgPool,
    sharer_id: Uuid,
    target_user_id: Uuid,
    resource_type: &str,
    resource_id: Uuid,
) -> Result<Option<Notification>> {
    if sharer_id == target_user_id {
        return Ok(None);
    }

    let notif = create_notification(
        pool,
        CreateNotificationInput {
            user_id: target_user_id,
            kind: NotificationKind::Share,
            title: format!("A {resource_type} was shared with you"),
            body: String::new(),
            resource_type: resource_type.into(),
            resource_id,
        },
    )
    .await?;

    Ok(Some(notif))
}

// ── Internal types ─────────────────────────────────────────────────

/// Raw database row for the notifications table.
#[derive(Debug, Clone, sqlx::FromRow)]
struct NotificationRow {
    id: Uuid,
    user_id: Uuid,
    kind: String,
    title: String,
    body: String,
    resource_type: String,
    resource_id: Uuid,
    read: bool,
    created_at: DateTime<Utc>,
}

impl NotificationRow {
    fn into_notification(self) -> Notification {
        let kind = NotificationKind::from_str_opt(&self.kind)
            .unwrap_or(NotificationKind::SystemAlert);
        Notification {
            id: self.id,
            user_id: self.user_id,
            kind,
            title: self.title,
            body: self.body,
            resource_type: self.resource_type,
            resource_id: self.resource_id,
            read: self.read,
            created_at: self.created_at,
        }
    }
}

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut truncated = s[..max_len].to_string();
        truncated.push_str("...");
        truncated
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_notification_kind_roundtrip() {
        let kinds = [
            NotificationKind::Mention,
            NotificationKind::Reply,
            NotificationKind::Assignment,
            NotificationKind::Share,
            NotificationKind::SystemAlert,
        ];

        for kind in kinds {
            let s = kind.as_str();
            let parsed = NotificationKind::from_str_opt(s).expect("should parse back");
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn test_notification_kind_serialization() {
        let json = serde_json::to_value(NotificationKind::Mention).unwrap();
        assert_eq!(json, "mention");

        let parsed: NotificationKind = serde_json::from_value(json!("reply")).unwrap();
        assert_eq!(parsed, NotificationKind::Reply);
    }

    #[test]
    fn test_notification_serialization() {
        let notif = Notification {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            kind: NotificationKind::Mention,
            title: "You were mentioned".into(),
            body: "Check out this comment".into(),
            resource_type: "comment".into(),
            resource_id: Uuid::new_v4(),
            read: false,
            created_at: Utc::now(),
        };

        let json = serde_json::to_value(&notif).unwrap();
        assert_eq!(json["kind"], "mention");
        assert_eq!(json["read"], false);
        assert_eq!(json["title"], "You were mentioned");
    }

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_long() {
        let long = "a".repeat(200);
        let result = truncate(&long, 100);
        assert_eq!(result.len(), 103); // 100 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_exact() {
        let exact = "a".repeat(100);
        assert_eq!(truncate(&exact, 100), exact);
    }
}
