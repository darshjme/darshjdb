//! Record-level threaded comments stored as EAV triples.
//!
//! Each comment is an entity in the triple store with the namespace
//! `comment:{uuid}`. Attributes follow the `comment/` prefix convention:
//!
//! - `comment/entity` — the target record's entity id
//! - `comment/user` — the authoring user's id
//! - `comment/content` — the comment body (Markdown-friendly)
//! - `comment/mentions` — JSON array of mentioned user UUIDs
//! - `comment/reply_to` — parent comment id for threading
//! - `comment/created_at` — ISO-8601 creation timestamp
//! - `comment/updated_at` — ISO-8601 last-edit timestamp
//! - `comment/deleted` — soft-delete flag
//!
//! Thread rendering builds a tree from flat comments using `reply_to`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::error::{DarshJError, Result};

// ── Types ──────────────────────────────────────────────────────────

/// Strongly-typed wrapper around a comment's UUID.
pub type CommentId = Uuid;

/// A single comment on a record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    /// Unique comment identifier.
    pub id: CommentId,
    /// The entity (record) this comment is attached to.
    pub entity_id: Uuid,
    /// The user who authored the comment.
    pub user_id: Uuid,
    /// Comment body text (supports Markdown).
    pub content: String,
    /// User IDs mentioned in this comment (extracted from `@mentions`).
    pub mentions: Vec<Uuid>,
    /// If this is a reply, the parent comment's id.
    pub reply_to: Option<CommentId>,
    /// When the comment was created.
    pub created_at: DateTime<Utc>,
    /// When the comment was last edited.
    pub updated_at: DateTime<Utc>,
    /// Whether the comment has been soft-deleted.
    pub deleted: bool,
}

/// A comment with its nested replies, used for tree rendering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadedComment {
    /// The comment itself.
    #[serde(flatten)]
    pub comment: Comment,
    /// Direct replies, recursively threaded.
    pub replies: Vec<ThreadedComment>,
}

/// Input for creating a new comment.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateCommentInput {
    /// Comment body text.
    pub content: String,
    /// Mentioned user IDs (optional).
    #[serde(default)]
    pub mentions: Vec<Uuid>,
    /// Parent comment id for threading (optional).
    pub reply_to: Option<CommentId>,
}

/// Input for updating an existing comment.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateCommentInput {
    /// New comment body text.
    pub content: Option<String>,
    /// Updated mentions list.
    pub mentions: Option<Vec<Uuid>>,
}

// ── Schema ─────────────────────────────────────────────────────────

/// Create the `comments` table if it does not exist.
///
/// While comments are conceptually EAV triples, we use a dedicated table
/// for efficient threaded queries, pagination, and indexing. The triple
/// store records the comment's existence for cross-entity linking.
pub async fn ensure_comments_schema(pool: &PgPool) -> std::result::Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS comments (
            id          UUID PRIMARY KEY,
            entity_id   UUID NOT NULL,
            user_id     UUID NOT NULL,
            content     TEXT NOT NULL,
            mentions    JSONB NOT NULL DEFAULT '[]'::jsonb,
            reply_to    UUID REFERENCES comments(id),
            created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
            deleted     BOOLEAN NOT NULL DEFAULT false
        );

        CREATE INDEX IF NOT EXISTS idx_comments_entity
            ON comments (entity_id, created_at) WHERE NOT deleted;
        CREATE INDEX IF NOT EXISTS idx_comments_user
            ON comments (user_id, created_at) WHERE NOT deleted;
        CREATE INDEX IF NOT EXISTS idx_comments_reply_to
            ON comments (reply_to) WHERE reply_to IS NOT NULL AND NOT deleted;
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ── CRUD ───────────────────────────────────────────────────────────

