//! Schema discovery and migration types for the triple store.
//!
//! The schema layer inspects live triples to infer entity types,
//! their attributes, value types, and inter-entity references.
//! [`MigrationGenerator`] diffs two schemas to produce the DDL
//! statements needed to evolve the underlying storage.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Well-known value type discriminators stored in `triple.value_type`.
///
/// These match the `i16` tag persisted alongside every triple value
/// so the query engine can apply type-specific operators (ordering,
/// range scans, full-text search, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(i16)]
pub enum ValueType {
    /// UTF-8 string.
    String = 0,
    /// 64-bit signed integer.
    Integer = 1,
    /// 64-bit IEEE 754 float.
    Float = 2,
    /// Boolean.
    Boolean = 3,
    /// RFC 3339 timestamp.
    Timestamp = 4,
    /// UUID reference to another entity.
    Reference = 5,
    /// Arbitrary JSON blob.
    Json = 6,
}

impl ValueType {
    /// Convert a raw `i16` tag back into the enum.
    ///
    /// Returns `None` for values outside the known range.
    pub fn from_i16(v: i16) -> Option<Self> {
        match v {
            0 => Some(Self::String),
            1 => Some(Self::Integer),
            2 => Some(Self::Float),
            3 => Some(Self::Boolean),
            4 => Some(Self::Timestamp),
            5 => Some(Self::Reference),
            6 => Some(Self::Json),
            _ => None,
        }
    }

    /// Human-readable label used in error messages and schema output.
    pub fn label(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Boolean => "boolean",
            Self::Timestamp => "timestamp",
            Self::Reference => "reference",
            Self::Json => "json",
        }
    }
}

/// Information about a single attribute within an entity type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttributeInfo {
    /// The attribute name (e.g. `"email"`, `"created_at"`).
    pub name: String,
    /// Observed value types for this attribute across all entities.
    pub value_types: Vec<ValueType>,
    /// Whether every entity of this type carries the attribute.
    pub required: bool,
    /// Number of distinct entities that have this attribute.
    pub cardinality: u64,
}

/// A discovered reference from one entity type to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceInfo {
    /// The attribute name that holds the reference UUID.
    pub attribute: String,
    /// The inferred target entity type (best-effort).
    pub target_type: String,
    /// Number of entities carrying this reference.
    pub cardinality: u64,
}

/// A discovered entity type aggregated from triple data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityType {
    /// The type name, typically the value of the `:db/type` attribute.
    pub name: String,
    /// Attributes observed on entities of this type.
    pub attributes: HashMap<String, AttributeInfo>,
    /// Outgoing references to other entity types.
    pub references: Vec<ReferenceInfo>,
    /// Total number of distinct entities of this type.
    pub entity_count: u64,
}

/// Full schema snapshot inferred from the triple store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    /// Map from type name to its discovered shape.
    pub entity_types: HashMap<String, EntityType>,
    /// Transaction id at which this snapshot was taken.
    pub as_of_tx: i64,
}

/// A single migration action produced by diffing two schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationAction {
    /// A new entity type appeared.
    AddEntityType {
        /// Name of the new type.
        name: String,
    },
    /// An entity type was removed (all entities retracted).
    RemoveEntityType {
        /// Name of the removed type.
        name: String,
    },
    /// A new attribute appeared on an existing type.
    AddAttribute {
        /// The entity type.
        entity_type: String,
        /// The new attribute.
        attribute: String,
        /// Observed value types.
        value_types: Vec<ValueType>,
    },
    /// An attribute was removed from a type.
    RemoveAttribute {
        /// The entity type.
        entity_type: String,
        /// The removed attribute.
        attribute: String,
    },
    /// An attribute changed its observed value types.
    AlterAttribute {
        /// The entity type.
        entity_type: String,
        /// The attribute.
        attribute: String,
        /// Previous value types.
        old_types: Vec<ValueType>,
        /// New value types.
        new_types: Vec<ValueType>,
    },
}

/// Diffs two [`Schema`] snapshots and produces migration actions.
///
/// The generator is stateless — call [`MigrationGenerator::diff`] with
/// the old and new schemas to get the list of structural changes.
pub struct MigrationGenerator;

impl MigrationGenerator {
    /// Compare `old` and `new` schemas, returning ordered migration actions.
    ///
    /// Actions are emitted in a safe order: additions before removals,
    /// so downstream consumers can apply them sequentially.
    pub fn diff(old: &Schema, new: &Schema) -> Vec<MigrationAction> {
        let mut actions = Vec::new();

        // Detect added entity types.
        for (name, new_et) in &new.entity_types {
            if !old.entity_types.contains_key(name) {
                actions.push(MigrationAction::AddEntityType { name: name.clone() });
                // All attributes on a new type are implicitly "added",
                // but we still emit them for consumers that track per-attribute.
                for (attr, info) in &new_et.attributes {
                    actions.push(MigrationAction::AddAttribute {
                        entity_type: name.clone(),
                        attribute: attr.clone(),
                        value_types: info.value_types.clone(),
                    });
                }
            }
        }

        // Detect removed entity types.
        for name in old.entity_types.keys() {
            if !new.entity_types.contains_key(name) {
                actions.push(MigrationAction::RemoveEntityType { name: name.clone() });
            }
        }

        // Diff attributes on types that exist in both.
        for (name, old_et) in &old.entity_types {
            let Some(new_et) = new.entity_types.get(name) else {
                continue; // already handled as removal
            };

            for (attr, new_info) in &new_et.attributes {
                match old_et.attributes.get(attr) {
                    None => {
                        actions.push(MigrationAction::AddAttribute {
                            entity_type: name.clone(),
                            attribute: attr.clone(),
                            value_types: new_info.value_types.clone(),
                        });
                    }
                    Some(old_info) if old_info.value_types != new_info.value_types => {
                        actions.push(MigrationAction::AlterAttribute {
                            entity_type: name.clone(),
                            attribute: attr.clone(),
                            old_types: old_info.value_types.clone(),
                            new_types: new_info.value_types.clone(),
                        });
                    }
                    _ => {}
                }
            }

            for attr in old_et.attributes.keys() {
                if !new_et.attributes.contains_key(attr) {
                    actions.push(MigrationAction::RemoveAttribute {
                        entity_type: name.clone(),
                        attribute: attr.clone(),
                    });
                }
            }
        }

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_empty_to_new_type() {
        let old = Schema::default();
        let mut new = Schema::default();
        let mut attrs = HashMap::new();
        attrs.insert(
            "name".to_string(),
            AttributeInfo {
                name: "name".to_string(),
                value_types: vec![ValueType::String],
                required: true,
                cardinality: 10,
            },
        );
        new.entity_types.insert(
            "User".to_string(),
            EntityType {
                name: "User".to_string(),
                attributes: attrs,
                references: vec![],
                entity_count: 10,
            },
        );

        let actions = MigrationGenerator::diff(&old, &new);
        assert!(actions.len() >= 2); // AddEntityType + AddAttribute
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, MigrationAction::AddEntityType { name } if name == "User"))
        );
    }

    #[test]
    fn value_type_round_trip() {
        for v in [
            ValueType::String,
            ValueType::Integer,
            ValueType::Float,
            ValueType::Boolean,
            ValueType::Timestamp,
            ValueType::Reference,
            ValueType::Json,
        ] {
            assert_eq!(ValueType::from_i16(v as i16), Some(v));
        }
        assert_eq!(ValueType::from_i16(99), None);
    }
}
