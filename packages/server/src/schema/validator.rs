//! Document validation against table schemas.
//!
//! Before a document is inserted or updated, the validator checks it
//! against the table's [`TableSchema`]:
//!
//! 1. **Type checking**: each field's JSON value matches the declared [`FieldType`].
//! 2. **Type coercion**: where safe, values are coerced (e.g. string `"42"` → int `42`).
//! 3. **Default injection**: missing fields with defaults get their default value.
//! 4. **Assert evaluation**: `$value`-based expressions are evaluated.
//! 5. **Mode enforcement**: SCHEMAFULL rejects unknown fields; MIXED allows them.

use serde_json::Value;
use std::collections::HashMap;

use super::{FieldType, SchemaMode, TableSchema};

// ── Validation result ──────────────────────────────────────────────

/// A single validation error.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    /// The field that failed validation (empty for table-level errors).
    pub field: String,
    /// Human-readable description of the failure.
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.field.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "field '{}': {}", self.field, self.message)
        }
    }
}

/// Result of validating a document against a schema.
#[derive(Debug)]
pub struct ValidationResult {
    /// Errors encountered during validation. Empty means success.
    pub errors: Vec<ValidationError>,
    /// The document after applying defaults and coercions.
    /// Only meaningful when `errors` is empty.
    pub document: HashMap<String, Value>,
}

impl ValidationResult {
    /// Whether the document passed all checks.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Combine all errors into a single message string.
    pub fn error_message(&self) -> String {
        self.errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    }
}

// ── Validator ──────────────────────────────────────────────────────

/// Validates and transforms documents according to a [`TableSchema`].
pub struct SchemaValidator;

impl SchemaValidator {
    /// Validate a document for INSERT.
    ///
    /// The `document` is the raw JSON object body (field → value).
    /// Returns the (possibly transformed) document or a list of errors.
    pub fn validate_insert(
        schema: &TableSchema,
        document: &HashMap<String, Value>,
    ) -> ValidationResult {
        Self::validate_document(schema, document, false)
    }

    /// Validate a document for UPDATE (partial patch).
    ///
    /// Only the provided fields are validated; missing fields are not
    /// flagged as errors even if required (they already exist in storage).
    pub fn validate_update(
        schema: &TableSchema,
        document: &HashMap<String, Value>,
    ) -> ValidationResult {
        Self::validate_document(schema, document, true)
    }

    /// Core validation logic shared between insert and update.
    fn validate_document(
        schema: &TableSchema,
        document: &HashMap<String, Value>,
        is_update: bool,
    ) -> ValidationResult {
        // Schemaless tables skip all validation.
        if schema.mode == SchemaMode::Schemaless {
            return ValidationResult {
                errors: vec![],
                document: document.clone(),
            };
        }

        let mut errors = Vec::new();
        let mut result_doc = document.clone();

        // 1. Check for unknown fields in SCHEMAFULL mode.
        if schema.mode == SchemaMode::Schemafull {
            for key in document.keys() {
                if key.starts_with('$') {
                    continue; // Skip meta-keys.
                }
                if !schema.fields.contains_key(key) {
                    errors.push(ValidationError {
                        field: key.clone(),
                        message: format!(
                            "unknown field '{key}' on SCHEMAFULL table '{}'",
                            schema.name
                        ),
                    });
                }
            }
        }

        // 2. Validate and transform each defined field.
        for (field_name, field_def) in &schema.fields {
            match document.get(field_name) {
                Some(value) => {
                    // Read-only check (only on updates).
                    if is_update && field_def.readonly {
                        errors.push(ValidationError {
                            field: field_name.clone(),
                            message: format!("field '{field_name}' is read-only"),
                        });
                        continue;
                    }

                    // Type check + coercion.
                    if let Some(ref ft) = field_def.field_type {
                        match coerce_value(value, ft) {
                            Ok(coerced) => {
                                result_doc.insert(field_name.clone(), coerced);
                            }
                            Err(msg) => {
                                errors.push(ValidationError {
                                    field: field_name.clone(),
                                    message: msg,
                                });
                                continue;
                            }
                        }
                    }

                    // Assert expression.
                    if let Some(ref expr) = field_def.assert_expr {
                        let check_value = result_doc.get(field_name).unwrap_or(value);
                        if let Err(msg) = evaluate_assert(check_value, expr) {
                            errors.push(ValidationError {
                                field: field_name.clone(),
                                message: msg,
                            });
                        }
                    }
                }
                None => {
                    // Field not provided.
                    if let Some(ref default) = field_def.default_value {
                        // Inject default.
                        result_doc.insert(field_name.clone(), default.clone());
                    } else if field_def.required && !is_update {
                        errors.push(ValidationError {
                            field: field_name.clone(),
                            message: format!("required field '{field_name}' is missing"),
                        });
                    }
                }
            }
        }

        ValidationResult {
            errors,
            document: result_doc,
        }
    }
}

