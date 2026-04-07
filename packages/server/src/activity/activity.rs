//! Activity log — full audit trail of who changed what and when.
//!
//! Every mutation, comment, share, or link operation is recorded as an
//! [`ActivityEntry`] in the `activity_log` table. Each entry captures:
//!
//! - The action performed ([`Action`] enum)
//! - The entity type and id affected
//! - The user who performed the action
//! - A list of field-level changes ([`FieldChange`])
//! - Optional metadata (JSON blob for action-specific context)
//!
//! The activity log is append-only and never modified after insertion.
//! It complements the Merkle audit trail with human-readable context.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};

// ── Types ──────────────────────────────────────────────────────────

/// The kind of action that was performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Action {
    /// A new record was created.
    Created,
    /// An existing record was updated.
    Updated,
    /// A record was deleted (soft or hard).
    Deleted,
    /// A comment was added to a record.
    Commented,
    /// A record was shared with another user.
    Shared,
    /// A reference link was established between two records.
    LinkedRecord,
    /// A reference link was removed between two records.
    UnlinkedRecord,
}

impl Action {
    /// Convert from a string representation.
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "created" => Some(Self::Created),
            "updated" => Some(Self::Updated),
            "deleted" => Some(Self::Deleted),
            "commented" => Some(Self::Commented),
            "shared" => Some(Self::Shared),
            "linked_record" => Some(Self::LinkedRecord),
            "unlinked_record" => Some(Self::UnlinkedRecord),
            _ => None,
        }
    }

    /// Canonical string form for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Deleted => "deleted",
            Self::Commented => "commented",
            Self::Shared => "shared",
            Self::LinkedRecord => "linked_record",
            Self::UnlinkedRecord => "unlinked_record",
        }
    }
}

/// A single field-level change within an activity entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldChange {
    /// The name of the field/attribute that changed.
    pub field_name: String,
    /// The previous value (None for newly created fields).
    pub old_value: Option<serde_json::Value>,
    /// The new value (None for deleted fields).
    pub new_value: Option<serde_json::Value>,
}

/// A single entry in the activity log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    /// Unique activity entry identifier.
    pub id: Uuid,
    /// The type of entity affected (e.g. "user", "order", "product").
    pub entity_type: String,
    /// The specific entity's UUID.
    pub entity_id: Uuid,
    /// What action was performed.
    pub action: Action,
    /// Who performed the action.
    pub user_id: Uuid,
    /// When the action occurred.
    pub timestamp: DateTime<Utc>,
    /// Field-level changes (empty for actions like Commented, Shared).
    pub changes: Vec<FieldChange>,
    /// Action-specific metadata (e.g. comment id, share target, etc.).
    pub metadata: Option<serde_json::Value>,
}

/// Input for recording a new activity entry (id and timestamp auto-assigned).
#[derive(Debug, Clone)]
pub struct RecordActivityInput {
    /// The type of entity affected.
    pub entity_type: String,
    /// The specific entity's UUID.
    pub entity_id: Uuid,
    /// What action was performed.
    pub action: Action,
    /// Who performed the action.
    pub user_id: Uuid,
    /// Field-level changes.
    pub changes: Vec<FieldChange>,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
}

// ── Schema ─────────────────────────────────────────────────────────

