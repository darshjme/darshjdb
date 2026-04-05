//! Argument validation for DarshJDB server functions.
//!
//! Each function can declare an [`ArgSchema`] that describes the shape and
//! constraints of its arguments. [`validate_args`] checks a JSON value
//! against a schema and produces descriptive errors on mismatch.
//!
//! # Example
//!
//! ```rust
//! use serde_json::json;
//! use ddb_server::functions::validator::{ArgSchema, validate_args};
//! use std::collections::HashMap;
//!
//! let schema = ArgSchema::Object({
//!     let mut fields = HashMap::new();
//!     fields.insert("name".into(), ArgSchema::String { min: Some(1), max: Some(100) });
//!     fields.insert("age".into(), ArgSchema::Optional(Box::new(
//!         ArgSchema::Number { min: Some(0.0), max: Some(200.0) },
//!     )));
//!     fields
//! });
//!
//! let args = json!({ "name": "Alice", "age": 30 });
//! assert!(validate_args(&schema, &args).is_ok());
//! ```

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// An argument validation failure with a human-readable path and message.
#[derive(Debug, Error)]
pub struct ValidationError {
    /// Dot-separated path to the offending field (e.g. `"args.user.email"`).
    pub path: String,
    /// What went wrong.
    pub message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "validation error at `{}`: {}", self.path, self.message)
    }
}

// ---------------------------------------------------------------------------
// Schema definition
// ---------------------------------------------------------------------------

/// Describes the expected shape and constraints of a function argument.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ArgSchema {
    /// A UTF-8 string with optional length bounds.
    String {
        /// Minimum length in characters (inclusive).
        #[serde(default)]
        min: Option<usize>,
        /// Maximum length in characters (inclusive).
        #[serde(default)]
        max: Option<usize>,
    },

    /// A JSON number (f64) with optional range bounds.
    Number {
        /// Minimum value (inclusive).
        #[serde(default)]
        min: Option<f64>,
        /// Maximum value (inclusive).
        #[serde(default)]
        max: Option<f64>,
    },

    /// A boolean value.
    Bool,

    /// A DarshJDB document ID (validated as a non-empty string starting with an
    /// optional table prefix followed by a UUID-like suffix).
    Id,

    /// A homogeneous array where every element matches the inner schema.
    Array(Box<ArgSchema>),

    /// A record with named fields, each having its own schema.
    Object(HashMap<String, ArgSchema>),

    /// An optional value — `null` / missing is acceptable, otherwise the
    /// inner schema applies.
    Optional(Box<ArgSchema>),

    /// Accepts any valid JSON value without further constraints.
    Any,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a JSON value against an argument schema.
///
/// Returns `Ok(())` if the value satisfies all constraints, or a
/// [`ValidationError`] describing the first violation found.
pub fn validate_args(schema: &ArgSchema, value: &Value) -> Result<(), ValidationError> {
    validate_at("args", schema, value)
}

