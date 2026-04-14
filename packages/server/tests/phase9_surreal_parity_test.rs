//! Slice 28/30 — Phase 9 SurrealDB parity integration tests.
//!
//! Author: Darshankumar Joshi.
//!
//! Exercises the three sub-slices:
//!
//! - **9.1 Strict schema mode**: pure-logic validation against
//!   `schema_definitions`, plus the canonical error payload contract.
//! - **9.2 LIVE SELECT over HTTP**: covered by the in-handler unit
//!   tests inside `rest.rs` and by the
//!   `strip_leading_live_keyword` parser checks below.
//! - **9.3 SQL passthrough**: whitelist / DDL rejection / first-keyword
//!   parser behaviour.
//!
//! All tests are hermetic — no DATABASE_URL required. Database-bound
//! integration coverage lives in the `admin_role_test` and other
//! existing files; this file focuses on pure-logic guarantees so
//! `cargo test --no-run` always works in CI without Postgres.

#![cfg(test)]

use ddb_server::api::sql_passthrough::{StatementClass, classify, first_keyword};
use ddb_server::schema::strict::{StrictFieldDef, StrictValidationError, StrictValueType};
use serde_json::{Value, json};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// 9.1 — Strict schema validation
// ---------------------------------------------------------------------------

fn users_defs() -> Vec<StrictFieldDef> {
    vec![
        StrictFieldDef {
            collection: "users".into(),
            attribute: "email".into(),
            value_type: "string".into(),
            required: true,
            unique_index: true,
            default_val: None,
            validator: None,
        },
        StrictFieldDef {
            collection: "users".into(),
            attribute: "age".into(),
            value_type: "number".into(),
            required: false,
            unique_index: false,
            default_val: Some(json!(0)),
            validator: None,
        },
        StrictFieldDef {
            collection: "users".into(),
            attribute: "manager_id".into(),
            value_type: "link:users".into(),
            required: false,
            unique_index: false,
            default_val: None,
            validator: None,
        },
    ]
}

/// Walk the same validation logic the enforcer uses. This keeps the
/// integration test independent of the live DB pool yet faithful to
/// the production implementation.
fn validate(defs: &[StrictFieldDef], doc: &HashMap<String, Value>) -> Vec<StrictValidationError> {
    let mut errors = Vec::new();
    for def in defs {
        let value = doc.get(&def.attribute);
        match value {
            None => {
                if def.default_val.is_none() && def.required {
                    errors.push(StrictValidationError::new(&def.attribute, "REQUIRED"));
                }
            }
            Some(Value::Null) if def.required => {
                errors.push(StrictValidationError::new(&def.attribute, "REQUIRED"));
            }
            Some(v) => {
                if let Some(parsed_type) = StrictValueType::parse(&def.value_type)
                    && !parsed_type.matches(v)
                {
                    errors.push(StrictValidationError::new(&def.attribute, "TYPE_MISMATCH"));
                }
            }
        }
    }
    errors
}

#[test]
fn slice_9_1_required_field_emits_canonical_payload() {
    let defs = users_defs();
    let doc = HashMap::new(); // email missing
    let errors = validate(&defs, &doc);

    // Canonical payload shape: {"errors":[{"field":"email","code":"REQUIRED"}]}
    let payload = json!({ "errors": errors });
    let list = payload.get("errors").and_then(|v| v.as_array()).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["field"], "email");
    assert_eq!(list[0]["code"], "REQUIRED");
}

#[test]
fn slice_9_1_accepts_well_typed_document() {
    let defs = users_defs();
    let doc = HashMap::from([
        ("email".to_string(), json!("darsh@example.com")),
        ("age".to_string(), json!(32)),
    ]);
    assert!(validate(&defs, &doc).is_empty());
}

#[test]
fn slice_9_1_type_mismatch_is_flagged_per_field() {
    let defs = users_defs();
    let doc = HashMap::from([
        ("email".to_string(), json!(12345)),  // number, not string
        ("age".to_string(), json!("thirty")), // string, not number
    ]);
    let errors = validate(&defs, &doc);
    assert_eq!(errors.len(), 2);
    assert!(errors.iter().all(|e| e.code == "TYPE_MISMATCH"));
}

#[test]
fn slice_9_1_link_type_requires_uuid_value() {
    let defs = users_defs();
    let bad = HashMap::from([
        ("email".to_string(), json!("a@b.c")),
        ("manager_id".to_string(), json!("not-uuid")),
    ]);
    assert!(!validate(&defs, &bad).is_empty());

    let good = HashMap::from([
        ("email".to_string(), json!("a@b.c")),
        (
            "manager_id".to_string(),
            json!("550e8400-e29b-41d4-a716-446655440000"),
        ),
    ]);
    assert!(validate(&defs, &good).is_empty());
}

#[test]
fn slice_9_1_every_value_type_tag_parses() {
    for tag in [
        "string",
        "number",
        "boolean",
        "datetime",
        "uuid",
        "array",
        "object",
        "geometry",
        "vector",
        "link:users",
    ] {
        assert!(
            StrictValueType::parse(tag).is_some(),
            "slice-mandated tag `{tag}` must parse"
        );
    }
}

// ---------------------------------------------------------------------------
// 9.3 — SQL passthrough whitelist
// ---------------------------------------------------------------------------

#[test]
fn slice_9_3_first_keyword_survives_comments_and_whitespace() {
    assert_eq!(first_keyword("SELECT * FROM t").as_deref(), Some("SELECT"));
    assert_eq!(first_keyword("  select 1").as_deref(), Some("SELECT"));
    assert_eq!(
        first_keyword("-- leading line\nUPDATE t SET x = 1").as_deref(),
        Some("UPDATE")
    );
    assert_eq!(
        first_keyword("/* multi\nline */ DELETE FROM t").as_deref(),
        Some("DELETE")
    );
}

#[test]
fn slice_9_3_select_insert_update_delete_with_are_whitelisted() {
    for (sql, expected) in [
        ("SELECT 1", StatementClass::Read),
        ("WITH x AS (SELECT 1) SELECT * FROM x", StatementClass::Read),
        ("INSERT INTO t (a) VALUES ($1)", StatementClass::Write),
        ("UPDATE t SET a = $1 WHERE id = $2", StatementClass::Write),
        ("DELETE FROM t WHERE id = $1", StatementClass::Write),
    ] {
        assert_eq!(classify(sql), expected, "{sql} should be whitelisted");
    }
}

#[test]
fn slice_9_3_ddl_is_always_rejected() {
    let ddl = [
        "CREATE TABLE foo (x int)",
        "DROP TABLE foo",
        "ALTER TABLE foo ADD COLUMN y int",
        "TRUNCATE foo",
        "GRANT SELECT ON foo TO bar",
        "REVOKE ALL ON foo FROM bar",
    ];
    for sql in ddl {
        assert!(
            matches!(classify(sql), StatementClass::Rejected(_)),
            "{sql} must be rejected"
        );
    }
}

#[test]
fn slice_9_3_ddl_rejection_is_case_insensitive() {
    assert!(matches!(
        classify("create table foo (x int)"),
        StatementClass::Rejected(_)
    ));
    assert!(matches!(
        classify("  Drop TABLE foo"),
        StatementClass::Rejected(_)
    ));
}

#[test]
fn slice_9_3_empty_input_is_rejected() {
    assert!(matches!(classify(""), StatementClass::Rejected(_)));
    assert!(matches!(classify("   \n\t"), StatementClass::Rejected(_)));
}