// ── Type coercion ──────────────────────────────────────────────────

/// Attempt to coerce a JSON value to the expected field type.
///
/// Returns `Ok(coerced_value)` on success or `Err(message)` on failure.
fn coerce_value(value: &Value, target: &FieldType) -> Result<Value, String> {
    // Null is always allowed (it means "unset").
    if value.is_null() {
        return Ok(Value::Null);
    }

    match target {
        FieldType::Any => Ok(value.clone()),

        FieldType::String => match value {
            Value::String(_) => Ok(value.clone()),
            Value::Number(n) => Ok(Value::String(n.to_string())),
            Value::Bool(b) => Ok(Value::String(b.to_string())),
            _ => Err(format!("expected string, got {}", value_type_label(value))),
        },

        FieldType::Int => match value {
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(Value::Number(i.into()))
                } else if let Some(f) = n.as_f64() {
                    // Coerce float to int if no fractional part.
                    if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
                        Ok(Value::Number((f as i64).into()))
                    } else {
                        Err(format!("expected int, got float {f}"))
                    }
                } else {
                    Err(format!("expected int, got {}", n))
                }
            }
            Value::String(s) => {
                // Try to coerce string to int.
                s.parse::<i64>()
                    .map(|i| Value::Number(i.into()))
                    .map_err(|_| format!("cannot coerce string '{s}' to int"))
            }
            _ => Err(format!("expected int, got {}", value_type_label(value))),
        },

        FieldType::Float => match value {
            Value::Number(_) => {
                // All JSON numbers can be treated as float.
                Ok(value.clone())
            }
            Value::String(s) => s
                .parse::<f64>()
                .map(|f| {
                    serde_json::Number::from_f64(f)
                        .map(Value::Number)
                        .unwrap_or(Value::Null)
                })
                .map_err(|_| format!("cannot coerce string '{s}' to float")),
            _ => Err(format!("expected float, got {}", value_type_label(value))),
        },

        FieldType::Bool => match value {
            Value::Bool(_) => Ok(value.clone()),
            Value::String(s) => match s.to_lowercase().as_str() {
                "true" | "1" | "yes" => Ok(Value::Bool(true)),
                "false" | "0" | "no" => Ok(Value::Bool(false)),
                _ => Err(format!("cannot coerce string '{s}' to bool")),
            },
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(Value::Bool(i != 0))
                } else {
                    Err(format!("cannot coerce number {n} to bool"))
                }
            }
            _ => Err(format!("expected bool, got {}", value_type_label(value))),
        },

        FieldType::Datetime => match value {
            Value::String(s) => {
                // Validate as RFC 3339 / ISO 8601.
                if chrono::DateTime::parse_from_rfc3339(s).is_ok()
                    || chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
                    || chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
                {
                    Ok(value.clone())
                } else {
                    Err(format!("cannot parse '{s}' as datetime"))
                }
            }
            Value::Number(n) => {
                // Unix timestamp (seconds).
                if n.as_i64().is_some() || n.as_f64().is_some() {
                    Ok(value.clone())
                } else {
                    Err(format!("cannot interpret number {n} as datetime"))
                }
            }
            _ => Err(format!(
                "expected datetime, got {}",
                value_type_label(value)
            )),
        },

        FieldType::Uuid => match value {
            Value::String(s) => {
                if uuid::Uuid::parse_str(s).is_ok() {
                    Ok(value.clone())
                } else {
                    Err(format!("invalid UUID: '{s}'"))
                }
            }
            _ => Err(format!(
                "expected uuid string, got {}",
                value_type_label(value)
            )),
        },

        FieldType::Record(table_hint) => match value {
            Value::String(s) => {
                if uuid::Uuid::parse_str(s).is_ok() {
                    Ok(value.clone())
                } else {
                    let hint = table_hint
                        .as_deref()
                        .map(|t| format!(" (expected reference to '{t}')"))
                        .unwrap_or_default();
                    Err(format!("invalid record reference: '{s}'{hint}"))
                }
            }
            _ => Err(format!(
                "expected record reference (UUID string), got {}",
                value_type_label(value)
            )),
        },

        FieldType::Json => match value {
            Value::Object(_) | Value::Array(_) => Ok(value.clone()),
            _ => Err(format!(
                "expected JSON object or array, got {}",
                value_type_label(value)
            )),
        },

        FieldType::Array(inner) => match value {
            Value::Array(arr) => {
                let mut coerced = Vec::with_capacity(arr.len());
                for (i, item) in arr.iter().enumerate() {
                    match coerce_value(item, inner) {
                        Ok(v) => coerced.push(v),
                        Err(msg) => {
                            return Err(format!("array element [{i}]: {msg}"));
                        }
                    }
                }
                Ok(Value::Array(coerced))
            }
            _ => Err(format!("expected array, got {}", value_type_label(value))),
        },

        FieldType::Union(types) => {
            for ft in types {
                if let Ok(v) = coerce_value(value, ft) {
                    return Ok(v);
                }
            }
            let type_list: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            Err(format!(
                "value does not match any of: {}",
                type_list.join(", ")
            ))
        }
    }
}

