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

/// A single triple change within an entity patch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripleChange {
    /// The attribute (triple key) that changed.
    pub attribute: String,
    /// The value (new value for added/updated, old value for removed).
    pub value: Value,
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

    /// Triples that were added (new fields not in old).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added_triples: Vec<TripleChange>,

    /// Triples that were removed (old fields not in new).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed_triples: Vec<TripleChange>,

    /// Triples that were updated (same key, different value).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub updated_triples: Vec<TripleChange>,
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
///
/// Uses canonical (sorted-key) serialization to ensure logically equal
/// JSON objects produce identical hashes regardless of insertion order.
pub fn hash_value(value: &Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_value_recursive(value, &mut hasher);
    hasher.finish()
}

/// Recursively hash a JSON value with sorted object keys for canonical ordering.
fn hash_value_recursive(value: &Value, hasher: &mut DefaultHasher) {
    match value {
        Value::Null => 0u8.hash(hasher),
        Value::Bool(b) => {
            1u8.hash(hasher);
            b.hash(hasher);
        }
        Value::Number(n) => {
            2u8.hash(hasher);
            // Use string representation for consistent hashing of numbers.
            n.to_string().hash(hasher);
        }
        Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        Value::Array(arr) => {
            4u8.hash(hasher);
            arr.len().hash(hasher);
            for item in arr {
                hash_value_recursive(item, hasher);
            }
        }
        Value::Object(obj) => {
            5u8.hash(hasher);
            obj.len().hash(hasher);
            // Sort keys for canonical ordering -- critical for determinism.
            let mut keys: Vec<&String> = obj.keys().collect();
            keys.sort();
            for key in keys {
                key.hash(hasher);
                hash_value_recursive(&obj[key], hasher);
            }
        }
    }
}

