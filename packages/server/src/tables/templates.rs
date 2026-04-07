//! Built-in table templates for quick-start table creation.
//!
//! Templates define a set of fields, their types, and optional sample
//! data. Calling [`create_from_template`] creates the table config,
//! its fields as triples, and optionally populates sample records.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;

use super::{FieldId, PgTableStore, TableConfig, TableStore};
use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

// ── Template types ────────────────────────────────────────────────────

/// Describes a field within a table template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldTemplate {
    /// Human-readable field name.
    pub name: String,
    /// Field type: "text", "number", "select", "date", "email", "url", "multiline".
    pub field_type: String,
    /// Type-specific options (e.g., select choices).
    #[serde(default)]
    pub options: HashMap<String, serde_json::Value>,
}

/// A complete table template with fields and optional sample data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableTemplate {
    /// Template identifier (e.g. "project_tracker").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Ordered list of field definitions.
    pub fields: Vec<FieldTemplate>,
    /// Optional sample rows. Each entry maps field name to value.
    #[serde(default)]
    pub sample_data: Vec<HashMap<String, serde_json::Value>>,
}

// ── Built-in templates ────────────────────────────────────────────────

/// Return the full registry of built-in templates.
pub fn builtin_templates() -> Vec<TableTemplate> {
    vec![
        project_tracker(),
        contacts(),
        inventory(),
        content_calendar(),
        bug_tracker(),
    ]
}

/// Find a built-in template by name (case-insensitive).
pub fn get_template(name: &str) -> Option<TableTemplate> {
    let lower = name.to_lowercase();
    builtin_templates()
        .into_iter()
        .find(|t| t.name.to_lowercase() == lower)
}

fn project_tracker() -> TableTemplate {
    TableTemplate {
        name: "project_tracker".to_string(),
        description: "Track projects with status, priority, and assignments.".to_string(),
        fields: vec![
            FieldTemplate {
                name: "Name".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Status".into(),
                field_type: "select".into(),
                options: HashMap::from([(
                    "choices".into(),
                    serde_json::json!(["Todo", "In Progress", "Done"]),
                )]),
            },
            FieldTemplate {
                name: "Priority".into(),
                field_type: "select".into(),
                options: HashMap::from([(
                    "choices".into(),
                    serde_json::json!(["Low", "Medium", "High", "Urgent"]),
                )]),
            },
            FieldTemplate {
                name: "Assignee".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Due Date".into(),
                field_type: "date".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Description".into(),
                field_type: "multiline".into(),
                options: HashMap::new(),
            },
        ],
        sample_data: vec![
            HashMap::from([
                ("Name".into(), serde_json::json!("Design landing page")),
                ("Status".into(), serde_json::json!("In Progress")),
                ("Priority".into(), serde_json::json!("High")),
                ("Assignee".into(), serde_json::json!("Alice")),
            ]),
            HashMap::from([
                ("Name".into(), serde_json::json!("Set up CI/CD")),
                ("Status".into(), serde_json::json!("Todo")),
                ("Priority".into(), serde_json::json!("Medium")),
            ]),
        ],
    }
}

fn contacts() -> TableTemplate {
    TableTemplate {
        name: "contacts".to_string(),
        description: "Manage contacts with emails, phones, and company info.".to_string(),
        fields: vec![
            FieldTemplate {
                name: "Name".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Email".into(),
                field_type: "email".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Phone".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Company".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Tags".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Notes".into(),
                field_type: "multiline".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Last Contact".into(),
                field_type: "date".into(),
                options: HashMap::new(),
            },
        ],
        sample_data: vec![
            HashMap::from([
                ("Name".into(), serde_json::json!("Jane Doe")),
                ("Email".into(), serde_json::json!("jane@example.com")),
                ("Company".into(), serde_json::json!("Acme Corp")),
            ]),
        ],
    }
}