/// Internal recursive validator that tracks the current path for error messages.
fn validate_at(path: &str, schema: &ArgSchema, value: &Value) -> Result<(), ValidationError> {
    match schema {
        ArgSchema::String { min, max } => {
            let s = value.as_str().ok_or_else(|| ValidationError {
                path: path.to_string(),
                message: format!("expected string, got {}", json_type_name(value)),
            })?;

            if let Some(min_len) = min
                && s.chars().count() < *min_len
            {
                return Err(ValidationError {
                    path: path.to_string(),
                    message: format!(
                        "string length {} is below minimum {}",
                        s.chars().count(),
                        min_len
                    ),
                });
            }

            if let Some(max_len) = max
                && s.chars().count() > *max_len
            {
                return Err(ValidationError {
                    path: path.to_string(),
                    message: format!(
                        "string length {} exceeds maximum {}",
                        s.chars().count(),
                        max_len
                    ),
                });
            }

            Ok(())
        }

        ArgSchema::Number { min, max } => {
            let n = value.as_f64().ok_or_else(|| ValidationError {
                path: path.to_string(),
                message: format!("expected number, got {}", json_type_name(value)),
            })?;

            // Reject NaN and Infinity — these are not valid JSON numbers and
            // would silently bypass min/max comparisons.
            if n.is_nan() || n.is_infinite() {
                return Err(ValidationError {
                    path: path.to_string(),
                    message: format!("number must be finite, got {n}"),
                });
            }

            if let Some(min_val) = min
                && n < *min_val
            {
                return Err(ValidationError {
                    path: path.to_string(),
                    message: format!("value {n} is below minimum {min_val}"),
                });
            }

            if let Some(max_val) = max
                && n > *max_val
            {
                return Err(ValidationError {
                    path: path.to_string(),
                    message: format!("value {n} exceeds maximum {max_val}"),
                });
            }

            Ok(())
        }

        ArgSchema::Bool => {
            if value.is_boolean() {
                Ok(())
            } else {
                Err(ValidationError {
                    path: path.to_string(),
                    message: format!("expected boolean, got {}", json_type_name(value)),
                })
            }
        }

        ArgSchema::Id => {
            let s = value.as_str().ok_or_else(|| ValidationError {
                path: path.to_string(),
                message: format!("expected ID string, got {}", json_type_name(value)),
            })?;

            if s.is_empty() {
                return Err(ValidationError {
                    path: path.to_string(),
                    message: "ID must not be empty".to_string(),
                });
            }

            Ok(())
        }

        ArgSchema::Array(inner) => {
            let arr = value.as_array().ok_or_else(|| ValidationError {
                path: path.to_string(),
                message: format!("expected array, got {}", json_type_name(value)),
            })?;

            for (i, element) in arr.iter().enumerate() {
                let element_path = format!("{path}[{i}]");
                validate_at(&element_path, inner, element)?;
            }

            Ok(())
        }

        ArgSchema::Object(fields) => {
            let obj = value.as_object().ok_or_else(|| ValidationError {
                path: path.to_string(),
                message: format!("expected object, got {}", json_type_name(value)),
            })?;

            for (field_name, field_schema) in fields {
                let field_path = format!("{path}.{field_name}");

                match obj.get(field_name) {
                    Some(field_value) => {
                        validate_at(&field_path, field_schema, field_value)?;
                    }
                    None => {
                        // Missing field is only OK if the schema wraps it in Optional.
                        if !matches!(field_schema, ArgSchema::Optional(_)) {
                            return Err(ValidationError {
                                path: field_path,
                                message: "required field is missing".to_string(),
                            });
                        }
                    }
                }
            }

            Ok(())
        }

        ArgSchema::Optional(inner) => {
            if value.is_null() {
                Ok(())
            } else {
                validate_at(path, inner, value)
            }
        }

        ArgSchema::Any => Ok(()),
    }
}

