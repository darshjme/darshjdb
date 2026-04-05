//! Schema discovery and migration types for the triple store.
//!
//! The schema layer inspects live triples to infer entity types,
//! their attributes, value types, and inter-entity references.
//! [`MigrationGenerator`] diffs two schemas to produce the DDL
//! statements needed to evolve the underlying storage.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

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

    /// Returns the maximum valid discriminator value.
    pub fn max_discriminator() -> i16 {
        6
    }
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl TryFrom<i16> for ValueType {
    type Error = i16;

    /// Convert a raw `i16` discriminator into `ValueType`.
    ///
    /// Returns `Err(raw_value)` if the tag is not a known discriminator.
    fn try_from(v: i16) -> std::result::Result<Self, Self::Error> {
        Self::from_i16(v).ok_or(v)
    }
}

/// Information about a single attribute within an entity type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReferenceInfo {
    /// The attribute name that holds the reference UUID.
    pub attribute: String,
    /// The inferred target entity type (best-effort).
    pub target_type: String,
    /// Number of entities carrying this reference.
    pub cardinality: u64,
}

/// A discovered entity type aggregated from triple data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    /// Map from type name to its discovered shape.
    pub entity_types: HashMap<String, EntityType>,
    /// Transaction id at which this snapshot was taken.
    pub as_of_tx: i64,
}