fn inventory() -> TableTemplate {
    TableTemplate {
        name: "inventory".to_string(),
        description: "Track products, SKUs, quantities, and suppliers.".to_string(),
        fields: vec![
            FieldTemplate {
                name: "Product Name".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "SKU".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Category".into(),
                field_type: "select".into(),
                options: HashMap::from([(
                    "choices".into(),
                    serde_json::json!(["Electronics", "Clothing", "Food", "Other"]),
                )]),
            },
            FieldTemplate {
                name: "Quantity".into(),
                field_type: "number".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Price".into(),
                field_type: "number".into(),
                options: HashMap::from([
                    ("precision".into(), serde_json::json!(2)),
                    ("prefix".into(), serde_json::json!("$")),
                ]),
            },
            FieldTemplate {
                name: "Supplier".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Reorder Level".into(),
                field_type: "number".into(),
                options: HashMap::new(),
            },
        ],
        sample_data: vec![
            HashMap::from([
                ("Product Name".into(), serde_json::json!("Widget A")),
                ("SKU".into(), serde_json::json!("WGT-001")),
                ("Category".into(), serde_json::json!("Electronics")),
                ("Quantity".into(), serde_json::json!(150)),
                ("Price".into(), serde_json::json!(29.99)),
                ("Reorder Level".into(), serde_json::json!(25)),
            ]),
        ],
    }
}

fn content_calendar() -> TableTemplate {
    TableTemplate {
        name: "content_calendar".to_string(),
        description: "Plan and schedule content publication.".to_string(),
        fields: vec![
            FieldTemplate {
                name: "Title".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Status".into(),
                field_type: "select".into(),
                options: HashMap::from([(
                    "choices".into(),
                    serde_json::json!(["Draft", "Review", "Scheduled", "Published"]),
                )]),
            },
            FieldTemplate {
                name: "Author".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Publish Date".into(),
                field_type: "date".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Category".into(),
                field_type: "select".into(),
                options: HashMap::from([(
                    "choices".into(),
                    serde_json::json!(["Blog", "Social", "Newsletter", "Video"]),
                )]),
            },
            FieldTemplate {
                name: "URL".into(),
                field_type: "url".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Notes".into(),
                field_type: "multiline".into(),
                options: HashMap::new(),
            },
        ],
        sample_data: vec![],
    }
}

fn bug_tracker() -> TableTemplate {
    TableTemplate {
        name: "bug_tracker".to_string(),
        description: "Track bugs with severity, status, and reproduction steps.".to_string(),
        fields: vec![
            FieldTemplate {
                name: "Title".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Severity".into(),
                field_type: "select".into(),
                options: HashMap::from([(
                    "choices".into(),
                    serde_json::json!(["Critical", "High", "Medium", "Low"]),
                )]),
            },
            FieldTemplate {
                name: "Status".into(),
                field_type: "select".into(),
                options: HashMap::from([(
                    "choices".into(),
                    serde_json::json!(["Open", "In Progress", "Fixed", "Closed", "Wont Fix"]),
                )]),
            },
            FieldTemplate {
                name: "Reporter".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Assignee".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Steps to Reproduce".into(),
                field_type: "multiline".into(),
                options: HashMap::new(),
            },
            FieldTemplate {
                name: "Environment".into(),
                field_type: "text".into(),
                options: HashMap::new(),
            },
        ],
        sample_data: vec![
            HashMap::from([
                ("Title".into(), serde_json::json!("Login button unresponsive on mobile")),
                ("Severity".into(), serde_json::json!("High")),
                ("Status".into(), serde_json::json!("Open")),
                ("Reporter".into(), serde_json::json!("QA Team")),
                ("Environment".into(), serde_json::json!("iOS Safari 17")),
            ]),
        ],
    }
}

// ── Template creation ─────────────────────────────────────────────────

