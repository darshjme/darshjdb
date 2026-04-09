//! Record version history reconstruction from the triple store.
//!
//! Every distinct `tx_id` that touches an entity represents a version
//! boundary. By replaying triples in `tx_id` order, we can reconstruct
//! the full state of any record at any version or point in time.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::Triple;

// ── Types ─────────────────────────────────────────────────────────

/// The type of change applied to a single field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    /// A new attribute was added for the first time.
    Added,
    /// An existing attribute's value was changed.
    Modified,
    /// An attribute was retracted (removed).
    Removed,
}

/// A single field-level change within a version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldChange {
    /// The attribute that changed.
    pub attribute: String,
    /// The value before this change (`None` if the attribute was newly added).
    pub old_value: Option<Value>,
    /// The value after this change (`None` if the attribute was removed).
    pub new_value: Option<Value>,
    /// Whether this was an add, modify, or remove.
    pub change_type: ChangeType,
}

/// A single version of a record's state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordVersion {
    /// 1-based version number (chronological order).
    pub version_number: u32,
    /// The transaction id that created this version.
    pub tx_id: i64,
    /// Who made this change (if tracked via a `_changed_by` attribute).
    pub changed_by: Option<Uuid>,
    /// When this version was created.
    pub changed_at: DateTime<Utc>,
    /// The individual field changes in this version.
    pub changes: Vec<FieldChange>,
    /// The complete record state after this version's changes.
    pub snapshot: HashMap<String, Value>,
}

// ── Core reconstruction ───────────────────────────────────────────

