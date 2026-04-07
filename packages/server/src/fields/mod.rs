//! Typed field system for DarshJDB.
//!
//! Fields define the schema for entity types, providing validation,
//! type conversion, and smart casting -- similar to Teable's field
//! definitions but stored as EAV triples in the triple store.
//!
//! Each field is persisted as an entity with type `field:{uuid}` and
//! attributes:
//!
//! - `field/name` -- human-readable field name
//! - `field/type` -- [`FieldType`] discriminator
//! - `field/table` -- the entity type this field belongs to
//! - `field/config` -- JSON-encoded [`FieldOptions`]
//! - `field/order` -- display ordering (i32)

pub mod conversion;
pub mod handlers;
pub mod validation;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── FieldId ────────────────────────────────────────────────────────

/// Strongly-typed wrapper around a UUID identifying a field definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FieldId(pub Uuid);

impl FieldId {
    /// Generate a new random field id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Build the entity id string used in the triple store (`field:{uuid}`).
    pub fn entity_key(&self) -> String {
        format!("field:{}", self.0)
    }
}

impl Default for FieldId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FieldId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ── FieldType ──────────────────────────────────────────────────────

/// All supported field types.
///
/// Each variant maps to a specific value type (or combination) in the
/// underlying triple store and carries its own validation rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    SingleLineText,
    LongText,
    Number,
    Checkbox,
    Date,
    DateTime,
    Email,
    Url,
    Phone,
    Currency,
    Percent,
    Duration,
    Rating,
    SingleSelect,
    MultiSelect,
    Attachment,
    Link,
    Lookup,
    Rollup,
    Formula,
    AutoNumber,
    CreatedTime,
    LastModifiedTime,
    CreatedBy,
    LastModifiedBy,
}

impl FieldType {
    /// Human-readable label for error messages.
    pub fn label(self) -> &'static str {
        match self {
            Self::SingleLineText => "single_line_text",
            Self::LongText => "long_text",
            Self::Number => "number",
            Self::Checkbox => "checkbox",
            Self::Date => "date",
            Self::DateTime => "date_time",
            Self::Email => "email",
            Self::Url => "url",
            Self::Phone => "phone",
            Self::Currency => "currency",
            Self::Percent => "percent",
            Self::Duration => "duration",
            Self::Rating => "rating",
            Self::SingleSelect => "single_select",
            Self::MultiSelect => "multi_select",
            Self::Attachment => "attachment",
            Self::Link => "link",
            Self::Lookup => "lookup",
            Self::Rollup => "rollup",
            Self::Formula => "formula",
            Self::AutoNumber => "auto_number",
            Self::CreatedTime => "created_time",
            Self::LastModifiedTime => "last_modified_time",
            Self::CreatedBy => "created_by",
            Self::LastModifiedBy => "last_modified_by",
        }
    }

    /// Whether this field type is computed (read-only, system-managed).
    pub fn is_computed(self) -> bool {
        matches!(
            self,
            Self::AutoNumber
                | Self::CreatedTime
                | Self::LastModifiedTime
                | Self::CreatedBy
                | Self::LastModifiedBy
                | Self::Lookup
                | Self::Rollup
                | Self::Formula
        )
    }
}

impl std::fmt::Display for FieldType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

// ── SelectChoice ───────────────────────────────────────────────────

/// A single option in a select (dropdown) field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectChoice {
    /// Unique id for this choice.
    pub id: String,
    /// Display name.
    pub name: String,
    /// CSS-compatible color string (hex, named, etc.).
    pub color: String,
}

// ── RollupFn ───────────────────────────────────────────────────────

/// Aggregation functions available for rollup fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollupFn {
    Count,
    Sum,
    Average,
    Min,
    Max,
    CountAll,
    CountValues,
    CountEmpty,
    ArrayJoin,
}

// ── FieldOptions ───────────────────────────────────────────────────