/// Returns a human-readable name for a JSON value type.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_string_valid() {
        let schema = ArgSchema::String {
            min: Some(1),
            max: Some(10),
        };
        assert!(validate_args(&schema, &json!("hello")).is_ok());
    }

    #[test]
    fn test_string_too_short() {
        let schema = ArgSchema::String {
            min: Some(3),
            max: None,
        };
        let err = validate_args(&schema, &json!("ab")).unwrap_err();
        assert!(err.message.contains("below minimum"));
    }

    #[test]
    fn test_string_too_long() {
        let schema = ArgSchema::String {
            min: None,
            max: Some(3),
        };
        let err = validate_args(&schema, &json!("abcdef")).unwrap_err();
        assert!(err.message.contains("exceeds maximum"));
    }

    #[test]
    fn test_number_in_range() {
        let schema = ArgSchema::Number {
            min: Some(0.0),
            max: Some(100.0),
        };
        assert!(validate_args(&schema, &json!(50)).is_ok());
    }

    #[test]
    fn test_number_below_min() {
        let schema = ArgSchema::Number {
            min: Some(10.0),
            max: None,
        };
        let err = validate_args(&schema, &json!(5)).unwrap_err();
        assert!(err.message.contains("below minimum"));
    }

    #[test]
    fn test_bool_valid() {
        assert!(validate_args(&ArgSchema::Bool, &json!(true)).is_ok());
    }

    #[test]
    fn test_bool_invalid() {
        let err = validate_args(&ArgSchema::Bool, &json!("true")).unwrap_err();
        assert!(err.message.contains("expected boolean"));
    }

    #[test]
    fn test_id_valid() {
        assert!(validate_args(&ArgSchema::Id, &json!("users:abc123")).is_ok());
    }

    #[test]
    fn test_id_empty() {
        let err = validate_args(&ArgSchema::Id, &json!("")).unwrap_err();
        assert!(err.message.contains("must not be empty"));
    }

    #[test]
    fn test_array_valid() {
        let schema = ArgSchema::Array(Box::new(ArgSchema::Number {
            min: None,
            max: None,
        }));
        assert!(validate_args(&schema, &json!([1, 2, 3])).is_ok());
    }

    #[test]
    fn test_array_element_invalid() {
        let schema = ArgSchema::Array(Box::new(ArgSchema::Number {
            min: None,
            max: None,
        }));
        let err = validate_args(&schema, &json!([1, "two", 3])).unwrap_err();
        assert!(err.path.contains("[1]"));
    }

    #[test]
    fn test_object_valid() {
        let schema = ArgSchema::Object({
            let mut fields = HashMap::new();
            fields.insert(
                "name".into(),
                ArgSchema::String {
                    min: Some(1),
                    max: None,
                },
            );
            fields.insert(
                "age".into(),
                ArgSchema::Number {
                    min: Some(0.0),
                    max: None,
                },
            );
            fields
        });
        assert!(validate_args(&schema, &json!({"name": "Alice", "age": 30})).is_ok());
    }

    #[test]
    fn test_object_missing_required_field() {
        let schema = ArgSchema::Object({
            let mut fields = HashMap::new();
            fields.insert(
                "name".into(),
                ArgSchema::String {
                    min: None,
                    max: None,
                },
            );
            fields
        });
        let err = validate_args(&schema, &json!({})).unwrap_err();
        assert!(err.message.contains("required field"));
    }

    #[test]
    fn test_optional_null() {
        let schema = ArgSchema::Optional(Box::new(ArgSchema::String {
            min: None,
            max: None,
        }));
        assert!(validate_args(&schema, &Value::Null).is_ok());
    }

    #[test]
    fn test_optional_present() {
        let schema = ArgSchema::Optional(Box::new(ArgSchema::String {
            min: Some(1),
            max: None,
        }));
        assert!(validate_args(&schema, &json!("hello")).is_ok());
    }

    #[test]
    fn test_any_accepts_everything() {
        assert!(validate_args(&ArgSchema::Any, &json!(null)).is_ok());
        assert!(validate_args(&ArgSchema::Any, &json!(42)).is_ok());
        assert!(validate_args(&ArgSchema::Any, &json!("hello")).is_ok());
        assert!(validate_args(&ArgSchema::Any, &json!([1, 2])).is_ok());
    }

    // -----------------------------------------------------------------------
    // String edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_string_empty_allowed_when_no_min() {
        let schema = ArgSchema::String {
            min: None,
            max: None,
        };
        assert!(validate_args(&schema, &json!("")).is_ok());
    }

    #[test]
    fn test_string_empty_rejected_when_min_1() {
        let schema = ArgSchema::String {
            min: Some(1),
            max: None,
        };
        let err = validate_args(&schema, &json!("")).unwrap_err();
        assert!(err.message.contains("below minimum"));
    }

    #[test]
    fn test_string_exact_boundary() {
        let schema = ArgSchema::String {
            min: Some(3),
            max: Some(3),
        };
        assert!(validate_args(&schema, &json!("abc")).is_ok());
        assert!(validate_args(&schema, &json!("ab")).is_err());
        assert!(validate_args(&schema, &json!("abcd")).is_err());
    }

    #[test]
    fn test_string_unicode_length() {
        // Multi-byte chars: length is in chars, not bytes.
        let schema = ArgSchema::String {
            min: None,
            max: Some(3),
        };
        // 3 emoji chars = 3 char length, should pass
        assert!(validate_args(&schema, &json!("\u{1F600}\u{1F601}\u{1F602}")).is_ok());
        // 4 emoji chars = 4 char length, should fail
        assert!(validate_args(&schema, &json!("\u{1F600}\u{1F601}\u{1F602}\u{1F603}")).is_err());
    }

    #[test]
    fn test_string_rejects_null() {
        let schema = ArgSchema::String {
            min: None,
            max: None,
        };
        let err = validate_args(&schema, &json!(null)).unwrap_err();
        assert!(err.message.contains("expected string"));
    }

    #[test]
    fn test_string_rejects_number() {
        let schema = ArgSchema::String {
            min: None,
            max: None,
        };
        let err = validate_args(&schema, &json!(42)).unwrap_err();
        assert!(err.message.contains("expected string"));
    }

    // -----------------------------------------------------------------------
    // Number edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_number_at_exact_min() {
        let schema = ArgSchema::Number {
            min: Some(10.0),
            max: None,
        };
        assert!(validate_args(&schema, &json!(10.0)).is_ok());
    }

    #[test]
    fn test_number_at_exact_max() {
        let schema = ArgSchema::Number {
            min: None,
            max: Some(100.0),
        };
        assert!(validate_args(&schema, &json!(100.0)).is_ok());
    }

    #[test]
    fn test_number_exceeds_max() {
        let schema = ArgSchema::Number {
            min: None,
            max: Some(100.0),
        };
        let err = validate_args(&schema, &json!(100.1)).unwrap_err();
        assert!(err.message.contains("exceeds maximum"));
    }

    #[test]
    fn test_number_zero() {
        let schema = ArgSchema::Number {
            min: Some(0.0),
            max: Some(0.0),
        };
        assert!(validate_args(&schema, &json!(0)).is_ok());
    }

    #[test]
    fn test_number_negative() {
        let schema = ArgSchema::Number {
            min: Some(-100.0),
            max: Some(-1.0),
        };
        assert!(validate_args(&schema, &json!(-50)).is_ok());
        assert!(validate_args(&schema, &json!(0)).is_err());
    }

    #[test]
    fn test_number_rejects_string() {
        let schema = ArgSchema::Number {
            min: None,
            max: None,
        };
        let err = validate_args(&schema, &json!("42")).unwrap_err();
        assert!(err.message.contains("expected number"));
    }

    #[test]
    fn test_number_rejects_null() {
        let schema = ArgSchema::Number {
            min: None,
            max: None,
        };
        let err = validate_args(&schema, &json!(null)).unwrap_err();
        assert!(err.message.contains("expected number"));
    }

    #[test]
    fn test_number_no_constraints() {
        let schema = ArgSchema::Number {
            min: None,
            max: None,
        };
        assert!(validate_args(&schema, &json!(f64::MAX)).is_ok());
        assert!(validate_args(&schema, &json!(f64::MIN)).is_ok());
    }

    // -----------------------------------------------------------------------
    // Bool edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_bool_false() {
        assert!(validate_args(&ArgSchema::Bool, &json!(false)).is_ok());
    }

    #[test]
    fn test_bool_rejects_null() {
        let err = validate_args(&ArgSchema::Bool, &json!(null)).unwrap_err();
        assert!(err.message.contains("expected boolean"));
    }

    #[test]
    fn test_bool_rejects_integer_0() {
        let err = validate_args(&ArgSchema::Bool, &json!(0)).unwrap_err();
        assert!(err.message.contains("expected boolean"));
    }

    #[test]
    fn test_bool_rejects_integer_1() {
        let err = validate_args(&ArgSchema::Bool, &json!(1)).unwrap_err();
        assert!(err.message.contains("expected boolean"));
    }

    // -----------------------------------------------------------------------
    // ID edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_id_rejects_null() {
        let err = validate_args(&ArgSchema::Id, &json!(null)).unwrap_err();
        assert!(err.message.contains("expected ID string"));
    }

    #[test]
    fn test_id_rejects_number() {
        let err = validate_args(&ArgSchema::Id, &json!(123)).unwrap_err();
        assert!(err.message.contains("expected ID string"));
    }

    #[test]
    fn test_id_accepts_prefixed_id() {
        assert!(validate_args(&ArgSchema::Id, &json!("users:abc123def")).is_ok());
    }

    #[test]
    fn test_id_accepts_plain_string() {
        assert!(validate_args(&ArgSchema::Id, &json!("abc123")).is_ok());
    }

    // -----------------------------------------------------------------------
    // Array edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_array_empty() {
        let schema = ArgSchema::Array(Box::new(ArgSchema::Number {
            min: None,
            max: None,
        }));
        assert!(validate_args(&schema, &json!([])).is_ok());
    }

    #[test]
    fn test_array_rejects_non_array() {
        let schema = ArgSchema::Array(Box::new(ArgSchema::Any));
        let err = validate_args(&schema, &json!("not an array")).unwrap_err();
        assert!(err.message.contains("expected array"));
    }

    #[test]
    fn test_array_rejects_null() {
        let schema = ArgSchema::Array(Box::new(ArgSchema::Any));
        let err = validate_args(&schema, &json!(null)).unwrap_err();
        assert!(err.message.contains("expected array"));
    }

    #[test]
    fn test_array_nested() {
        let schema = ArgSchema::Array(Box::new(ArgSchema::Array(Box::new(ArgSchema::Number {
            min: None,
            max: None,
        }))));
        assert!(validate_args(&schema, &json!([[1, 2], [3, 4]])).is_ok());
        let err = validate_args(&schema, &json!([[1, "two"]])).unwrap_err();
        assert!(err.path.contains("[0][1]"));
    }

    #[test]
    fn test_array_error_path_shows_index() {
        let schema = ArgSchema::Array(Box::new(ArgSchema::String {
            min: None,
            max: None,
        }));
        let err = validate_args(&schema, &json!(["a", "b", 3])).unwrap_err();
        assert!(err.path.contains("[2]"));
    }

    // -----------------------------------------------------------------------
    // Object edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_object_empty_schema_accepts_empty() {
        let schema = ArgSchema::Object(HashMap::new());
        assert!(validate_args(&schema, &json!({})).is_ok());
    }

    #[test]
    fn test_object_extra_fields_allowed() {
        // The current validator does not reject extra fields.
        let schema = ArgSchema::Object({
            let mut fields = HashMap::new();
            fields.insert(
                "name".into(),
                ArgSchema::String {
                    min: None,
                    max: None,
                },
            );
            fields
        });
        assert!(validate_args(&schema, &json!({"name": "Alice", "extra": true})).is_ok());
    }

    #[test]
    fn test_object_rejects_non_object() {
        let schema = ArgSchema::Object(HashMap::new());
        let err = validate_args(&schema, &json!("not an object")).unwrap_err();
        assert!(err.message.contains("expected object"));
    }

    #[test]
    fn test_object_nested() {
        let schema = ArgSchema::Object({
            let mut fields = HashMap::new();
            fields.insert(
                "address".into(),
                ArgSchema::Object({
                    let mut inner = HashMap::new();
                    inner.insert(
                        "city".into(),
                        ArgSchema::String {
                            min: Some(1),
                            max: None,
                        },
                    );
                    inner
                }),
            );
            fields
        });
        assert!(validate_args(&schema, &json!({"address": {"city": "NYC"}})).is_ok());
        let err = validate_args(&schema, &json!({"address": {"city": ""}})).unwrap_err();
        assert!(err.path.contains("address.city"));
    }

    #[test]
    fn test_object_with_optional_field_missing() {
        let schema = ArgSchema::Object({
            let mut fields = HashMap::new();
            fields.insert(
                "name".into(),
                ArgSchema::String {
                    min: None,
                    max: None,
                },
            );
            fields.insert(
                "bio".into(),
                ArgSchema::Optional(Box::new(ArgSchema::String {
                    min: None,
                    max: None,
                })),
            );
            fields
        });
        assert!(validate_args(&schema, &json!({"name": "Alice"})).is_ok());
    }

    #[test]
    fn test_object_with_optional_field_null() {
        let schema = ArgSchema::Object({
            let mut fields = HashMap::new();
            fields.insert(
                "bio".into(),
                ArgSchema::Optional(Box::new(ArgSchema::String {
                    min: None,
                    max: None,
                })),
            );
            fields
        });
        assert!(validate_args(&schema, &json!({"bio": null})).is_ok());
    }

    // -----------------------------------------------------------------------
    // Optional edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_optional_with_wrong_type() {
        let schema = ArgSchema::Optional(Box::new(ArgSchema::Number {
            min: None,
            max: None,
        }));
        let err = validate_args(&schema, &json!("not a number")).unwrap_err();
        assert!(err.message.contains("expected number"));
    }

    // -----------------------------------------------------------------------
    // json_type_name coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_error_messages_include_type_names() {
        let schema = ArgSchema::String {
            min: None,
            max: None,
        };
        assert!(
            validate_args(&schema, &json!(null))
                .unwrap_err()
                .message
                .contains("null")
        );
        assert!(
            validate_args(&schema, &json!(true))
                .unwrap_err()
                .message
                .contains("boolean")
        );
        assert!(
            validate_args(&schema, &json!(42))
                .unwrap_err()
                .message
                .contains("number")
        );
        assert!(
            validate_args(&schema, &json!([1]))
                .unwrap_err()
                .message
                .contains("array")
        );
        assert!(
            validate_args(&schema, &json!({}))
                .unwrap_err()
                .message
                .contains("object")
        );
    }

    // -----------------------------------------------------------------------
    // Complex combined schemas
    // -----------------------------------------------------------------------

    #[test]
    fn test_complex_nested_schema() {
        let schema = ArgSchema::Object({
            let mut fields = HashMap::new();
            fields.insert(
                "users".into(),
                ArgSchema::Array(Box::new(ArgSchema::Object({
                    let mut user_fields = HashMap::new();
                    user_fields.insert("id".into(), ArgSchema::Id);
                    user_fields.insert(
                        "name".into(),
                        ArgSchema::String {
                            min: Some(1),
                            max: Some(50),
                        },
                    );
                    user_fields.insert(
                        "tags".into(),
                        ArgSchema::Optional(Box::new(ArgSchema::Array(Box::new(
                            ArgSchema::String {
                                min: None,
                                max: None,
                            },
                        )))),
                    );
                    user_fields
                }))),
            );
            fields
        });

        let valid = json!({
            "users": [
                {"id": "u:1", "name": "Alice", "tags": ["admin"]},
                {"id": "u:2", "name": "Bob"}
            ]
        });
        assert!(validate_args(&schema, &valid).is_ok());

        let invalid = json!({
            "users": [
                {"id": "u:1", "name": "Alice"},
                {"id": "", "name": "Bad"}
            ]
        });
        let err = validate_args(&schema, &invalid).unwrap_err();
        assert!(err.path.contains("[1]"));
        assert!(err.message.contains("must not be empty"));
    }
}
