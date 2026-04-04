//! Delta diff engine for query result sets.
//!
//! Computes minimal diffs between two snapshots of a query's result set,
//! producing [`QueryDiff`] with added, removed, and updated entities.
//! Uses hash-based change detection to avoid deep comparisons when entities
//! have not changed.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A diff between two query result snapshots.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryDiff {
    /// Entities present in `new` but not in `old`.
    pub added: Vec<Value>,

    /// Entity IDs present in `old` but not in `new`.
    pub removed: Vec<String>,

    /// Entities present in both but with changed fields.
    pub updated: Vec<EntityPatch>,
}

impl QueryDiff {
    /// Returns `true` if the diff contains no changes.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.updated.is_empty()
    }

    /// Total number of changes across all categories.
    pub fn change_count(&self) -> usize {
        self.added.len() + self.removed.len() + self.updated.len()
    }
}

/// A patch for a single entity describing which fields changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityPatch {
    /// The entity ID that was updated.
    pub entity_id: String,

    /// Map of field name to new value for fields that changed.
    pub changed_fields: HashMap<String, Value>,

    /// Fields that were removed (present in old, absent in new).
    pub removed_fields: Vec<String>,
}

/// Extract the entity ID from a result row.
/// Looks for `_id`, `id`, or `entity_id` fields, in that order.
fn extract_entity_id(entity: &Value) -> Option<String> {
    let obj = entity.as_object()?;
    for key in &["_id", "id", "entity_id"] {
        if let Some(val) = obj.get(*key) {
            return match val {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => Some(val.to_string()),
            };
        }
    }
    None
}

/// Compute a deterministic hash of a JSON value for change detection.
pub fn hash_value(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    // Serialize to canonical form for consistent hashing.
    // serde_json's to_string produces deterministic output for the same Value.
    let canonical = serde_json::to_string(value).unwrap_or_default();
    canonical.hash(&mut hasher);
    hasher.finish()
}

/// Compute a hash over an entire result set for quick equality checks.
pub fn hash_result_set(results: &[Value]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for value in results {
        let canonical = serde_json::to_string(value).unwrap_or_default();
        canonical.hash(&mut hasher);
    }
    hasher.finish()
}

/// Compute the diff between an old and new query result set.
///
/// Entities are matched by their ID field (`_id`, `id`, or `entity_id`).
/// Entities without an identifiable ID are treated as opaque -- they appear
/// as removed from old and added in new if the sets differ.
///
/// # Arguments
///
/// * `old` - Previous result set snapshot.
/// * `new` - Current result set snapshot.
///
/// # Returns
///
/// A [`QueryDiff`] describing the minimal set of changes.
pub fn compute_diff(old: &[Value], new: &[Value]) -> QueryDiff {
    // Quick path: if hashes match, no changes.
    if hash_result_set(old) == hash_result_set(new) {
        return QueryDiff::default();
    }

    // Index old results by entity ID.
    let mut old_by_id: HashMap<String, &Value> = HashMap::with_capacity(old.len());
    let mut old_without_id: Vec<&Value> = Vec::new();

    for entity in old {
        match extract_entity_id(entity) {
            Some(id) => {
                old_by_id.insert(id, entity);
            }
            None => {
                old_without_id.push(entity);
            }
        }
    }

    // Index new results by entity ID.
    let mut new_by_id: HashMap<String, &Value> = HashMap::with_capacity(new.len());
    let mut new_without_id: Vec<&Value> = Vec::new();

    for entity in new {
        match extract_entity_id(entity) {
            Some(id) => {
                new_by_id.insert(id, entity);
            }
            None => {
                new_without_id.push(entity);
            }
        }
    }

    let mut diff = QueryDiff::default();

    // Find removed entities (in old but not in new).
    let old_ids: HashSet<&String> = old_by_id.keys().collect();
    let new_ids: HashSet<&String> = new_by_id.keys().collect();

    for id in old_ids.difference(&new_ids) {
        diff.removed.push((*id).clone());
    }

    // Find added entities (in new but not in old).
    for id in new_ids.difference(&old_ids) {
        if let Some(entity) = new_by_id.get(*id) {
            diff.added.push((*entity).clone());
        }
    }

    // Find updated entities (in both, but changed).
    for id in old_ids.intersection(&new_ids) {
        let old_entity = old_by_id[*id];
        let new_entity = new_by_id[*id];

        // Fast path: hash comparison.
        if hash_value(old_entity) == hash_value(new_entity) {
            continue;
        }

        // Compute field-level diff.
        if let Some(patch) = compute_entity_patch(id, old_entity, new_entity) {
            diff.updated.push(patch);
        }
    }

    // Handle entities without IDs: treat all old ones as removed-equivalent
    // and all new ones as added, but only if the sets actually differ.
    let old_hashes: HashSet<u64> = old_without_id.iter().map(|v| hash_value(v)).collect();
    let new_hashes: HashSet<u64> = new_without_id.iter().map(|v| hash_value(v)).collect();

    if old_hashes != new_hashes {
        // For unkeyed entities, we can't do field-level patches.
        // Remove any that disappeared and add any that appeared.
        for entity in &new_without_id {
            let h = hash_value(entity);
            if !old_hashes.contains(&h) {
                diff.added.push((*entity).clone());
            }
        }
        // We can't produce meaningful IDs for removed unkeyed entities,
        // so we emit a sentinel.
        for entity in &old_without_id {
            let h = hash_value(entity);
            if !new_hashes.contains(&h) {
                diff.removed.push(format!("__unkeyed:{h}"));
            }
        }
    }

    diff
}