/// Fetch all triples (active and retracted) for an entity, ordered by tx_id.
async fn fetch_all_triples(pool: &PgPool, entity_id: Uuid) -> Result<Vec<Triple>> {
    let triples = sqlx::query_as::<_, Triple>(
        r#"
        SELECT id, entity_id, attribute, value, value_type, tx_id,
               created_at, retracted, expires_at
        FROM triples
        WHERE entity_id = $1
        ORDER BY tx_id ASC, id ASC
        "#,
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;

    Ok(triples)
}

/// Replay triples to build the version history for an entity.
///
/// Groups triples by `tx_id`. For each transaction, computes the diff
/// against the previous state and produces a [`RecordVersion`].
pub(crate) fn build_versions(triples: &[Triple]) -> Vec<RecordVersion> {
    if triples.is_empty() {
        return Vec::new();
    }

    // Group triples by tx_id, preserving order.
    let mut tx_groups: Vec<(i64, DateTime<Utc>, Vec<&Triple>)> = Vec::new();
    for triple in triples {
        if let Some(last) = tx_groups.last_mut()
            && last.0 == triple.tx_id
        {
            last.2.push(triple);
            continue;
        }
        tx_groups.push((triple.tx_id, triple.created_at, vec![triple]));
    }

    let mut versions = Vec::new();
    let mut state: HashMap<String, Value> = HashMap::new();
    let mut version_number: u32 = 0;

    for (tx_id, created_at, group) in &tx_groups {
        let prev_state = state.clone();
        let mut changed_by: Option<Uuid> = None;

        // Apply this transaction's triples to the running state.
        for triple in group {
            if triple.retracted {
                state.remove(&triple.attribute);
            } else {
                state.insert(triple.attribute.clone(), triple.value.clone());
            }

            // Check for a `_changed_by` meta-attribute.
            if triple.attribute == "_changed_by"
                && !triple.retracted
                && let Some(s) = triple.value.as_str()
            {
                changed_by = Uuid::parse_str(s).ok();
            }
        }

        // Compute changes between prev_state and current state.
        let mut changes = Vec::new();

        // Find modified and added attributes.
        for (attr, new_val) in &state {
            match prev_state.get(attr) {
                Some(old_val) if old_val != new_val => {
                    changes.push(FieldChange {
                        attribute: attr.clone(),
                        old_value: Some(old_val.clone()),
                        new_value: Some(new_val.clone()),
                        change_type: ChangeType::Modified,
                    });
                }
                None => {
                    changes.push(FieldChange {
                        attribute: attr.clone(),
                        old_value: None,
                        new_value: Some(new_val.clone()),
                        change_type: ChangeType::Added,
                    });
                }
                _ => {} // Unchanged.
            }
        }

        // Find removed attributes.
        for (attr, old_val) in &prev_state {
            if !state.contains_key(attr) {
                changes.push(FieldChange {
                    attribute: attr.clone(),
                    old_value: Some(old_val.clone()),
                    new_value: None,
                    change_type: ChangeType::Removed,
                });
            }
        }

        // Sort changes for deterministic output.
        changes.sort_by(|a, b| a.attribute.cmp(&b.attribute));

        version_number += 1;
        versions.push(RecordVersion {
            version_number,
            tx_id: *tx_id,
            changed_by,
            changed_at: *created_at,
            changes,
            snapshot: state.clone(),
        });
    }

    versions
}

// ── Public API ────────────────────────────────────────────────────

/// Get the full version history of a record.
///
/// Returns versions in chronological order (oldest first). The `limit`
/// parameter caps the number of versions returned (0 = unlimited).
pub async fn get_history(pool: &PgPool, entity_id: Uuid, limit: u32) -> Result<Vec<RecordVersion>> {
    let triples = fetch_all_triples(pool, entity_id).await?;
    if triples.is_empty() {
        return Err(DarshJError::EntityNotFound(entity_id));
    }

    let mut versions = build_versions(&triples);

    if limit > 0 {
        let len = versions.len();
        if len > limit as usize {
            // Return the most recent `limit` versions.
            versions = versions.split_off(len - limit as usize);
        }
    }

    Ok(versions)
}

/// Reconstruct a record's state at a specific version number.
///
/// Version numbers are 1-based. Returns the attribute map as it existed
/// after the given version's transaction was applied.
pub async fn get_version(
    pool: &PgPool,
    entity_id: Uuid,
    version: u32,
) -> Result<HashMap<String, Value>> {
    if version == 0 {
        return Err(DarshJError::InvalidQuery(
            "version number must be >= 1".into(),
        ));
    }

    let triples = fetch_all_triples(pool, entity_id).await?;
    if triples.is_empty() {
        return Err(DarshJError::EntityNotFound(entity_id));
    }

    let versions = build_versions(&triples);
    versions
        .into_iter()
        .find(|v| v.version_number == version)
        .map(|v| v.snapshot)
        .ok_or_else(|| {
            DarshJError::InvalidQuery(format!(
                "version {version} does not exist for entity {entity_id}"
            ))
        })
}

/// Reconstruct a record's state at a specific point in time.
///
/// Returns the record as it existed at or before the given timestamp.
/// If the entity did not exist at that time, returns an error.
pub async fn get_at_time(
    pool: &PgPool,
    entity_id: Uuid,
    timestamp: DateTime<Utc>,
) -> Result<HashMap<String, Value>> {
    let triples = sqlx::query_as::<_, Triple>(
        r#"
        SELECT id, entity_id, attribute, value, value_type, tx_id,
               created_at, retracted, expires_at
        FROM triples
        WHERE entity_id = $1 AND created_at <= $2
        ORDER BY tx_id ASC, id ASC
        "#,
    )
    .bind(entity_id)
    .bind(timestamp)
    .fetch_all(pool)
    .await?;

    if triples.is_empty() {
        return Err(DarshJError::EntityNotFound(entity_id));
    }

    let versions = build_versions(&triples);
    versions
        .into_iter()
        .last()
        .map(|v| v.snapshot)
        .ok_or_else(|| DarshJError::EntityNotFound(entity_id))
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_triple(
        entity_id: Uuid,
        attribute: &str,
        value: Value,
        tx_id: i64,
        retracted: bool,
    ) -> Triple {
        Triple {
            id: 0,
            entity_id,
            attribute: attribute.to_string(),
            value,
            value_type: 0,
            tx_id,
            created_at: Utc::now(),
            retracted,
            expires_at: None,
        }
    }

    #[test]
    fn test_build_versions_empty() {
        assert!(build_versions(&[]).is_empty());
    }

    #[test]
    fn test_build_versions_single_tx() {
        let id = Uuid::new_v4();
        let triples = vec![
            make_triple(id, "name", json!("Alice"), 1, false),
            make_triple(id, "email", json!("alice@test.com"), 1, false),
        ];

        let versions = build_versions(&triples);
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version_number, 1);
        assert_eq!(versions[0].tx_id, 1);
        assert_eq!(versions[0].snapshot.len(), 2);
        assert_eq!(versions[0].snapshot["name"], json!("Alice"));
        assert_eq!(versions[0].snapshot["email"], json!("alice@test.com"));

        // All fields should be Added.
        assert_eq!(versions[0].changes.len(), 2);
        for change in &versions[0].changes {
            assert_eq!(change.change_type, ChangeType::Added);
            assert!(change.old_value.is_none());
            assert!(change.new_value.is_some());
        }
    }

    #[test]
    fn test_build_versions_modification() {
        let id = Uuid::new_v4();
        let triples = vec![
            make_triple(id, "name", json!("Alice"), 1, false),
            make_triple(id, "email", json!("a@old.com"), 1, false),
            // tx 2: retract old email, add new one
            make_triple(id, "email", json!("a@old.com"), 2, true),
            make_triple(id, "email", json!("a@new.com"), 2, false),
        ];

        let versions = build_versions(&triples);
        assert_eq!(versions.len(), 2);

        // Version 1: initial state.
        assert_eq!(versions[0].snapshot["email"], json!("a@old.com"));

        // Version 2: email changed.
        assert_eq!(versions[1].snapshot["email"], json!("a@new.com"));
        assert_eq!(versions[1].snapshot["name"], json!("Alice")); // unchanged

        let email_change = versions[1]
            .changes
            .iter()
            .find(|c| c.attribute == "email")
            .unwrap();
        assert_eq!(email_change.change_type, ChangeType::Modified);
        assert_eq!(email_change.old_value, Some(json!("a@old.com")));
        assert_eq!(email_change.new_value, Some(json!("a@new.com")));
    }

    #[test]
    fn test_build_versions_removal() {
        let id = Uuid::new_v4();
        let triples = vec![
            make_triple(id, "name", json!("Alice"), 1, false),
            make_triple(id, "temp", json!("data"), 1, false),
            // tx 2: retract temp
            make_triple(id, "temp", json!("data"), 2, true),
        ];

        let versions = build_versions(&triples);
        assert_eq!(versions.len(), 2);

        assert!(versions[0].snapshot.contains_key("temp"));
        assert!(!versions[1].snapshot.contains_key("temp"));

        let removal = versions[1]
            .changes
            .iter()
            .find(|c| c.attribute == "temp")
            .unwrap();
        assert_eq!(removal.change_type, ChangeType::Removed);
        assert_eq!(removal.old_value, Some(json!("data")));
        assert!(removal.new_value.is_none());
    }

    #[test]
    fn test_build_versions_multi_tx_state_accumulation() {
        let id = Uuid::new_v4();
        let triples = vec![
            make_triple(id, "name", json!("Alice"), 1, false),
            make_triple(id, "email", json!("a@x.com"), 2, false),
            make_triple(id, "age", json!(30), 3, false),
        ];

        let versions = build_versions(&triples);
        assert_eq!(versions.len(), 3);

        // Each version accumulates state.
        assert_eq!(versions[0].snapshot.len(), 1);
        assert_eq!(versions[1].snapshot.len(), 2);
        assert_eq!(versions[2].snapshot.len(), 3);
        assert_eq!(versions[2].snapshot["name"], json!("Alice"));
        assert_eq!(versions[2].snapshot["email"], json!("a@x.com"));
        assert_eq!(versions[2].snapshot["age"], json!(30));
    }

    #[test]
    fn test_build_versions_changed_by_tracking() {
        let id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let triples = vec![
            make_triple(id, "name", json!("Alice"), 1, false),
            make_triple(id, "_changed_by", json!(user_id.to_string()), 1, false),
        ];

        let versions = build_versions(&triples);
        assert_eq!(versions[0].changed_by, Some(user_id));
    }
}