/// Compute a hash over an entire result set for quick equality checks.
///
/// Uses XOR-combination of individual hashes so that result sets with the
/// same entities in different order produce the same hash. This avoids
/// spurious diffs when the query engine returns rows in non-deterministic order.
pub fn hash_result_set(results: &[Value]) -> u64 {
    let mut combined: u64 = 0;
    // Mix length into the hash to distinguish [] from [x] where hash(x)==0.
    let mut len_hasher = DefaultHasher::new();
    results.len().hash(&mut len_hasher);
    combined ^= len_hasher.finish();

    for value in results {
        combined ^= hash_value(value);
    }
    combined
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
    let mut added_triples = Vec::new();
    let mut removed_triples = Vec::new();
    let mut updated_triples = Vec::new();

    // Check for changed and added fields.
    for (key, new_val) in new_obj {
        match old_obj.get(key) {
            Some(old_val) if old_val == new_val => {}
            Some(_old_val) => {
                // Field existed before with a different value -- this is an update.
                changed_fields.insert(key.clone(), new_val.clone());
                updated_triples.push(TripleChange {
                    attribute: key.clone(),
                    value: new_val.clone(),
                });
            }
            None => {
                // Field is new -- this is an addition.
                changed_fields.insert(key.clone(), new_val.clone());
                added_triples.push(TripleChange {
                    attribute: key.clone(),
                    value: new_val.clone(),
                });
            }
        }
    }

    // Check for removed fields.
    for (key, old_val) in old_obj {
        if !new_obj.contains_key(key) {
            removed_fields.push(key.clone());
            removed_triples.push(TripleChange {
                attribute: key.clone(),
                value: old_val.clone(),
            });
        }
    }

    if changed_fields.is_empty() && removed_fields.is_empty() {
        return None;
    }

    Some(EntityPatch {
        entity_id: entity_id.to_string(),
        changed_fields,
        removed_fields,
        added_triples,
        removed_triples,
        updated_triples,
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

    #[test]
    fn empty_to_empty_produces_empty_diff() {
        let diff = compute_diff(&[], &[]);
        assert!(diff.is_empty());
        assert_eq!(diff.change_count(), 0);
    }

    #[test]
    fn empty_to_nonempty_all_added() {
        let new = vec![json!({"_id": "1", "x": 1}), json!({"_id": "2", "x": 2})];
        let diff = compute_diff(&[], &new);
        assert_eq!(diff.added.len(), 2);
        assert!(diff.removed.is_empty());
        assert!(diff.updated.is_empty());
    }

    #[test]
    fn nonempty_to_empty_all_removed() {
        let old = vec![json!({"_id": "1", "x": 1}), json!({"_id": "2", "x": 2})];
        let diff = compute_diff(&old, &[]);
        assert_eq!(diff.removed.len(), 2);
        assert!(diff.added.is_empty());
        assert!(diff.updated.is_empty());
    }

    #[test]
    fn nested_object_change_detected() {
        let old = vec![json!({"_id": "1", "meta": {"level": 1, "tag": "a"}})];
        let new = vec![json!({"_id": "1", "meta": {"level": 2, "tag": "a"}})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(
            diff.updated[0].changed_fields.get("meta"),
            Some(&json!({"level": 2, "tag": "a"}))
        );
    }

    #[test]
    fn array_field_change_detected() {
        let old = vec![json!({"_id": "1", "tags": ["a", "b"]})];
        let new = vec![json!({"_id": "1", "tags": ["a", "c"]})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(
            diff.updated[0].changed_fields.get("tags"),
            Some(&json!(["a", "c"]))
        );
    }

    #[test]
    fn null_value_changes() {
        let old = vec![json!({"_id": "1", "value": null})];
        let new = vec![json!({"_id": "1", "value": 42})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(
            diff.updated[0].changed_fields.get("value"),
            Some(&json!(42))
        );
    }

    #[test]
    fn value_to_null_change() {
        let old = vec![json!({"_id": "1", "value": 42})];
        let new = vec![json!({"_id": "1", "value": null})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(
            diff.updated[0].changed_fields.get("value"),
            Some(&json!(null))
        );
    }

    #[test]
    fn added_new_field() {
        let old = vec![json!({"_id": "1", "name": "Alice"})];
        let new = vec![json!({"_id": "1", "name": "Alice", "email": "a@b.c"})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(
            diff.updated[0].changed_fields.get("email"),
            Some(&json!("a@b.c"))
        );
        assert!(diff.updated[0].removed_fields.is_empty());
    }

    #[test]
    fn simultaneous_add_remove_update() {
        let old = vec![
            json!({"_id": "1", "name": "Alice"}),
            json!({"_id": "2", "name": "Bob"}),
        ];
        let new = vec![
            json!({"_id": "1", "name": "Alice2"}),
            json!({"_id": "3", "name": "Charlie"}),
        ];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.added.len(), 1); // Charlie
        assert_eq!(diff.removed, vec!["2"]); // Bob
        assert_eq!(diff.updated.len(), 1); // Alice -> Alice2
        assert_eq!(diff.change_count(), 3);
    }

    #[test]
    fn entities_with_numeric_ids() {
        let old = vec![json!({"id": 1, "val": "a"})];
        let new = vec![json!({"id": 1, "val": "b"})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(diff.updated[0].entity_id, "1");
    }

    #[test]
    fn entities_without_ids_treated_as_unkeyed() {
        let old = vec![json!({"name": "Alice"})];
        let new = vec![json!({"name": "Bob"})];
        let diff = compute_diff(&old, &new);
        // Old unkeyed entity removed, new unkeyed entity added.
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert!(diff.removed[0].starts_with("__unkeyed:"));
    }

    #[test]
    fn hash_value_canonical_key_order() {
        // Two objects with same keys in different insertion order must hash the same.
        let a = json!({"z": 1, "a": 2, "m": 3});
        let b = {
            let mut map = serde_json::Map::new();
            map.insert("a".into(), json!(2));
            map.insert("m".into(), json!(3));
            map.insert("z".into(), json!(1));
            Value::Object(map)
        };
        assert_eq!(hash_value(&a), hash_value(&b));
    }

    #[test]
    fn hash_value_nested_canonical() {
        let a = json!({"outer": {"z": 1, "a": 2}});
        let b = {
            let mut inner = serde_json::Map::new();
            inner.insert("a".into(), json!(2));
            inner.insert("z".into(), json!(1));
            let mut outer = serde_json::Map::new();
            outer.insert("outer".into(), Value::Object(inner));
            Value::Object(outer)
        };
        assert_eq!(hash_value(&a), hash_value(&b));
    }

    #[test]
    fn hash_result_set_order_independent() {
        let a = vec![json!({"_id": "1"}), json!({"_id": "2"})];
        let b = vec![json!({"_id": "2"}), json!({"_id": "1"})];
        assert_eq!(hash_result_set(&a), hash_result_set(&b));
    }

    #[test]
    fn hash_result_set_different_content() {
        let a = vec![json!({"_id": "1"})];
        let b = vec![json!({"_id": "2"})];
        assert_ne!(hash_result_set(&a), hash_result_set(&b));
    }

    #[test]
    fn hash_distinguishes_types() {
        assert_ne!(hash_value(&json!(null)), hash_value(&json!(false)));
        assert_ne!(hash_value(&json!(0)), hash_value(&json!("0")));
        assert_ne!(hash_value(&json!([])), hash_value(&json!({})));
    }

    #[test]
    fn deeply_nested_diff() {
        let old = vec![json!({"_id": "1", "a": {"b": {"c": {"d": 1}}}})];
        let new = vec![json!({"_id": "1", "a": {"b": {"c": {"d": 2}}}})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        assert!(diff.updated[0].changed_fields.contains_key("a"));
    }

    #[test]
    fn entity_id_extraction_priority() {
        // _id takes priority over id.
        let entity = json!({"_id": "preferred", "id": "fallback"});
        assert_eq!(extract_entity_id(&entity), Some("preferred".into()));

        // id used when no _id.
        let entity = json!({"id": "used", "entity_id": "fallback"});
        assert_eq!(extract_entity_id(&entity), Some("used".into()));

        // entity_id as last resort.
        let entity = json!({"entity_id": "last"});
        assert_eq!(extract_entity_id(&entity), Some("last".into()));
    }

    #[test]
    fn same_entities_different_order_no_diff() {
        let old = vec![
            json!({"_id": "1", "name": "Alice"}),
            json!({"_id": "2", "name": "Bob"}),
        ];
        let new = vec![
            json!({"_id": "2", "name": "Bob"}),
            json!({"_id": "1", "name": "Alice"}),
        ];
        let diff = compute_diff(&old, &new);
        assert!(diff.is_empty());
    }

    #[test]
    fn triple_change_added_field() {
        let old = vec![json!({"_id": "1", "name": "Alice"})];
        let new = vec![json!({"_id": "1", "name": "Alice", "email": "a@b.c"})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        let patch = &diff.updated[0];
        assert_eq!(patch.added_triples.len(), 1);
        assert_eq!(patch.added_triples[0].attribute, "email");
        assert_eq!(patch.added_triples[0].value, json!("a@b.c"));
        assert!(patch.updated_triples.is_empty());
        assert!(patch.removed_triples.is_empty());
    }

    #[test]
    fn triple_change_removed_field() {
        let old = vec![json!({"_id": "1", "name": "Alice", "temp": true})];
        let new = vec![json!({"_id": "1", "name": "Alice"})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        let patch = &diff.updated[0];
        assert_eq!(patch.removed_triples.len(), 1);
        assert_eq!(patch.removed_triples[0].attribute, "temp");
        assert_eq!(patch.removed_triples[0].value, json!(true));
        assert!(patch.added_triples.is_empty());
        assert!(patch.updated_triples.is_empty());
    }

    #[test]
    fn triple_change_updated_field() {
        let old = vec![json!({"_id": "1", "name": "Alice", "age": 30})];
        let new = vec![json!({"_id": "1", "name": "Alice", "age": 31})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        let patch = &diff.updated[0];
        assert_eq!(patch.updated_triples.len(), 1);
        assert_eq!(patch.updated_triples[0].attribute, "age");
        assert_eq!(patch.updated_triples[0].value, json!(31));
        assert!(patch.added_triples.is_empty());
        assert!(patch.removed_triples.is_empty());
    }

    #[test]
    fn triple_change_mixed_operations() {
        let old = vec![json!({"_id": "1", "name": "Alice", "age": 30, "temp": true})];
        let new = vec![json!({"_id": "1", "name": "Alice", "age": 31, "email": "a@b.c"})];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.updated.len(), 1);
        let patch = &diff.updated[0];
        // age changed (updated triple)
        assert_eq!(patch.updated_triples.len(), 1);
        assert_eq!(patch.updated_triples[0].attribute, "age");
        // email added (added triple)
        assert_eq!(patch.added_triples.len(), 1);
        assert_eq!(patch.added_triples[0].attribute, "email");
        // temp removed (removed triple)
        assert_eq!(patch.removed_triples.len(), 1);
        assert_eq!(patch.removed_triples[0].attribute, "temp");
    }

    #[test]
    fn query_diff_serializes_correctly() {
        let old = vec![json!({"_id": "1", "x": 1}), json!({"_id": "2", "y": 2})];
        let new = vec![json!({"_id": "1", "x": 2}), json!({"_id": "3", "z": 3})];
        let diff = compute_diff(&old, &new);
        let json_str = serde_json::to_string(&diff).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        // Verify it has the expected top-level keys.
        assert!(parsed.get("added").unwrap().is_array());
        assert!(parsed.get("removed").unwrap().is_array());
        assert!(parsed.get("updated").unwrap().is_array());
    }

    #[test]
    fn large_result_set_diff() {
        let old: Vec<Value> = (0..100)
            .map(|i| json!({"_id": i.to_string(), "val": i}))
            .collect();
        let mut new = old.clone();
        // Remove entity 50, add entity 100, update entity 25.
        new.retain(|v| v.get("_id").unwrap().as_str().unwrap() != "50");
        new.push(json!({"_id": "100", "val": 100}));
        for v in &mut new {
            if v.get("_id").unwrap().as_str().unwrap() == "25" {
                v.as_object_mut().unwrap().insert("val".into(), json!(999));
            }
        }
        let diff = compute_diff(&old, &new);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.updated.len(), 1);
        assert_eq!(diff.removed[0], "50");
        assert_eq!(diff.updated[0].entity_id, "25");
    }
}