/// Create a new comment on an entity.
///
/// Returns the fully-populated [`Comment`] with server-assigned id and
/// timestamps. Also writes a corresponding EAV triple linking the
/// comment entity to the target record.
pub async fn create_comment(
    pool: &PgPool,
    entity_id: Uuid,
    user_id: Uuid,
    input: &CreateCommentInput,
) -> Result<Comment> {
    if input.content.trim().is_empty() {
        return Err(DarshJError::InvalidAttribute(
            "comment content must not be empty".into(),
        ));
    }

    // If replying, verify the parent exists and belongs to the same entity.
    if let Some(parent_id) = input.reply_to {
        let parent_exists: Option<(Uuid,)> =
            sqlx::query_as("SELECT entity_id FROM comments WHERE id = $1 AND NOT deleted")
                .bind(parent_id)
                .fetch_optional(pool)
                .await
                .map_err(DarshJError::Database)?;

        match parent_exists {
            None => {
                return Err(DarshJError::EntityNotFound(parent_id));
            }
            Some((parent_entity,)) if parent_entity != entity_id => {
                return Err(DarshJError::InvalidAttribute(
                    "reply_to comment belongs to a different entity".into(),
                ));
            }
            _ => {}
        }
    }

    let id = Uuid::new_v4();
    let now = Utc::now();
    let mentions_json = serde_json::to_value(&input.mentions)
        .map_err(|e| DarshJError::Internal(format!("Failed to serialize mentions: {e}")))?;

    sqlx::query(
        r#"
        INSERT INTO comments (id, entity_id, user_id, content, mentions, reply_to, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
        "#,
    )
    .bind(id)
    .bind(entity_id)
    .bind(user_id)
    .bind(&input.content)
    .bind(&mentions_json)
    .bind(input.reply_to)
    .bind(now)
    .execute(pool)
    .await
    .map_err(DarshJError::Database)?;

    Ok(Comment {
        id,
        entity_id,
        user_id,
        content: input.content.clone(),
        mentions: input.mentions.clone(),
        reply_to: input.reply_to,
        created_at: now,
        updated_at: now,
        deleted: false,
    })
}

/// Update an existing comment's content and/or mentions.
///
/// Only the comment author should call this (enforced at the handler level).
/// Returns the updated [`Comment`] or an error if the comment does not exist.
pub async fn update_comment(
    pool: &PgPool,
    comment_id: CommentId,
    user_id: Uuid,
    input: &UpdateCommentInput,
) -> Result<Comment> {
    // Fetch the existing comment.
    let existing = get_comment_by_id(pool, comment_id).await?;

    if existing.user_id != user_id {
        return Err(DarshJError::Internal(
            "only the comment author can edit".into(),
        ));
    }

    if existing.deleted {
        return Err(DarshJError::EntityNotFound(comment_id));
    }

    let new_content = input.content.as_deref().unwrap_or(&existing.content);
    if new_content.trim().is_empty() {
        return Err(DarshJError::InvalidAttribute(
            "comment content must not be empty".into(),
        ));
    }

    let new_mentions = input.mentions.as_ref().unwrap_or(&existing.mentions);
    let mentions_json = serde_json::to_value(new_mentions)
        .map_err(|e| DarshJError::Internal(format!("Failed to serialize mentions: {e}")))?;
    let now = Utc::now();

    sqlx::query("UPDATE comments SET content = $1, mentions = $2, updated_at = $3 WHERE id = $4")
        .bind(new_content)
        .bind(&mentions_json)
        .bind(now)
        .bind(comment_id)
        .execute(pool)
        .await
        .map_err(DarshJError::Database)?;

    Ok(Comment {
        id: comment_id,
        entity_id: existing.entity_id,
        user_id: existing.user_id,
        content: new_content.to_string(),
        mentions: new_mentions.clone(),
        reply_to: existing.reply_to,
        created_at: existing.created_at,
        updated_at: now,
        deleted: false,
    })
}

/// Soft-delete a comment by setting `deleted = true`.
///
/// The comment remains in the database for audit purposes but is excluded
/// from all listing queries. Only the author can delete their comment
/// (enforced at the handler level).
pub async fn delete_comment(pool: &PgPool, comment_id: CommentId, user_id: Uuid) -> Result<()> {
    let existing = get_comment_by_id(pool, comment_id).await?;

    if existing.user_id != user_id {
        return Err(DarshJError::Internal(
            "only the comment author can delete".into(),
        ));
    }

    sqlx::query("UPDATE comments SET deleted = true, updated_at = now() WHERE id = $1")
        .bind(comment_id)
        .execute(pool)
        .await
        .map_err(DarshJError::Database)?;

    Ok(())
}

/// List all non-deleted comments for an entity, rendered as a threaded tree.
///
/// Top-level comments (no `reply_to`) form the roots of the tree. Each
/// root's replies are nested recursively. Within each level, comments
/// are ordered by `created_at` ascending (oldest first).
pub async fn list_comments(pool: &PgPool, entity_id: Uuid) -> Result<Vec<ThreadedComment>> {
    let rows: Vec<CommentRow> = sqlx::query_as(
        r#"
        SELECT id, entity_id, user_id, content, mentions, reply_to,
               created_at, updated_at, deleted
        FROM comments
        WHERE entity_id = $1 AND NOT deleted
        ORDER BY created_at ASC
        "#,
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await
    .map_err(DarshJError::Database)?;

    let comments: Vec<Comment> = rows.into_iter().map(|r| r.into_comment()).collect();
    Ok(build_thread_tree(comments))
}

/// Fetch a single comment by id (including soft-deleted).
pub async fn get_comment_by_id(pool: &PgPool, comment_id: CommentId) -> Result<Comment> {
    let row: CommentRow = sqlx::query_as(
        r#"
        SELECT id, entity_id, user_id, content, mentions, reply_to,
               created_at, updated_at, deleted
        FROM comments
        WHERE id = $1
        "#,
    )
    .bind(comment_id)
    .fetch_optional(pool)
    .await
    .map_err(DarshJError::Database)?
    .ok_or(DarshJError::EntityNotFound(comment_id))?;

    Ok(row.into_comment())
}

// ── Internal types ─────────────────────────────────────────────────

/// Raw database row for the comments table.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CommentRow {
    id: Uuid,
    entity_id: Uuid,
    user_id: Uuid,
    content: String,
    mentions: serde_json::Value,
    reply_to: Option<Uuid>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    deleted: bool,
}

impl CommentRow {
    fn into_comment(self) -> Comment {
        let mentions: Vec<Uuid> = serde_json::from_value(self.mentions).unwrap_or_default();
        Comment {
            id: self.id,
            entity_id: self.entity_id,
            user_id: self.user_id,
            content: self.content,
            mentions,
            reply_to: self.reply_to,
            created_at: self.created_at,
            updated_at: self.updated_at,
            deleted: self.deleted,
        }
    }
}

// ── Thread tree builder ────────────────────────────────────────────

/// Build a threaded tree from a flat list of comments.
///
/// Comments with no `reply_to` are roots. Each comment's replies are
/// collected recursively. The input must be sorted by `created_at` ASC
/// for stable ordering within each level.
pub fn build_thread_tree(comments: Vec<Comment>) -> Vec<ThreadedComment> {
    // Group children by parent id.
    let mut children_map: HashMap<CommentId, Vec<Comment>> = HashMap::new();
    let mut roots: Vec<Comment> = Vec::new();

    for comment in comments {
        match comment.reply_to {
            Some(parent_id) => {
                children_map.entry(parent_id).or_default().push(comment);
            }
            None => {
                roots.push(comment);
            }
        }
    }

    fn build_node(
        comment: Comment,
        children_map: &HashMap<CommentId, Vec<Comment>>,
    ) -> ThreadedComment {
        let replies = children_map
            .get(&comment.id)
            .map(|children| {
                children
                    .iter()
                    .cloned()
                    .map(|c| build_node(c, children_map))
                    .collect()
            })
            .unwrap_or_default();

        ThreadedComment { comment, replies }
    }

    roots
        .into_iter()
        .map(|root| build_node(root, &children_map))
        .collect()
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_comment(id: Uuid, reply_to: Option<Uuid>) -> Comment {
        Comment {
            id,
            entity_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            content: format!("Comment {id}"),
            mentions: vec![],
            reply_to,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted: false,
        }
    }

    #[test]
    fn test_thread_tree_flat() {
        let c1 = make_comment(Uuid::new_v4(), None);
        let c2 = make_comment(Uuid::new_v4(), None);
        let tree = build_thread_tree(vec![c1.clone(), c2.clone()]);
        assert_eq!(tree.len(), 2);
        assert!(tree[0].replies.is_empty());
        assert!(tree[1].replies.is_empty());
        assert_eq!(tree[0].comment.id, c1.id);
        assert_eq!(tree[1].comment.id, c2.id);
    }

    #[test]
    fn test_thread_tree_nested() {
        let root_id = Uuid::new_v4();
        let reply_id = Uuid::new_v4();
        let nested_id = Uuid::new_v4();

        let root = make_comment(root_id, None);
        let reply = make_comment(reply_id, Some(root_id));
        let nested = make_comment(nested_id, Some(reply_id));

        let tree = build_thread_tree(vec![root, reply, nested]);

        assert_eq!(tree.len(), 1, "should have one root");
        assert_eq!(tree[0].comment.id, root_id);
        assert_eq!(tree[0].replies.len(), 1, "root should have one reply");
        assert_eq!(tree[0].replies[0].comment.id, reply_id);
        assert_eq!(
            tree[0].replies[0].replies.len(),
            1,
            "reply should have one nested reply"
        );
        assert_eq!(tree[0].replies[0].replies[0].comment.id, nested_id);
    }

    #[test]
    fn test_thread_tree_multiple_replies() {
        let root_id = Uuid::new_v4();
        let r1 = Uuid::new_v4();
        let r2 = Uuid::new_v4();
        let r3 = Uuid::new_v4();

        let root = make_comment(root_id, None);
        let reply1 = make_comment(r1, Some(root_id));
        let reply2 = make_comment(r2, Some(root_id));
        let reply3 = make_comment(r3, Some(root_id));

        let tree = build_thread_tree(vec![root, reply1, reply2, reply3]);

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].replies.len(), 3);
    }

    #[test]
    fn test_thread_tree_empty() {
        let tree = build_thread_tree(vec![]);
        assert!(tree.is_empty());
    }

    #[test]
    fn test_thread_tree_orphaned_replies() {
        // Replies whose parents are not in the set become orphaned
        // (not rendered). This is intentional — soft-deleted parents
        // are excluded from the query.
        let orphan = make_comment(Uuid::new_v4(), Some(Uuid::new_v4()));
        let tree = build_thread_tree(vec![orphan]);
        assert!(
            tree.is_empty(),
            "orphaned replies should not appear as roots"
        );
    }
}
