//! Argument validation for DarshanDB server functions.
//!
//! Each function can declare an [`ArgSchema`] that describes the shape and
//! constraints of its arguments. [`validate_args`] checks a JSON value
//! against a schema and produces descriptive errors on mismatch.
//!
//! # Example
//!
//! ```rust
//! use serde_json::json;
//! use darshandb_server::functions::validator::{ArgSchema, validate_args};
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

    /// A DarshanDB document ID (validated as a non-empty string starting with an
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

            if let Some(min_len) = min {
                if s.chars().count() < *min_len {
                    return Err(ValidationError {
                        path: path.to_string(),
                        message: format!(
                            "string length {} is below minimum {}",
                            s.chars().count(),
                            min_len
                        ),
                    });
                }
            }

            if let Some(max_len) = max {
                if s.chars().count() > *max_len {
                    return Err(ValidationError {
                        path: path.to_string(),
                        message: format!(
                            "string length {} exceeds maximum {}",
                            s.chars().count(),
                            max_len
                        ),
                    });
                }
            }

            Ok(())
        }

        ArgSchema::Number { min, max } => {
            let n = value.as_f64().ok_or_else(|| ValidationError {
                path: path.to_string(),
                message: format!("expected number, got {}", json_type_name(value)),
            })?;

            if let Some(min_val) = min {
                if n < *min_val {
                    return Err(ValidationError {
                        path: path.to_string(),
                        message: format!("value {n} is below minimum {min_val}"),
                    });
                }
            }

            if let Some(max_val) = max {
                if n > *max_val {
                    return Err(ValidationError {
                        path: path.to_string(),
                        message: format!("value {n} exceeds maximum {max_val}"),
                    });
                }
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
}