/// Create the `activity_log` table if it does not exist.
pub async fn ensure_activity_schema(pool: &PgPool) -> std::result::Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS activity_log (
            id           UUID PRIMARY KEY,
            entity_type  TEXT NOT NULL,
            entity_id    UUID NOT NULL,
            action       TEXT NOT NULL,
            user_id      UUID NOT NULL,
            timestamp    TIMESTAMPTZ NOT NULL DEFAULT now(),
            changes      JSONB NOT NULL DEFAULT '[]'::jsonb,
            metadata     JSONB
        );

        CREATE INDEX IF NOT EXISTS idx_activity_entity
            ON activity_log (entity_id, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_activity_user
            ON activity_log (user_id, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_activity_entity_type
            ON activity_log (entity_type, timestamp DESC);
        CREATE INDEX IF NOT EXISTS idx_activity_timestamp
            ON activity_log (timestamp DESC);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ── Core operations ────────────────────────────────────────────────

/// Record an activity entry in the log.
///
/// This is the primary entry point for the activity system. All mutations
/// should call this after successfully applying changes.
pub async fn record_activity(pool: &PgPool, input: RecordActivityInput) -> Result<ActivityEntry> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let changes_json = serde_json::to_value(&input.changes)
        .map_err(|e| DarshJError::Internal(format!("Failed to serialize changes: {e}")))?;

    sqlx::query(
        r#"
        INSERT INTO activity_log (id, entity_type, entity_id, action, user_id, timestamp, changes, metadata)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(id)
    .bind(&input.entity_type)
    .bind(input.entity_id)
    .bind(input.action.as_str())
    .bind(input.user_id)
    .bind(now)
    .bind(&changes_json)
    .bind(&input.metadata)
    .execute(pool)
    .await
    .map_err(DarshJError::Database)?;

    Ok(ActivityEntry {
        id,
        entity_type: input.entity_type,
        entity_id: input.entity_id,
        action: input.action,
        user_id: input.user_id,
        timestamp: now,
        changes: input.changes,
        metadata: input.metadata,
    })
}

/// Get the activity log for a specific entity (record).
///
/// Returns entries in reverse chronological order (newest first),
/// limited to `limit` entries.
pub async fn get_activity(
    pool: &PgPool,
    entity_id: Uuid,
    limit: u32,
) -> Result<Vec<ActivityEntry>> {
    let rows: Vec<ActivityRow> = sqlx::query_as(
        r#"
        SELECT id, entity_type, entity_id, action, user_id, timestamp, changes, metadata
        FROM activity_log
        WHERE entity_id = $1
        ORDER BY timestamp DESC
        LIMIT $2
        "#,
    )
    .bind(entity_id)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(DarshJError::Database)?;

    Ok(rows.into_iter().map(|r| r.into_entry()).collect())
}

/// Get all activity by a specific user.
///
/// Returns entries in reverse chronological order, limited to `limit`.
pub async fn get_user_activity(
    pool: &PgPool,
    user_id: Uuid,
    limit: u32,
) -> Result<Vec<ActivityEntry>> {
    let rows: Vec<ActivityRow> = sqlx::query_as(
        r#"
        SELECT id, entity_type, entity_id, action, user_id, timestamp, changes, metadata
        FROM activity_log
        WHERE user_id = $1
        ORDER BY timestamp DESC
        LIMIT $2
        "#,
    )
    .bind(user_id)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(DarshJError::Database)?;

    Ok(rows.into_iter().map(|r| r.into_entry()).collect())
}

/// Get all activity for an entity type (table-level view).
///
/// Returns entries in reverse chronological order, limited to `limit`.
pub async fn get_table_activity(
    pool: &PgPool,
    entity_type: &str,
    limit: u32,
) -> Result<Vec<ActivityEntry>> {
    let rows: Vec<ActivityRow> = sqlx::query_as(
        r#"
        SELECT id, entity_type, entity_id, action, user_id, timestamp, changes, metadata
        FROM activity_log
        WHERE entity_type = $1
        ORDER BY timestamp DESC
        LIMIT $2
        "#,
    )
    .bind(entity_type)
    .bind(limit as i64)
    .fetch_all(pool)
    .await
    .map_err(DarshJError::Database)?;

    Ok(rows.into_iter().map(|r| r.into_entry()).collect())
}

// ── Mutation-pipeline hook ─────────────────────────────────────────

/// Compute field-level changes by diffing old and new attribute maps.
///
/// Call this before a mutation to capture what changed. Pass the result
/// into [`record_activity`] after the mutation succeeds.
pub fn diff_fields(
    old: &[(String, serde_json::Value)],
    new: &[(String, serde_json::Value)],
) -> Vec<FieldChange> {
    let old_map: std::collections::HashMap<&str, &serde_json::Value> =
        old.iter().map(|(k, v)| (k.as_str(), v)).collect();
    let new_map: std::collections::HashMap<&str, &serde_json::Value> =
        new.iter().map(|(k, v)| (k.as_str(), v)).collect();

    let mut changes = Vec::new();

    // Check for updated and deleted fields.
    for (key, old_val) in &old_map {
        match new_map.get(key) {
            Some(new_val) if *new_val != *old_val => {
                changes.push(FieldChange {
                    field_name: key.to_string(),
                    old_value: Some((*old_val).clone()),
                    new_value: Some((*new_val).clone()),
                });
            }
            None => {
                changes.push(FieldChange {
                    field_name: key.to_string(),
                    old_value: Some((*old_val).clone()),
                    new_value: None,
                });
            }
            _ => {} // unchanged
        }
    }

    // Check for newly added fields.
    for (key, new_val) in &new_map {
        if !old_map.contains_key(key) {
            changes.push(FieldChange {
                field_name: key.to_string(),
                old_value: None,
                new_value: Some((*new_val).clone()),
            });
        }
    }

    changes
}

// ── Internal types ─────────────────────────────────────────────────

/// Raw database row for the activity_log table.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ActivityRow {
    id: Uuid,
    entity_type: String,
    entity_id: Uuid,
    action: String,
    user_id: Uuid,
    timestamp: DateTime<Utc>,
    changes: serde_json::Value,
    metadata: Option<serde_json::Value>,
}

impl ActivityRow {
    fn into_entry(self) -> ActivityEntry {
        let action = Action::from_str_opt(&self.action).unwrap_or(Action::Updated);
        let changes: Vec<FieldChange> =
            serde_json::from_value(self.changes).unwrap_or_default();

        ActivityEntry {
            id: self.id,
            entity_type: self.entity_type,
            entity_id: self.entity_id,
            action,
            user_id: self.user_id,
            timestamp: self.timestamp,
            changes,
            metadata: self.metadata,
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_action_roundtrip() {
        let actions = [
            Action::Created,
            Action::Updated,
            Action::Deleted,
            Action::Commented,
            Action::Shared,
            Action::LinkedRecord,
            Action::UnlinkedRecord,
        ];

        for action in actions {
            let s = action.as_str();
            let parsed = Action::from_str_opt(s).expect("should parse back");
            assert_eq!(parsed, action);
        }
    }

    #[test]
    fn test_action_serialization() {
        let json = serde_json::to_value(Action::LinkedRecord).unwrap();
        assert_eq!(json, "linked_record");

        let parsed: Action = serde_json::from_value(json!("created")).unwrap();
        assert_eq!(parsed, Action::Created);
    }

    #[test]
    fn test_diff_fields_no_changes() {
        let old = vec![("name".into(), json!("Alice"))];
        let new = vec![("name".into(), json!("Alice"))];
        let changes = diff_fields(&old, &new);
        assert!(changes.is_empty());
    }

    #[test]
    fn test_diff_fields_updated() {
        let old = vec![("name".into(), json!("Alice"))];
        let new = vec![("name".into(), json!("Bob"))];
        let changes = diff_fields(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field_name, "name");
        assert_eq!(changes[0].old_value, Some(json!("Alice")));
        assert_eq!(changes[0].new_value, Some(json!("Bob")));
    }

    #[test]
    fn test_diff_fields_added() {
        let old: Vec<(String, serde_json::Value)> = vec![];
        let new = vec![("email".into(), json!("a@b.com"))];
        let changes = diff_fields(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field_name, "email");
        assert!(changes[0].old_value.is_none());
        assert_eq!(changes[0].new_value, Some(json!("a@b.com")));
    }

    #[test]
    fn test_diff_fields_removed() {
        let old = vec![("email".into(), json!("a@b.com"))];
        let new: Vec<(String, serde_json::Value)> = vec![];
        let changes = diff_fields(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field_name, "email");
        assert_eq!(changes[0].old_value, Some(json!("a@b.com")));
        assert!(changes[0].new_value.is_none());
    }

    #[test]
    fn test_diff_fields_mixed() {
        let old = vec![
            ("name".into(), json!("Alice")),
            ("age".into(), json!(30)),
            ("email".into(), json!("old@example.com")),
        ];
        let new = vec![
            ("name".into(), json!("Alice")),     // unchanged
            ("age".into(), json!(31)),            // updated
            ("phone".into(), json!("+1234")),     // added
            // email removed
        ];
        let changes = diff_fields(&old, &new);
        assert_eq!(changes.len(), 3);

        let names: Vec<&str> = changes.iter().map(|c| c.field_name.as_str()).collect();
        assert!(names.contains(&"age"));
        assert!(names.contains(&"email"));
        assert!(names.contains(&"phone"));
    }

    #[test]
    fn test_activity_entry_serialization() {
        let entry = ActivityEntry {
            id: Uuid::new_v4(),
            entity_type: "user".into(),
            entity_id: Uuid::new_v4(),
            action: Action::Updated,
            user_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            changes: vec![FieldChange {
                field_name: "name".into(),
                old_value: Some(json!("Alice")),
                new_value: Some(json!("Bob")),
            }],
            metadata: Some(json!({"source": "api"})),
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["action"], "updated");
        assert_eq!(json["entity_type"], "user");
        assert!(json["changes"].is_array());
        assert_eq!(json["changes"][0]["field_name"], "name");
    }
}