/// Type-specific configuration for a field.
///
/// Each variant carries the options relevant to its field type. Field
/// types without special configuration use `None` in [`FieldConfig`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldOptions {
    /// Options for [`FieldType::Number`].
    Number {
        /// Decimal precision (0 = integer).
        precision: u8,
        /// Display format (e.g. `"decimal"`, `"integer"`).
        format: String,
    },
    /// Options for [`FieldType::SingleSelect`] and [`FieldType::MultiSelect`].
    Select {
        /// Available choices.
        choices: Vec<SelectChoice>,
    },
    /// Options for [`FieldType::Link`].
    Link {
        /// Entity type of the linked table.
        linked_table: String,
        /// Whether to create a symmetric back-link.
        symmetric: bool,
    },
    /// Options for [`FieldType::Lookup`].
    Lookup {
        /// The link field to traverse.
        link_field: FieldId,
        /// The field to read from the linked entity.
        lookup_field: FieldId,
    },
    /// Options for [`FieldType::Rollup`].
    Rollup {
        /// The link field to traverse.
        link_field: FieldId,
        /// The field to aggregate from linked entities.
        rollup_field: FieldId,
        /// Aggregation function.
        function: RollupFn,
    },
    /// Options for [`FieldType::Formula`].
    Formula {
        /// Formula expression string.
        expression: String,
    },
    /// Options for [`FieldType::Currency`].
    Currency {
        /// Currency symbol (e.g. `"$"`, `"EUR"`).
        symbol: String,
        /// Decimal precision.
        precision: u8,
    },
    /// Options for [`FieldType::Rating`].
    Rating {
        /// Maximum rating value.
        max: u8,
        /// Icon identifier (e.g. `"star"`, `"heart"`).
        icon: String,
    },
}

// ── FieldConfig ────────────────────────────────────────────────────

/// Complete definition of a typed field.
///
/// Stored as EAV triples under entity `field:{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldConfig {
    /// Unique field identifier.
    pub id: FieldId,
    /// Human-readable field name.
    pub name: String,
    /// The field's type.
    pub field_type: FieldType,
    /// The entity type (table) this field belongs to.
    pub table_entity_type: String,
    /// Optional description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether a value is required for this field.
    #[serde(default)]
    pub required: bool,
    /// Whether values must be unique across entities.
    #[serde(default)]
    pub unique: bool,
    /// Default value when none is provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
    /// Type-specific options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<FieldOptions>,
    /// Display order within the table.
    #[serde(default)]
    pub order: i32,
}

impl FieldConfig {
    /// Triple-store entity id for this field.
    pub fn entity_id(&self) -> Uuid {
        self.id.0
    }

    /// Validate that the options match the field type.
    pub fn validate_options(&self) -> crate::error::Result<()> {
        if let Some(ref opts) = self.options {
            let valid = match (&self.field_type, opts) {
                (FieldType::Number, FieldOptions::Number { .. }) => true,
                (FieldType::SingleSelect, FieldOptions::Select { .. }) => true,
                (FieldType::MultiSelect, FieldOptions::Select { .. }) => true,
                (FieldType::Link, FieldOptions::Link { .. }) => true,
                (FieldType::Lookup, FieldOptions::Lookup { .. }) => true,
                (FieldType::Rollup, FieldOptions::Rollup { .. }) => true,
                (FieldType::Formula, FieldOptions::Formula { .. }) => true,
                (FieldType::Currency, FieldOptions::Currency { .. }) => true,
                (FieldType::Rating, FieldOptions::Rating { .. }) => true,
                _ => false,
            };
            if !valid {
                return Err(crate::error::DarshJError::InvalidAttribute(format!(
                    "options kind does not match field type '{}'",
                    self.field_type
                )));
            }
        }
        Ok(())
    }
}

// ── Triple-store attribute constants ───────────────────────────────