/// Create a new table from a built-in template.
///
/// This creates:
/// 1. The table config with generated field ids
/// 2. Field metadata triples (field/{uuid}/name, field/{uuid}/type, etc.)
/// 3. Sample data records (if the template includes them)
///
/// Returns the finalized [`TableConfig`].
pub async fn create_from_template(
    pool: &PgPool,
    table_store: &PgTableStore,
    template_name: &str,
    table_name: &str,
) -> Result<TableConfig> {
    let template = get_template(template_name).ok_or_else(|| {
        DarshJError::InvalidQuery(format!(
            "Unknown template '{template_name}'. Available: {}",
            builtin_templates()
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;

    let mut config = TableConfig::new(table_name);
    config.description = Some(template.description.clone());

    let triple_store = PgTripleStore::new_lazy(pool.clone());

    // Create field metadata and collect field ids.
    let mut field_name_to_id: HashMap<String, FieldId> = HashMap::new();

    for (idx, ft) in template.fields.iter().enumerate() {
        let field_id = FieldId::new();
        config.field_ids.push(field_id);
        field_name_to_id.insert(ft.name.clone(), field_id);

        // Set the first field as the primary field.
        if idx == 0 {
            config.primary_field = Some(field_id);
        }

        // Store field metadata as triples on the table entity.
        let field_entity_id = field_id.0;
        let options_json = serde_json::to_value(&ft.options)?;

        let field_triples = vec![
            TripleInput {
                entity_id: field_entity_id,
                attribute: ":db/type".to_string(),
                value: serde_json::Value::String("__field".to_string()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: field_entity_id,
                attribute: "field/table_id".to_string(),
                value: serde_json::json!(config.id.0.to_string()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: field_entity_id,
                attribute: "field/name".to_string(),
                value: serde_json::Value::String(ft.name.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: field_entity_id,
                attribute: "field/type".to_string(),
                value: serde_json::Value::String(ft.field_type.clone()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: field_entity_id,
                attribute: "field/options".to_string(),
                value: options_json,
                value_type: 6, // Json
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: field_entity_id,
                attribute: "field/order".to_string(),
                value: serde_json::json!(idx as i64),
                value_type: 1, // Integer
                ttl_seconds: None,
            },
        ];

        triple_store.set_triples(&field_triples).await?;
    }

    // Persist the table config itself.
    table_store.create_table(&config).await?;

    // Insert sample data records.
    for row in &template.sample_data {
        let record_id = uuid::Uuid::new_v4();
        let mut record_triples = vec![TripleInput {
            entity_id: record_id,
            attribute: ":db/type".to_string(),
            value: serde_json::Value::String(config.slug.clone()),
            value_type: 0,
            ttl_seconds: None,
        }];

        for (field_name, value) in row {
            let vtype = infer_triple_value_type(value);
            record_triples.push(TripleInput {
                entity_id: record_id,
                attribute: format!("{}/{}", config.slug, super::slugify(field_name)),
                value: value.clone(),
                value_type: vtype,
                ttl_seconds: None,
            });
        }

        if !record_triples.is_empty() {
            triple_store.set_triples(&record_triples).await?;
        }
    }

    Ok(config)
}

/// Infer the triple store value type tag from a JSON value.
fn infer_triple_value_type(value: &serde_json::Value) -> i16 {
    match value {
        serde_json::Value::String(_) => 0,
        serde_json::Value::Number(n) => {
            if n.is_f64() && n.as_i64().is_none() {
                2 // Float
            } else {
                1 // Integer
            }
        }
        serde_json::Value::Bool(_) => 3,
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => 6, // Json
        serde_json::Value::Null => 0, // Store null as empty string
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_templates_not_empty() {
        let templates = builtin_templates();
        assert!(templates.len() >= 5);
    }

    #[test]
    fn all_templates_have_fields() {
        for t in builtin_templates() {
            assert!(
                !t.fields.is_empty(),
                "Template '{}' has no fields",
                t.name
            );
        }
    }

    #[test]
    fn all_templates_have_names() {
        for t in builtin_templates() {
            assert!(!t.name.is_empty());
            assert!(!t.description.is_empty());
        }
    }

    #[test]
    fn get_template_by_name() {
        let t = get_template("project_tracker").unwrap();
        assert_eq!(t.name, "project_tracker");
        assert!(t.fields.len() >= 6);
    }

    #[test]
    fn get_template_case_insensitive() {
        let t = get_template("PROJECT_TRACKER").unwrap();
        assert_eq!(t.name, "project_tracker");
    }

    #[test]
    fn get_template_unknown_returns_none() {
        assert!(get_template("nonexistent").is_none());
    }

    #[test]
    fn project_tracker_fields() {
        let t = get_template("project_tracker").unwrap();
        let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Name"));
        assert!(names.contains(&"Status"));
        assert!(names.contains(&"Priority"));
        assert!(names.contains(&"Assignee"));
        assert!(names.contains(&"Due Date"));
        assert!(names.contains(&"Description"));
    }

    #[test]
    fn contacts_fields() {
        let t = get_template("contacts").unwrap();
        let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Name"));
        assert!(names.contains(&"Email"));
        assert!(names.contains(&"Phone"));
        assert!(names.contains(&"Company"));
        assert!(names.contains(&"Tags"));
        assert!(names.contains(&"Notes"));
        assert!(names.contains(&"Last Contact"));
    }

    #[test]
    fn inventory_fields() {
        let t = get_template("inventory").unwrap();
        let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Product Name"));
        assert!(names.contains(&"SKU"));
        assert!(names.contains(&"Quantity"));
        assert!(names.contains(&"Price"));
        assert!(names.contains(&"Reorder Level"));
    }

    #[test]
    fn content_calendar_fields() {
        let t = get_template("content_calendar").unwrap();
        let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Title"));
        assert!(names.contains(&"Status"));
        assert!(names.contains(&"Author"));
        assert!(names.contains(&"Publish Date"));
        assert!(names.contains(&"URL"));
    }

    #[test]
    fn bug_tracker_fields() {
        let t = get_template("bug_tracker").unwrap();
        let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"Title"));
        assert!(names.contains(&"Severity"));
        assert!(names.contains(&"Status"));
        assert!(names.contains(&"Reporter"));
        assert!(names.contains(&"Steps to Reproduce"));
        assert!(names.contains(&"Environment"));
    }

    #[test]
    fn project_tracker_has_sample_data() {
        let t = get_template("project_tracker").unwrap();
        assert!(!t.sample_data.is_empty());
        let first = &t.sample_data[0];
        assert!(first.contains_key("Name"));
        assert!(first.contains_key("Status"));
    }

    #[test]
    fn select_fields_have_choices() {
        for t in builtin_templates() {
            for f in &t.fields {
                if f.field_type == "select" {
                    assert!(
                        f.options.contains_key("choices"),
                        "Select field '{}' in template '{}' missing choices",
                        f.name,
                        t.name
                    );
                    let choices = &f.options["choices"];
                    assert!(choices.is_array());
                    assert!(
                        !choices.as_array().unwrap().is_empty(),
                        "Select field '{}' has empty choices",
                        f.name
                    );
                }
            }
        }
    }

    #[test]
    fn field_template_serialization() {
        let ft = FieldTemplate {
            name: "Status".into(),
            field_type: "select".into(),
            options: HashMap::from([(
                "choices".into(),
                serde_json::json!(["A", "B"]),
            )]),
        };
        let json = serde_json::to_value(&ft).unwrap();
        let back: FieldTemplate = serde_json::from_value(json).unwrap();
        assert_eq!(ft.name, back.name);
        assert_eq!(ft.field_type, back.field_type);
    }

    #[test]
    fn infer_value_type_string() {
        assert_eq!(infer_triple_value_type(&serde_json::json!("hello")), 0);
    }

    #[test]
    fn infer_value_type_integer() {
        assert_eq!(infer_triple_value_type(&serde_json::json!(42)), 1);
    }

    #[test]
    fn infer_value_type_float() {
        assert_eq!(infer_triple_value_type(&serde_json::json!(3.14)), 2);
    }

    #[test]
    fn infer_value_type_bool() {
        assert_eq!(infer_triple_value_type(&serde_json::json!(true)), 3);
    }

    #[test]
    fn infer_value_type_json_object() {
        assert_eq!(
            infer_triple_value_type(&serde_json::json!({"key": "val"})),
            6
        );
    }

    #[test]
    fn infer_value_type_json_array() {
        assert_eq!(
            infer_triple_value_type(&serde_json::json!([1, 2, 3])),
            6
        );
    }

    #[test]
    fn template_serialization_roundtrip() {
        let t = project_tracker();
        let json = serde_json::to_value(&t).unwrap();
        let back: TableTemplate = serde_json::from_value(json).unwrap();
        assert_eq!(t.name, back.name);
        assert_eq!(t.fields.len(), back.fields.len());
        assert_eq!(t.sample_data.len(), back.sample_data.len());
    }
}