/// Compute a field-level patch between two versions of the same entity.
fn compute_entity_patch(entity_id: &str, old: &Value, new: &Value) -> Option<EntityPatch> {
    let old_obj = old.as_object()?;
    let new_obj = new.as_object()?;

    let mut changed_fields = HashMap::new();
    let mut removed_fields = Vec::new();

    // Check for changed and added fields.
    for (key, new_val) in new_obj {
        match old_obj.get(key) {
            Some(old_val) if old_val == new_val => {}
            Some(_) | None => {
                changed_fields.insert(key.clone(), new_val.clone());
            }
        }
    }

    // Check for removed fields.
    for key in old_obj.keys() {
        if !new_obj.contains_key(key) {
            removed_fields.push(key.clone());
        }
    }

    if changed_fields.is_empty() && removed_fields.is_empty() {
        return None;
    }

    Some(EntityPatch {
        entity_id: entity_id.to_string(),
        changed_fields,
        removed_fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identical_results_produce_empty_diff() {
        let data = vec![json!({"_id": "1", "name": "Alice"})];
        let diff = compute_diff(&data, &data);
        assert!(diff.is_empty());
    }

    #[test]
    fn detects_added_entities() {
        let old = vec![json!({"_id": "1", "name": "Alice"})];
        let new = vec![
            json!({"_id": "1", "name": "Alice"}),
            json!({"_id": "2", "name": "Bob"}),
        ];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 0);
        assert_eq!(diff.updated.len(), 0);
    }

    #[test]
    fn detects_removed_entities() {
        let old = vec![
            json!({"_id": "1", "name": "Alice"}),
            json!({"_id": "2", "name": "Bob"}),
        ];
        let new = vec![json!({"_id": "1", "name": "Alice"})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.removed, vec!["2"]);
    }

    #[test]
    fn detects_field_changes() {
        let old = vec![json!({"_id": "1", "name": "Alice", "age": 30})];
        let new = vec![json!({"_id": "1", "name": "Alice", "age": 31})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(diff.updated[0].entity_id, "1");
        assert_eq!(diff.updated[0].changed_fields.get("age"), Some(&json!(31)));
    }

    #[test]
    fn detects_removed_fields() {
        let old = vec![json!({"_id": "1", "name": "Alice", "temp": true})];
        let new = vec![json!({"_id": "1", "name": "Alice"})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(diff.updated[0].removed_fields, vec!["temp"]);
    }
}