/// A single migration action produced by diffing two schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Actions are emitted in a deterministic, safe order:
    /// 1. New entity types (sorted by name) with their attributes
    /// 2. Attribute additions and alterations on existing types (sorted)
    /// 3. Attribute removals on existing types (sorted)
    /// 4. Removed entity types (sorted by name)
    ///
    /// This ensures downstream consumers can apply them sequentially
    /// without ordering surprises.
    pub fn diff(old: &Schema, new: &Schema) -> Vec<MigrationAction> {
        let mut actions = Vec::new();

        // 1. Detect added entity types (sorted for determinism).
        let mut new_type_names: Vec<&String> = new
            .entity_types
            .keys()
            .filter(|n| !old.entity_types.contains_key(*n))
            .collect();
        new_type_names.sort();

        for name in new_type_names {
            let new_et = &new.entity_types[name];
            actions.push(MigrationAction::AddEntityType { name: name.clone() });
            // Emit per-attribute additions sorted by attribute name.
            let mut attr_names: Vec<&String> = new_et.attributes.keys().collect();
            attr_names.sort();
            for attr in attr_names {
                let info = &new_et.attributes[attr];
                actions.push(MigrationAction::AddAttribute {
                    entity_type: name.clone(),
                    attribute: attr.clone(),
                    value_types: info.value_types.clone(),
                });
            }
        }

        // 2-3. Diff attributes on types that exist in both (sorted).
        let mut common_type_names: Vec<&String> = old
            .entity_types
            .keys()
            .filter(|n| new.entity_types.contains_key(*n))
            .collect();
        common_type_names.sort();

        for name in common_type_names {
            let old_et = &old.entity_types[name];
            let new_et = &new.entity_types[name];

            // Additions and alterations (sorted).
            let mut new_attr_names: Vec<&String> = new_et.attributes.keys().collect();
            new_attr_names.sort();
            for attr in new_attr_names {
                let new_info = &new_et.attributes[attr];
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

            // Removals (sorted).
            let mut removed_attrs: Vec<&String> = old_et
                .attributes
                .keys()
                .filter(|a| !new_et.attributes.contains_key(*a))
                .collect();
            removed_attrs.sort();
            for attr in removed_attrs {
                actions.push(MigrationAction::RemoveAttribute {
                    entity_type: name.clone(),
                    attribute: attr.clone(),
                });
            }
        }

        // 4. Detect removed entity types (sorted).
        let mut removed_type_names: Vec<&String> = old
            .entity_types
            .keys()
            .filter(|n| !new.entity_types.contains_key(*n))
            .collect();
        removed_type_names.sort();

        for name in removed_type_names {
            actions.push(MigrationAction::RemoveEntityType { name: name.clone() });
        }

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper factories ────────────────────────────────────────────

    fn attr(name: &str, vts: Vec<ValueType>, required: bool, card: u64) -> AttributeInfo {
        AttributeInfo {
            name: name.to_string(),
            value_types: vts,
            required,
            cardinality: card,
        }
    }

    fn entity_type(
        name: &str,
        attrs: Vec<(&str, Vec<ValueType>, bool, u64)>,
        count: u64,
    ) -> EntityType {
        let mut attributes = HashMap::new();
        for (aname, vts, req, card) in attrs {
            attributes.insert(aname.to_string(), attr(aname, vts, req, card));
        }
        EntityType {
            name: name.to_string(),
            attributes,
            references: vec![],
            entity_count: count,
        }
    }

    fn schema_with(types: Vec<EntityType>) -> Schema {
        let mut entity_types = HashMap::new();
        for et in types {
            entity_types.insert(et.name.clone(), et);
        }
        Schema {
            entity_types,
            as_of_tx: 0,
        }
    }

    // ── ValueType tests ─────────────────────────────────────────────

    #[test]
    fn value_type_round_trip_all_variants() {
        let all = [
            ValueType::String,
            ValueType::Integer,
            ValueType::Float,
            ValueType::Boolean,
            ValueType::Timestamp,
            ValueType::Reference,
            ValueType::Json,
        ];
        for v in all {
            let raw = v as i16;
            assert_eq!(ValueType::from_i16(raw), Some(v), "from_i16 failed for {v}");
            assert_eq!(ValueType::try_from(raw), Ok(v), "TryFrom failed for {v}");
        }
    }

    #[test]
    fn value_type_from_i16_invalid() {
        assert_eq!(ValueType::from_i16(-1), None);
        assert_eq!(ValueType::from_i16(7), None);
        assert_eq!(ValueType::from_i16(99), None);
        assert_eq!(ValueType::from_i16(i16::MAX), None);
        assert_eq!(ValueType::from_i16(i16::MIN), None);
    }

    #[test]
    fn value_type_try_from_invalid_returns_raw() {
        assert_eq!(ValueType::try_from(42_i16), Err(42));
        assert_eq!(ValueType::try_from(-1_i16), Err(-1));
    }

    #[test]
    fn value_type_display_matches_label() {
        let all = [
            ValueType::String,
            ValueType::Integer,
            ValueType::Float,
            ValueType::Boolean,
            ValueType::Timestamp,
            ValueType::Reference,
            ValueType::Json,
        ];
        for v in all {
            assert_eq!(format!("{v}"), v.label());
        }
    }

    #[test]
    fn value_type_labels_are_lowercase() {
        for i in 0..=ValueType::max_discriminator() {
            let vt = ValueType::from_i16(i).unwrap();
            let label = vt.label();
            assert_eq!(label, label.to_lowercase(), "label not lowercase: {label}");
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn value_type_discriminator_contiguous() {
        // All discriminators 0..=max should be valid
        for i in 0..=ValueType::max_discriminator() {
            assert!(ValueType::from_i16(i).is_some(), "gap at discriminator {i}");
        }
        // max+1 should be invalid
        assert!(ValueType::from_i16(ValueType::max_discriminator() + 1).is_none());
    }

    #[test]
    fn value_type_serialization_roundtrip() {
        let vt = ValueType::Reference;
        let json = serde_json::to_string(&vt).unwrap();
        let back: ValueType = serde_json::from_str(&json).unwrap();
        assert_eq!(vt, back);
    }

    // ── AttributeInfo tests ─────────────────────────────────────────

    #[test]
    fn attribute_info_equality() {
        let a1 = attr("email", vec![ValueType::String], true, 5);
        let a2 = attr("email", vec![ValueType::String], true, 5);
        assert_eq!(a1, a2);

        let a3 = attr("email", vec![ValueType::String], false, 5);
        assert_ne!(a1, a3); // required differs
    }

    // ── Schema default ──────────────────────────────────────────────

    #[test]
    fn schema_default_is_empty() {
        let s = Schema::default();
        assert!(s.entity_types.is_empty());
        assert_eq!(s.as_of_tx, 0);
    }

    // ── MigrationGenerator::diff ────────────────────────────────────

    #[test]
    fn diff_empty_to_empty() {
        let actions = MigrationGenerator::diff(&Schema::default(), &Schema::default());
        assert!(actions.is_empty());
    }

    #[test]
    fn diff_empty_to_new_type() {
        let old = Schema::default();
        let new = schema_with(vec![entity_type(
            "User",
            vec![("name", vec![ValueType::String], true, 10)],
            10,
        )]);

        let actions = MigrationGenerator::diff(&old, &new);
        assert_eq!(actions.len(), 2); // AddEntityType + AddAttribute
        assert_eq!(
            actions[0],
            MigrationAction::AddEntityType {
                name: "User".into()
            }
        );
        assert!(matches!(
            &actions[1],
            MigrationAction::AddAttribute { entity_type, attribute, .. }
            if entity_type == "User" && attribute == "name"
        ));
    }

    #[test]
    fn diff_remove_type() {
        let old = schema_with(vec![entity_type(
            "User",
            vec![("name", vec![ValueType::String], true, 5)],
            5,
        )]);
        let new = Schema::default();

        let actions = MigrationGenerator::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0],
            MigrationAction::RemoveEntityType {
                name: "User".into()
            }
        );
    }

    #[test]
    fn diff_add_attribute_to_existing_type() {
        let old = schema_with(vec![entity_type(
            "User",
            vec![("name", vec![ValueType::String], true, 5)],
            5,
        )]);
        let new = schema_with(vec![entity_type(
            "User",
            vec![
                ("name", vec![ValueType::String], true, 5),
                ("age", vec![ValueType::Integer], false, 3),
            ],
            5,
        )]);

        let actions = MigrationGenerator::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            MigrationAction::AddAttribute { entity_type, attribute, .. }
            if entity_type == "User" && attribute == "age"
        ));
    }

    #[test]
    fn diff_remove_attribute_from_existing_type() {
        let old = schema_with(vec![entity_type(
            "User",
            vec![
                ("name", vec![ValueType::String], true, 5),
                ("age", vec![ValueType::Integer], false, 3),
            ],
            5,
        )]);
        let new = schema_with(vec![entity_type(
            "User",
            vec![("name", vec![ValueType::String], true, 5)],
            5,
        )]);

        let actions = MigrationGenerator::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0],
            MigrationAction::RemoveAttribute {
                entity_type: "User".into(),
                attribute: "age".into(),
            }
        );
    }

    #[test]
    fn diff_alter_attribute_type_change() {
        let old = schema_with(vec![entity_type(
            "User",
            vec![("score", vec![ValueType::Integer], true, 5)],
            5,
        )]);
        let new = schema_with(vec![entity_type(
            "User",
            vec![("score", vec![ValueType::Float], true, 5)],
            5,
        )]);

        let actions = MigrationGenerator::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0],
            MigrationAction::AlterAttribute {
                entity_type: "User".into(),
                attribute: "score".into(),
                old_types: vec![ValueType::Integer],
                new_types: vec![ValueType::Float],
            }
        );
    }

    #[test]
    fn diff_attribute_type_widening() {
        // Attribute gains an additional type (polymorphic)
        let old = schema_with(vec![entity_type(
            "Doc",
            vec![("payload", vec![ValueType::String], true, 5)],
            5,
        )]);
        let new = schema_with(vec![entity_type(
            "Doc",
            vec![("payload", vec![ValueType::String, ValueType::Json], true, 5)],
            5,
        )]);

        let actions = MigrationGenerator::diff(&old, &new);
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            &actions[0],
            MigrationAction::AlterAttribute { .. }
        ));
    }

    #[test]
    fn diff_no_change_is_empty() {
        let s = schema_with(vec![entity_type(
            "User",
            vec![("name", vec![ValueType::String], true, 5)],
            5,
        )]);
        let actions = MigrationGenerator::diff(&s, &s.clone());
        assert!(actions.is_empty());
    }

    #[test]
    fn diff_multiple_types_add_and_remove() {
        let old = schema_with(vec![
            entity_type("A", vec![], 1),
            entity_type("B", vec![], 2),
        ]);
        let new = schema_with(vec![
            entity_type("B", vec![], 2),
            entity_type("C", vec![], 3),
        ]);

        let actions = MigrationGenerator::diff(&old, &new);
        // Should have: AddEntityType(C), RemoveEntityType(A)
        assert!(actions.iter().any(|a| matches!(
            a,
            MigrationAction::AddEntityType { name } if name == "C"
        )));
        assert!(actions.iter().any(|a| matches!(
            a,
            MigrationAction::RemoveEntityType { name } if name == "A"
        )));
        // Additions come before removals in the output
        let add_pos = actions
            .iter()
            .position(|a| matches!(a, MigrationAction::AddEntityType { .. }))
            .unwrap();
        let rem_pos = actions
            .iter()
            .position(|a| matches!(a, MigrationAction::RemoveEntityType { .. }))
            .unwrap();
        assert!(add_pos < rem_pos, "additions must precede removals");
    }

    #[test]
    fn diff_deterministic_ordering() {
        // Run diff multiple times with several types to ensure determinism
        let old = schema_with(vec![
            entity_type("Zebra", vec![("z", vec![ValueType::String], true, 1)], 1),
            entity_type("Apple", vec![("a", vec![ValueType::String], true, 1)], 1),
        ]);
        let new = schema_with(vec![
            entity_type("Mango", vec![("m", vec![ValueType::Integer], true, 1)], 1),
            entity_type("Banana", vec![("b", vec![ValueType::Boolean], true, 1)], 1),
        ]);

        let first = MigrationGenerator::diff(&old, &new);
        for _ in 0..20 {
            let again = MigrationGenerator::diff(&old, &new);
            assert_eq!(first, again, "diff output must be deterministic");
        }
    }

    #[test]
    fn diff_complex_scenario() {
        // Old: User(name, email), Post(title)
        // New: User(name, age), Post(title, body), Comment(text)
        // Expected: AddEntityType(Comment) + AddAttribute(Comment.text)
        //           AddAttribute(Post.body), AddAttribute(User.age)
        //           RemoveAttribute(User.email)
        let old = schema_with(vec![
            entity_type(
                "User",
                vec![
                    ("name", vec![ValueType::String], true, 5),
                    ("email", vec![ValueType::String], true, 5),
                ],
                5,
            ),
            entity_type("Post", vec![("title", vec![ValueType::String], true, 3)], 3),
        ]);
        let new = schema_with(vec![
            entity_type(
                "User",
                vec![
                    ("name", vec![ValueType::String], true, 5),
                    ("age", vec![ValueType::Integer], false, 3),
                ],
                5,
            ),
            entity_type(
                "Post",
                vec![
                    ("title", vec![ValueType::String], true, 3),
                    ("body", vec![ValueType::String], false, 2),
                ],
                3,
            ),
            entity_type(
                "Comment",
                vec![("text", vec![ValueType::String], true, 10)],
                10,
            ),
        ]);

        let actions = MigrationGenerator::diff(&old, &new);

        // Verify counts by action type
        let adds = actions
            .iter()
            .filter(|a| matches!(a, MigrationAction::AddEntityType { .. }))
            .count();
        let add_attrs = actions
            .iter()
            .filter(|a| matches!(a, MigrationAction::AddAttribute { .. }))
            .count();
        let rem_attrs = actions
            .iter()
            .filter(|a| matches!(a, MigrationAction::RemoveAttribute { .. }))
            .count();

        assert_eq!(adds, 1, "one new type: Comment");
        assert_eq!(add_attrs, 3, "Comment.text + Post.body + User.age");
        assert_eq!(rem_attrs, 1, "User.email removed");
    }

    // ── ReferenceInfo ───────────────────────────────────────────────

    #[test]
    fn reference_info_equality() {
        let r1 = ReferenceInfo {
            attribute: "author_id".into(),
            target_type: "User".into(),
            cardinality: 3,
        };
        let r2 = r1.clone();
        assert_eq!(r1, r2);
    }

    // ── EntityType ──────────────────────────────────────────────────

    #[test]
    fn entity_type_with_no_attributes() {
        let et = entity_type("Empty", vec![], 0);
        assert!(et.attributes.is_empty());
        assert_eq!(et.entity_count, 0);
    }

    // ── Schema equality ─────────────────────────────────────────────

    #[test]
    fn schema_equality() {
        let s1 = schema_with(vec![entity_type(
            "X",
            vec![("a", vec![ValueType::Boolean], true, 1)],
            1,
        )]);
        let s2 = s1.clone();
        assert_eq!(s1, s2);
    }

    #[test]
    fn schema_inequality_different_tx() {
        let mut s1 = Schema::default();
        let mut s2 = Schema::default();
        s1.as_of_tx = 10;
        s2.as_of_tx = 20;
        assert_ne!(s1, s2);
    }
}