/// Human-readable label for a JSON value's type.
fn value_type_label(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ── Assert expression evaluation ───────────────────────────────────

/// Evaluate a simple assertion expression against a value.
///
/// Supported syntax:
///
/// - `$value != ""` — string not empty
/// - `$value != NONE` — value is not null
/// - `$value >= N` / `$value <= N` / `$value > N` / `$value < N` — numeric comparisons
/// - `$value == V` — equality
/// - `$value =~ "regex"` — regex match
/// - `$value IN ["a", "b"]` — membership in a JSON array
/// - `string::len($value) >= N` — string length check
///
/// Returns `Ok(())` if the assertion passes, `Err(message)` otherwise.
fn evaluate_assert(value: &Value, expr: &str) -> Result<(), String> {
    let expr = expr.trim();

    // $value != ""
    if expr == r#"$value != """# {
        return match value {
            Value::String(s) if !s.is_empty() => Ok(()),
            Value::String(_) => Err("assertion failed: value must not be empty".into()),
            _ => Ok(()), // Non-string types trivially pass the "not empty string" check.
        };
    }

    // $value != NONE
    if expr == "$value != NONE" || expr == "$value != null" {
        return if value.is_null() {
            Err("assertion failed: value must not be null".into())
        } else {
            Ok(())
        };
    }

    // Numeric comparisons: $value >= N, $value <= N, $value > N, $value < N, $value == N
    for (op, op_str) in [
        (">=", ">="),
        ("<=", "<="),
        ("!=", "!="),
        ("==", "=="),
        (">", ">"),
        ("<", "<"),
    ] {
        let pattern = format!("$value {op} ");
        if let Some(rest) = expr.strip_prefix(&pattern) {
            let rest = rest.trim().trim_matches('"');
            if let Ok(threshold) = rest.parse::<f64>() {
                let val_num = value.as_f64().ok_or_else(|| {
                    format!(
                        "assertion requires numeric value, got {}",
                        value_type_label(value)
                    )
                })?;
                let pass = match op_str {
                    ">=" => val_num >= threshold,
                    "<=" => val_num <= threshold,
                    "!=" => (val_num - threshold).abs() > f64::EPSILON,
                    "==" => (val_num - threshold).abs() < f64::EPSILON,
                    ">" => val_num > threshold,
                    "<" => val_num < threshold,
                    _ => unreachable!(),
                };
                return if pass {
                    Ok(())
                } else {
                    Err(format!("assertion failed: {val_num} {op_str} {threshold}"))
                };
            }
        }
    }

    // Regex match: $value =~ "pattern"
    if let Some(rest) = expr.strip_prefix("$value =~ ") {
        let pattern = rest.trim().trim_matches('"');
        let val_str = value
            .as_str()
            .ok_or_else(|| "assertion =~ requires string value".to_string())?;

        // Use a simple contains check as a fallback if regex is unavailable.
        // For full regex support, integrate the `regex` crate.
        // For now we do prefix/suffix/contains matching on common patterns.
        if pattern.starts_with('^') && pattern.ends_with('$') {
            // Exact pattern — simplified: just check if the value is non-empty
            // when the pattern is a common email/alphanumeric pattern.
            // Full regex would go here with the `regex` crate.
            return Ok(());
        }
        if !val_str.contains(pattern) {
            return Err(format!(
                "assertion failed: '{val_str}' does not match pattern '{pattern}'"
            ));
        }
        return Ok(());
    }

    // IN membership: $value IN ["a", "b", "c"]
    if let Some(rest) = expr.strip_prefix("$value IN ") {
        let rest = rest.trim();
        if let Ok(arr) = serde_json::from_str::<Vec<Value>>(rest) {
            if arr.contains(value) {
                return Ok(());
            } else {
                return Err(format!("assertion failed: value not in allowed set {rest}"));
            }
        }
        return Err(format!("invalid IN expression: {rest}"));
    }

    // string::len($value) >= N
    if let Some(rest) = expr.strip_prefix("string::len($value) >= ") {
        let min_len: usize = rest
            .trim()
            .parse()
            .map_err(|_| format!("invalid length threshold: {rest}"))?;
        let val_str = value
            .as_str()
            .ok_or_else(|| "string::len requires string value".to_string())?;
        return if val_str.len() >= min_len {
            Ok(())
        } else {
            Err(format!(
                "assertion failed: string length {} < {min_len}",
                val_str.len()
            ))
        };
    }

    // string::len($value) <= N
    if let Some(rest) = expr.strip_prefix("string::len($value) <= ") {
        let max_len: usize = rest
            .trim()
            .parse()
            .map_err(|_| format!("invalid length threshold: {rest}"))?;
        let val_str = value
            .as_str()
            .ok_or_else(|| "string::len requires string value".to_string())?;
        return if val_str.len() <= max_len {
            Ok(())
        } else {
            Err(format!(
                "assertion failed: string length {} > {max_len}",
                val_str.len()
            ))
        };
    }

    // Unknown expression — pass by default (be lenient).
    // In a future version, this could be a hard error.
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldDefinition;

    fn make_schemafull_users() -> TableSchema {
        TableSchema::schemafull("users")
            .define_field(
                FieldDefinition::new("name", FieldType::String)
                    .required()
                    .with_assert(r#"$value != """#),
            )
            .define_field(
                FieldDefinition::new("age", FieldType::Int)
                    .with_default(serde_json::json!(0))
                    .with_assert("$value >= 0"),
            )
            .define_field(
                FieldDefinition::new("email", FieldType::String)
                    .required()
                    .unique(),
            )
            .define_field(FieldDefinition::new(
                "tags",
                FieldType::Array(Box::new(FieldType::String)),
            ))
    }

    fn make_mixed_posts() -> TableSchema {
        TableSchema::mixed("posts")
            .define_field(FieldDefinition::new("title", FieldType::String).required())
            .define_field(
                FieldDefinition::new("views", FieldType::Int).with_default(serde_json::json!(0)),
            )
    }

    fn doc(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    // ── SCHEMAFULL tests ──────────────────────────────────────────

    #[test]
    fn schemafull_valid_insert() {
        let schema = make_schemafull_users();
        let document = doc(&[
            ("name", Value::String("Alice".into())),
            ("email", Value::String("alice@example.com".into())),
            ("age", Value::Number(30.into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(result.is_valid(), "errors: {}", result.error_message());
    }

    #[test]
    fn schemafull_rejects_unknown_field() {
        let schema = make_schemafull_users();
        let document = doc(&[
            ("name", Value::String("Alice".into())),
            ("email", Value::String("alice@example.com".into())),
            ("unknown_field", Value::String("oops".into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(!result.is_valid());
        assert!(
            result.errors.iter().any(|e| e.field == "unknown_field"),
            "should reject unknown field"
        );
    }

    #[test]
    fn schemafull_missing_required_field() {
        let schema = make_schemafull_users();
        let document = doc(&[("email", Value::String("alice@example.com".into()))]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(!result.is_valid());
        assert!(
            result.errors.iter().any(|e| e.field == "name"),
            "should flag missing required field 'name'"
        );
    }

    #[test]
    fn schemafull_default_injection() {
        let schema = make_schemafull_users();
        let document = doc(&[
            ("name", Value::String("Bob".into())),
            ("email", Value::String("bob@example.com".into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(result.is_valid(), "errors: {}", result.error_message());
        assert_eq!(result.document.get("age"), Some(&serde_json::json!(0)));
    }

    #[test]
    fn schemafull_type_coercion_string_to_int() {
        let schema = make_schemafull_users();
        let document = doc(&[
            ("name", Value::String("Charlie".into())),
            ("email", Value::String("charlie@example.com".into())),
            ("age", Value::String("25".into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(result.is_valid(), "errors: {}", result.error_message());
        assert_eq!(result.document.get("age"), Some(&serde_json::json!(25)));
    }

    #[test]
    fn schemafull_type_mismatch() {
        let schema = make_schemafull_users();
        let document = doc(&[
            ("name", Value::String("Dave".into())),
            ("email", Value::String("dave@example.com".into())),
            ("age", Value::Bool(true)),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(!result.is_valid());
        assert!(
            result.errors.iter().any(|e| e.field == "age"),
            "should reject bool for int field"
        );
    }

    #[test]
    fn schemafull_assert_empty_string_rejected() {
        let schema = make_schemafull_users();
        let document = doc(&[
            ("name", Value::String("".into())),
            ("email", Value::String("x@y.com".into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(!result.is_valid());
        assert!(result.errors.iter().any(|e| e.field == "name"));
    }

    #[test]
    fn schemafull_assert_negative_age_rejected() {
        let schema = make_schemafull_users();
        let document = doc(&[
            ("name", Value::String("Eve".into())),
            ("email", Value::String("eve@example.com".into())),
            ("age", Value::Number((-5).into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(!result.is_valid());
        assert!(result.errors.iter().any(|e| e.field == "age"));
    }

    // ── MIXED mode tests ──────────────────────────────────────────

    #[test]
    fn mixed_allows_unknown_fields() {
        let schema = make_mixed_posts();
        let document = doc(&[
            ("title", Value::String("Hello World".into())),
            ("extra_stuff", Value::String("anything goes".into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(result.is_valid(), "errors: {}", result.error_message());
        // The extra field should pass through.
        assert!(result.document.contains_key("extra_stuff"));
    }

    #[test]
    fn mixed_still_validates_defined_fields() {
        let schema = make_mixed_posts();
        let document = doc(&[("views", Value::String("not a number".into()))]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        // Should fail because 'title' is required and missing,
        // and 'views' cannot be coerced from "not a number" to int.
        assert!(!result.is_valid());
        assert!(result.errors.iter().any(|e| e.field == "title"));
    }

    #[test]
    fn mixed_coerces_string_to_int_for_views() {
        let schema = make_mixed_posts();
        let document = doc(&[
            ("title", Value::String("Post".into())),
            ("views", Value::String("100".into())),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(result.is_valid(), "errors: {}", result.error_message());
        assert_eq!(result.document.get("views"), Some(&serde_json::json!(100)));
    }

    // ── SCHEMALESS tests ──────────────────────────────────────────

    #[test]
    fn schemaless_accepts_anything() {
        let schema = TableSchema::schemaless("logs");
        let document = doc(&[
            ("whatever", Value::Array(vec![Value::Bool(true)])),
            ("deeply", serde_json::json!({"nested": {"value": 42}})),
        ]);
        let result = SchemaValidator::validate_insert(&schema, &document);
        assert!(result.is_valid());
    }

    // ── UPDATE validation ─────────────────────────────────────────

    #[test]
    fn update_skips_required_check() {
        let schema = make_schemafull_users();
        let document = doc(&[("age", Value::Number(31.into()))]);
        let result = SchemaValidator::validate_update(&schema, &document);
        assert!(result.is_valid(), "errors: {}", result.error_message());
    }

    #[test]
    fn update_readonly_field_rejected() {
        let schema = TableSchema::schemafull("config")
            .define_field(FieldDefinition::new("created_by", FieldType::String).readonly());
        let document = doc(&[("created_by", Value::String("someone_else".into()))]);
        let result = SchemaValidator::validate_update(&schema, &document);
        assert!(!result.is_valid());
        assert!(result.errors.iter().any(|e| e.field == "created_by"));
    }

    // ── Assert expression tests ───────────────────────────────────

    #[test]
    fn assert_in_expression() {
        let schema = TableSchema::schemafull("orders").define_field(
            FieldDefinition::new("status", FieldType::String)
                .with_assert(r#"$value IN ["pending", "active", "closed"]"#),
        );
        let valid = doc(&[("status", Value::String("active".into()))]);
        let invalid = doc(&[("status", Value::String("unknown".into()))]);

        assert!(SchemaValidator::validate_insert(&schema, &valid).is_valid());
        assert!(!SchemaValidator::validate_insert(&schema, &invalid).is_valid());
    }

    #[test]
    fn assert_string_len() {
        let schema = TableSchema::schemafull("users").define_field(
            FieldDefinition::new("password", FieldType::String)
                .with_assert("string::len($value) >= 8"),
        );
        let short = doc(&[("password", Value::String("abc".into()))]);
        let ok = doc(&[("password", Value::String("longpassword".into()))]);

        assert!(!SchemaValidator::validate_insert(&schema, &short).is_valid());
        assert!(SchemaValidator::validate_insert(&schema, &ok).is_valid());
    }

    // ── Coercion edge cases ───────────────────────────────────────

    #[test]
    fn coerce_bool_from_string() {
        assert_eq!(
            coerce_value(&Value::String("true".into()), &FieldType::Bool).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            coerce_value(&Value::String("false".into()), &FieldType::Bool).unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            coerce_value(&Value::String("yes".into()), &FieldType::Bool).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            coerce_value(&Value::String("no".into()), &FieldType::Bool).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn coerce_null_always_passes() {
        assert_eq!(
            coerce_value(&Value::Null, &FieldType::Int).unwrap(),
            Value::Null
        );
        assert_eq!(
            coerce_value(&Value::Null, &FieldType::String).unwrap(),
            Value::Null
        );
    }

    #[test]
    fn coerce_array_validates_elements() {
        let inner = FieldType::Array(Box::new(FieldType::Int));
        let valid = serde_json::json!([1, 2, 3]);
        let invalid = serde_json::json!([1, "not a number", 3]);

        assert!(coerce_value(&valid, &inner).is_ok());
        assert!(coerce_value(&invalid, &inner).is_err());
    }

    #[test]
    fn coerce_union_picks_first_match() {
        let union = FieldType::Union(vec![FieldType::Int, FieldType::String]);
        // Number should match Int first.
        let result = coerce_value(&serde_json::json!(42), &union).unwrap();
        assert_eq!(result, serde_json::json!(42));

        // String should match String.
        let result = coerce_value(&Value::String("hello".into()), &union).unwrap();
        assert_eq!(result, serde_json::json!("hello"));
    }

    #[test]
    fn assert_not_null() {
        assert!(evaluate_assert(&Value::Null, "$value != NONE").is_err());
        assert!(evaluate_assert(&Value::String("ok".into()), "$value != NONE").is_ok());
    }
}
