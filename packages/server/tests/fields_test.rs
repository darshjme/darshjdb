//! Integration tests for the DarshJDB typed field system.
//!
//! Tests field creation, validation, type conversion, and required-field
//! enforcement against a real Postgres triple store.
//!
//! ```sh
//! DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb_test \
//!     cargo test --test fields_test
//! ```

use ddb_server::fields::conversion::{convert_field_type, summarise};
use ddb_server::fields::validation::validate_value;
use ddb_server::fields::{FieldConfig, FieldId, FieldOptions, FieldType, SelectChoice};
use ddb_server::triple_store::{PgTripleStore, TripleInput, TripleStore};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup() -> Option<(PgPool, PgTripleStore)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    let store = PgTripleStore::new(pool.clone()).await.ok()?;
    Some((pool, store))
}

async fn cleanup_entities(pool: &PgPool, ids: &[Uuid]) {
    if ids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM triples WHERE entity_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await
        .ok();
}

fn make_field(
    name: &str,
    field_type: FieldType,
    required: bool,
    options: Option<FieldOptions>,
) -> FieldConfig {
    FieldConfig {
        id: FieldId::new(),
        name: name.into(),
        field_type,
        table_entity_type: "test_entity".into(),
        description: None,
        required,
        unique: false,
        default_value: None,
        options,
        order: 0,
    }
}

// ===========================================================================
// 1. CREATE FIELDS AND PERSIST AS TRIPLES
// ===========================================================================

#[tokio::test]
async fn test_field_create_and_persist() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let field = make_field(
        "price",
        FieldType::Number,
        true,
        Some(FieldOptions::Number {
            precision: 2,
            format: "decimal".into(),
        }),
    );

    // Validate options match field type.
    field.validate_options().expect("options should match");

    // Persist the field config as triples.
    let eid = field.entity_id();
    let config_json = serde_json::to_value(&field).expect("serialize field config");

    let tx = store
        .set_triples(&[
            TripleInput {
                entity_id: eid,
                attribute: "field/name".into(),
                value: json!(field.name),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "field/type".into(),
                value: json!(field.field_type.label()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "field/table".into(),
                value: json!(field.table_entity_type),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "field/config".into(),
                value: config_json,
                value_type: 6, // JSON
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "field/order".into(),
                value: json!(field.order),
                value_type: 1, // Integer
                ttl_seconds: None,
            },
        ])
        .await
        .expect("persist field");
    assert!(tx > 0);

    // Read it back.
    let triples = store.get_entity(eid).await.expect("get field entity");
    assert!(triples.len() >= 5);

    let name_triple = triples
        .iter()
        .find(|t| t.attribute == "field/name")
        .expect("should have name");
    assert_eq!(name_triple.value, json!("price"));

    let config_triple = triples
        .iter()
        .find(|t| t.attribute == "field/config")
        .expect("should have config");
    let restored: FieldConfig =
        serde_json::from_value(config_triple.value.clone()).expect("deserialize");
    assert_eq!(restored.field_type, FieldType::Number);
    assert!(restored.required);

    cleanup_entities(&pool, &[eid]).await;
}

// ===========================================================================
// 2. VALIDATE FIELD OPTIONS
// ===========================================================================

#[tokio::test]
async fn test_field_options_matching() {
    // This test does not require a database.
    let _: Option<()> = None; // Compiler hint.

    // Number field with Number options -> valid.
    let f = make_field(
        "amount",
        FieldType::Number,
        false,
        Some(FieldOptions::Number {
            precision: 0,
            format: "integer".into(),
        }),
    );
    assert!(f.validate_options().is_ok());

    // Number field with Rating options -> invalid.
    let f = make_field(
        "amount",
        FieldType::Number,
        false,
        Some(FieldOptions::Rating {
            max: 5,
            icon: "star".into(),
        }),
    );
    assert!(f.validate_options().is_err());

    // SingleSelect with Select options -> valid.
    let f = make_field(
        "status",
        FieldType::SingleSelect,
        false,
        Some(FieldOptions::Select {
            choices: vec![
                SelectChoice {
                    id: "1".into(),
                    name: "Open".into(),
                    color: "#blue".into(),
                },
                SelectChoice {
                    id: "2".into(),
                    name: "Closed".into(),
                    color: "#gray".into(),
                },
            ],
        }),
    );
    assert!(f.validate_options().is_ok());

    // Link with Link options -> valid.
    let f = make_field(
        "project",
        FieldType::Link,
        false,
        Some(FieldOptions::Link {
            linked_table: "projects".into(),
            symmetric: false,
        }),
    );
    assert!(f.validate_options().is_ok());

    // No options for any type -> always valid.
    let f = make_field("notes", FieldType::LongText, false, None);
    assert!(f.validate_options().is_ok());
}