/// Attribute name for the field's human-readable name.
pub const ATTR_FIELD_NAME: &str = "field/name";
/// Attribute name for the field type discriminator.
pub const ATTR_FIELD_TYPE: &str = "field/type";
/// Attribute name for the table (entity type) the field belongs to.
pub const ATTR_FIELD_TABLE: &str = "field/table";
/// Attribute name for the JSON-encoded field config.
pub const ATTR_FIELD_CONFIG: &str = "field/config";
/// Attribute name for display order.
pub const ATTR_FIELD_ORDER: &str = "field/order";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_id_entity_key_format() {
        let id = FieldId(Uuid::nil());
        assert_eq!(
            id.entity_key(),
            "field:00000000-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn field_id_default_is_random() {
        let a = FieldId::default();
        let b = FieldId::default();
        assert_ne!(a, b);
    }

    #[test]
    fn field_type_label_roundtrip() {
        let types = [
            FieldType::SingleLineText,
            FieldType::LongText,
            FieldType::Number,
            FieldType::Checkbox,
            FieldType::Date,
            FieldType::DateTime,
            FieldType::Email,
            FieldType::Url,
            FieldType::Phone,
            FieldType::Currency,
            FieldType::Percent,
            FieldType::Duration,
            FieldType::Rating,
            FieldType::SingleSelect,
            FieldType::MultiSelect,
            FieldType::Attachment,
            FieldType::Link,
            FieldType::Lookup,
            FieldType::Rollup,
            FieldType::Formula,
            FieldType::AutoNumber,
            FieldType::CreatedTime,
            FieldType::LastModifiedTime,
            FieldType::CreatedBy,
            FieldType::LastModifiedBy,
        ];
        for ft in types {
            let label = ft.label();
            assert!(!label.is_empty());
            assert_eq!(label, label.to_lowercase());
        }
    }

    #[test]
    fn field_type_serialization_roundtrip() {
        let ft = FieldType::MultiSelect;
        let json = serde_json::to_string(&ft).unwrap();
        let back: FieldType = serde_json::from_str(&json).unwrap();
        assert_eq!(ft, back);
    }

    #[test]
    fn computed_fields_are_correct() {
        assert!(FieldType::AutoNumber.is_computed());
        assert!(FieldType::CreatedTime.is_computed());
        assert!(FieldType::LastModifiedTime.is_computed());
        assert!(FieldType::CreatedBy.is_computed());
        assert!(FieldType::LastModifiedBy.is_computed());
        assert!(FieldType::Lookup.is_computed());
        assert!(FieldType::Rollup.is_computed());
        assert!(FieldType::Formula.is_computed());

        assert!(!FieldType::SingleLineText.is_computed());
        assert!(!FieldType::Number.is_computed());
        assert!(!FieldType::Checkbox.is_computed());
    }

    #[test]
    fn field_config_validate_matching_options() {
        let config = FieldConfig {
            id: FieldId::new(),
            name: "Price".into(),
            field_type: FieldType::Number,
            table_entity_type: "product".into(),
            description: None,
            required: false,
            unique: false,
            default_value: None,
            options: Some(FieldOptions::Number {
                precision: 2,
                format: "decimal".into(),
            }),
            order: 0,
        };
        assert!(config.validate_options().is_ok());
    }

    #[test]
    fn field_config_validate_mismatched_options() {
        let config = FieldConfig {
            id: FieldId::new(),
            name: "Price".into(),
            field_type: FieldType::Number,
            table_entity_type: "product".into(),
            description: None,
            required: false,
            unique: false,
            default_value: None,
            options: Some(FieldOptions::Rating {
                max: 5,
                icon: "star".into(),
            }),
            order: 0,
        };
        assert!(config.validate_options().is_err());
    }

    #[test]
    fn field_config_no_options_always_valid() {
        let config = FieldConfig {
            id: FieldId::new(),
            name: "Name".into(),
            field_type: FieldType::SingleLineText,
            table_entity_type: "user".into(),
            description: None,
            required: true,
            unique: false,
            default_value: None,
            options: None,
            order: 0,
        };
        assert!(config.validate_options().is_ok());
    }

    #[test]
    fn select_choice_serialization() {
        let choice = SelectChoice {
            id: "opt1".into(),
            name: "Active".into(),
            color: "#22c55e".into(),
        };
        let json = serde_json::to_value(&choice).unwrap();
        assert_eq!(json["id"], "opt1");
        assert_eq!(json["name"], "Active");
        assert_eq!(json["color"], "#22c55e");
    }

    #[test]
    fn rollup_fn_serialization() {
        let fns = [
            (RollupFn::Count, "count"),
            (RollupFn::Sum, "sum"),
            (RollupFn::Average, "average"),
            (RollupFn::Min, "min"),
            (RollupFn::Max, "max"),
            (RollupFn::CountAll, "count_all"),
            (RollupFn::CountValues, "count_values"),
            (RollupFn::CountEmpty, "count_empty"),
            (RollupFn::ArrayJoin, "array_join"),
        ];
        for (rf, expected) in fns {
            let json = serde_json::to_value(rf).unwrap();
            assert_eq!(json, expected);
        }
    }

    #[test]
    fn field_options_tagged_serialization() {
        let opts = FieldOptions::Number {
            precision: 2,
            format: "decimal".into(),
        };
        let json = serde_json::to_value(&opts).unwrap();
        assert_eq!(json["kind"], "number");
        assert_eq!(json["precision"], 2);
        assert_eq!(json["format"], "decimal");
    }

    #[test]
    fn field_config_full_serialization_roundtrip() {
        let config = FieldConfig {
            id: FieldId::new(),
            name: "Status".into(),
            field_type: FieldType::SingleSelect,
            table_entity_type: "task".into(),
            description: Some("Task status".into()),
            required: true,
            unique: false,
            default_value: Some(serde_json::json!("todo")),
            options: Some(FieldOptions::Select {
                choices: vec![
                    SelectChoice {
                        id: "1".into(),
                        name: "Todo".into(),
                        color: "#gray".into(),
                    },
                    SelectChoice {
                        id: "2".into(),
                        name: "Done".into(),
                        color: "#green".into(),
                    },
                ],
            }),
            order: 1,
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: FieldConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, back);
    }
}