// ===========================================================================
// 3. FIELD VALUE VALIDATION
// ===========================================================================

#[tokio::test]
async fn test_field_validation_text() {
    let field = make_field("title", FieldType::SingleLineText, false, None);

    // Valid string.
    let v = validate_value(&field, &json!("Hello")).expect("valid");
    assert_eq!(v, json!("Hello"));

    // Newline rejected in single-line text.
    let err = validate_value(&field, &json!("line1\nline2"));
    assert!(err.is_err());

    // Number coerced to string.
    let v = validate_value(&field, &json!(42)).expect("coerce");
    assert_eq!(v, json!("42"));

    // Null accepted when not required.
    let v = validate_value(&field, &json!(null)).expect("null ok");
    assert!(v.is_null());
}

#[tokio::test]
async fn test_field_validation_number() {
    let field = make_field(
        "score",
        FieldType::Number,
        false,
        Some(FieldOptions::Number {
            precision: 2,
            format: "decimal".into(),
        }),
    );

    // Direct number.
    let v = validate_value(&field, &json!(3.14)).expect("valid");
    assert!(v.as_f64().is_some());

    // String that parses as number.
    let v = validate_value(&field, &json!("42.5")).expect("coerce");
    assert!((v.as_f64().unwrap() - 42.5).abs() < f64::EPSILON);

    // Non-numeric string rejected.
    let err = validate_value(&field, &json!("not a number"));
    assert!(err.is_err());
}

#[tokio::test]
async fn test_field_validation_checkbox() {
    let field = make_field("done", FieldType::Checkbox, false, None);

    let v = validate_value(&field, &json!(true)).expect("valid");
    assert_eq!(v, json!(true));

    // String "true" coerced.
    let v = validate_value(&field, &json!("true")).expect("coerce");
    assert_eq!(v, json!(true));

    // Integer 0/1 coerced.
    let v = validate_value(&field, &json!(1)).expect("coerce 1");
    assert_eq!(v, json!(true));
}

#[tokio::test]
async fn test_field_validation_email() {
    let field = make_field("email", FieldType::Email, false, None);

    let v = validate_value(&field, &json!("user@example.com")).expect("valid");
    assert_eq!(v, json!("user@example.com"));

    // Trimming applied.
    let v = validate_value(&field, &json!("  user@example.com  ")).expect("trim");
    assert_eq!(v, json!("user@example.com"));

    // Missing @ rejected.
    let err = validate_value(&field, &json!("not-an-email"));
    assert!(err.is_err());
}

// ===========================================================================
// 4. REQUIRED FIELD VALIDATION
// ===========================================================================

#[tokio::test]
async fn test_field_required_rejects_null() {
    let field = make_field("name", FieldType::SingleLineText, true, None);

    // Null rejected when required.
    let err = validate_value(&field, &json!(null));
    assert!(err.is_err());

    // Non-null accepted.
    let v = validate_value(&field, &json!("Alice")).expect("valid");
    assert_eq!(v, json!("Alice"));
}

#[tokio::test]
async fn test_field_required_with_data() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let field = make_field("username", FieldType::SingleLineText, true, None);

    // Simulate setting a record with the required field.
    let eid = Uuid::new_v4();
    let validated = validate_value(&field, &json!("darshan")).expect("valid value");
    let tx = store
        .set_triples(&[TripleInput {
            entity_id: eid,
            attribute: "test/username".into(),
            value: validated,
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .expect("set");

    let triples = store.get_entity(eid).await.expect("get");
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].value, json!("darshan"));

    // Attempting to set null should fail validation before write.
    let err = validate_value(&field, &json!(null));
    assert!(err.is_err());

    cleanup_entities(&pool, &[eid]).await;
    sqlx::query("DELETE FROM tx_merkle_roots WHERE tx_id = $1")
        .bind(tx)
        .execute(&pool)
        .await
        .ok();
}

// ===========================================================================
// 5. COMPUTED FIELDS REJECT USER INPUT
// ===========================================================================

#[tokio::test]
async fn test_computed_field_rejects_input() {
    let computed_types = [
        FieldType::AutoNumber,
        FieldType::CreatedTime,
        FieldType::LastModifiedTime,
        FieldType::CreatedBy,
        FieldType::LastModifiedBy,
        FieldType::Lookup,
        FieldType::Rollup,
        FieldType::Formula,
    ];

    for ft in computed_types {
        let field = make_field("computed", ft, false, None);
        let err = validate_value(&field, &json!("anything"));
        assert!(
            err.is_err(),
            "computed field type {:?} should reject user input",
            ft
        );
    }
}

// ===========================================================================
// 6. TYPE CONVERSION (text -> number, number -> text, etc.)
// ===========================================================================

#[tokio::test]
async fn test_field_conversion_text_to_number() {
    let values = vec![json!("42"), json!("3.14"), json!("abc"), json!(null)];

    let results = convert_field_type(&values, FieldType::SingleLineText, FieldType::Number);

    let summary = summarise(&results);
    assert_eq!(summary.total, 4);
    // "42" and "3.14" should convert successfully, "abc" should fail, null passes.
    assert!(summary.success >= 3); // null + two numeric strings
    assert!(summary.failed >= 1); // "abc"

    // "42" should produce a number.
    assert!(results[0].value.is_some());
    let num = results[0].value.as_ref().unwrap();
    assert!(num.as_f64().is_some());

    // null should pass through.
    assert!(results[3].value.is_some());
    assert!(results[3].value.as_ref().unwrap().is_null());
}

#[tokio::test]
async fn test_field_conversion_number_to_text() {
    let values = vec![json!(42), json!(3.14), json!(0), json!(null)];

    let results = convert_field_type(&values, FieldType::Number, FieldType::SingleLineText);

    let summary = summarise(&results);
    // Number to text is always lossless.
    assert_eq!(summary.total, 4);
    assert_eq!(summary.success, 4);
    assert_eq!(summary.failed, 0);

    // All values should be strings now.
    for (i, r) in results.iter().enumerate() {
        let v = r.value.as_ref().expect("should have value");
        if values[i].is_null() {
            assert!(v.is_null());
        } else {
            assert!(v.is_string(), "result[{i}] should be a string");
        }
    }
}

#[tokio::test]
async fn test_field_conversion_identity() {
    let values = vec![json!("hello"), json!("world")];

    let results = convert_field_type(
        &values,
        FieldType::SingleLineText,
        FieldType::SingleLineText,
    );

    // Same type -> identity conversion, all succeed.
    let summary = summarise(&results);
    assert_eq!(summary.success, 2);
    assert_eq!(summary.failed, 0);
    assert_eq!(results[0].value.as_ref().unwrap(), &json!("hello"));
}

#[tokio::test]
async fn test_field_conversion_checkbox_to_text() {
    let values = vec![json!(true), json!(false)];

    let results = convert_field_type(&values, FieldType::Checkbox, FieldType::SingleLineText);

    let summary = summarise(&results);
    assert_eq!(summary.success, 2);
    for r in &results {
        assert!(r.value.as_ref().unwrap().is_string());
    }
}

// ===========================================================================
// 7. SELECT FIELD VALIDATION
// ===========================================================================

#[tokio::test]
async fn test_field_single_select_validation() {
    let field = make_field(
        "status",
        FieldType::SingleSelect,
        false,
        Some(FieldOptions::Select {
            choices: vec![
                SelectChoice {
                    id: "1".into(),
                    name: "Open".into(),
                    color: "#green".into(),
                },
                SelectChoice {
                    id: "2".into(),
                    name: "Closed".into(),
                    color: "#red".into(),
                },
            ],
        }),
    );

    // Valid choice.
    let v = validate_value(&field, &json!("Open")).expect("valid");
    assert_eq!(v, json!("Open"));

    // Invalid choice rejected.
    let err = validate_value(&field, &json!("InvalidStatus"));
    assert!(err.is_err());
}

// ===========================================================================
// 8. FIELD SERIALIZATION ROUNDTRIP
// ===========================================================================

#[tokio::test]
async fn test_field_config_serde_roundtrip() {
    let config = FieldConfig {
        id: FieldId::new(),
        name: "Status".into(),
        field_type: FieldType::SingleSelect,
        table_entity_type: "task".into(),
        description: Some("Task status".into()),
        required: true,
        unique: false,
        default_value: Some(json!("todo")),
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

    let json = serde_json::to_string(&config).expect("serialize");
    let restored: FieldConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(config, restored);
}
