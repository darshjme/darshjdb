//! Type conversion for field values.
//!
//! When a field's type changes (e.g. Number -> Text), all existing
//! values must be converted. [`convert_field_type`] performs batch
//! conversion with best-effort casting and reports failures.

use serde_json::Value;

use super::FieldType;

// ── Public API ─────────────────────────────────────────────────────

/// Result of converting a single value.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversionResult {
    /// The converted value, or `None` if conversion was lossy/impossible.
    pub value: Option<Value>,
    /// Warning message if the conversion was lossy.
    pub warning: Option<String>,
}

/// Batch-convert values from one field type to another.
///
/// Returns one [`ConversionResult`] per input value. Lossless
/// conversions produce `value = Some(...)` with no warning. Lossy
/// conversions produce `value = None` and a descriptive warning.
pub fn convert_field_type(
    values: &[Value],
    from: FieldType,
    to: FieldType,
) -> Vec<ConversionResult> {
    if from == to {
        return values
            .iter()
            .map(|v| ConversionResult {
                value: Some(v.clone()),
                warning: None,
            })
            .collect();
    }

    values.iter().map(|v| convert_single(v, from, to)).collect()
}

/// Summary of a batch conversion.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversionSummary {
    /// Total values processed.
    pub total: usize,
    /// Successfully converted.
    pub success: usize,
    /// Failed (lossy) conversions.
    pub failed: usize,
    /// All warnings collected.
    pub warnings: Vec<String>,
}

/// Summarise conversion results.
pub fn summarise(results: &[ConversionResult]) -> ConversionSummary {
    let total = results.len();
    let success = results.iter().filter(|r| r.value.is_some()).count();
    let failed = total - success;
    let warnings = results.iter().filter_map(|r| r.warning.clone()).collect();
    ConversionSummary {
        total,
        success,
        failed,
        warnings,
    }
}

// ── Single-value conversion ────────────────────────────────────────

fn convert_single(value: &Value, from: FieldType, to: FieldType) -> ConversionResult {
    if value.is_null() {
        return ConversionResult {
            value: Some(Value::Null),
            warning: None,
        };
    }

    let result = match (from, to) {
        // ── To text (always lossless) ──────────────────────────────
        (_, FieldType::SingleLineText | FieldType::LongText) => Some(to_text(value)),

        // ── Text → Number ──────────────────────────────────────────
        (
            FieldType::SingleLineText | FieldType::LongText,
            FieldType::Number | FieldType::Currency | FieldType::Percent,
        ) => text_to_number(value),

        // ── Number → Number-like ───────────────────────────────────
        (FieldType::Number, FieldType::Currency | FieldType::Percent) => Some(value.clone()),
        (FieldType::Currency | FieldType::Percent, FieldType::Number) => Some(value.clone()),
        (FieldType::Currency, FieldType::Percent) | (FieldType::Percent, FieldType::Currency) => {
            Some(value.clone())
        }

        // ── Number → Checkbox ──────────────────────────────────────
        (FieldType::Number, FieldType::Checkbox) => {
            let n = value.as_f64().unwrap_or(0.0);
            Some(Value::Bool(n != 0.0))
        }

        // ── Checkbox → Number ──────────────────────────────────────
        (FieldType::Checkbox, FieldType::Number) => {
            let b = value.as_bool().unwrap_or(false);
            Some(serde_json::json!(if b { 1 } else { 0 }))
        }

        // ── Text → Checkbox ────────────────────────────────────────
        (FieldType::SingleLineText | FieldType::LongText, FieldType::Checkbox) => {
            text_to_bool(value)
        }

        // ── Text → Date/DateTime ───────────────────────────────────
        (FieldType::SingleLineText | FieldType::LongText, FieldType::Date) => text_to_date(value),
        (FieldType::SingleLineText | FieldType::LongText, FieldType::DateTime) => {
            text_to_datetime(value)
        }

        // ── Date ↔ DateTime ────────────────────────────────────────
        (FieldType::Date, FieldType::DateTime) => value.as_str().map(|s| {
            if s.contains('T') {
                Value::String(s.to_string())
            } else {
                Value::String(format!("{s}T00:00:00Z"))
            }
        }),
        (FieldType::DateTime, FieldType::Date) => value.as_str().map(|s| {
            let date_part = s.split('T').next().unwrap_or(s);
            Value::String(date_part.to_string())
        }),

        // ── Text → Email/URL/Phone ─────────────────────────────────
        (
            FieldType::SingleLineText | FieldType::LongText,
            FieldType::Email | FieldType::Url | FieldType::Phone,
        ) => {
            // Pass through -- validation happens at the field level.
            Some(value.clone())
        }

        // ── SingleSelect → MultiSelect ─────────────────────────────
        (FieldType::SingleSelect, FieldType::MultiSelect) => {
            Some(Value::Array(vec![value.clone()]))
        }

        // ── MultiSelect → SingleSelect ─────────────────────────────
        (FieldType::MultiSelect, FieldType::SingleSelect) => {
            value.as_array().and_then(|arr| arr.first().cloned())
        }

        // ── Number → Rating ────────────────────────────────────────
        (FieldType::Number, FieldType::Rating) => value.as_f64().map(|n| {
            let clamped = n.round().max(0.0).min(5.0) as u64;
            serde_json::json!(clamped)
        }),

        // ── Rating → Number ────────────────────────────────────────
        (FieldType::Rating, FieldType::Number) => Some(value.clone()),

        // ── Text → SingleSelect ────────────────────────────────────
        (FieldType::SingleLineText | FieldType::LongText, FieldType::SingleSelect) => {
            Some(value.clone())
        }

        // ── Text → MultiSelect ─────────────────────────────────────
        (FieldType::SingleLineText | FieldType::LongText, FieldType::MultiSelect) => {
            // Wrap string as single-element array.
            if value.is_string() {
                Some(Value::Array(vec![value.clone()]))
            } else {
                None
            }
        }

        // ── All other conversions are lossy ─────────────────────────
        _ => None,
    };

    match result {
        Some(v) => ConversionResult {
            value: Some(v),
            warning: None,
        },
        None => ConversionResult {
            value: None,
            warning: Some(format!(
                "cannot convert {} -> {}: value {:?}",
                from.label(),
                to.label(),
                truncate_for_warning(value),
            )),
        },
    }
}

// ── Helpers ────────────────────────────────────────────────────────

fn to_text(value: &Value) -> Value {
    match value {
        Value::String(_) => value.clone(),
        Value::Number(n) => Value::String(n.to_string()),
        Value::Bool(b) => Value::String(b.to_string()),
        Value::Null => Value::String(String::new()),
        Value::Array(arr) => {
            let items: Vec<String> = arr
                .iter()
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            Value::String(items.join(", "))
        }
        Value::Object(_) => Value::String(value.to_string()),
    }
}

fn text_to_number(value: &Value) -> Option<Value> {
    let s = value.as_str()?;
    let trimmed = s.trim();

    // Strip currency symbols and commas.
    let cleaned: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+')
        .collect();

    cleaned
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .map(|n| serde_json::json!(n))
}

fn text_to_bool(value: &Value) -> Option<Value> {
    let s = value.as_str()?;
    match s.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(Value::Bool(true)),
        "false" | "0" | "no" | "off" | "" => Some(Value::Bool(false)),
        _ => None,
    }
}

fn text_to_date(value: &Value) -> Option<Value> {
    let s = value.as_str()?;
    // Try ISO format YYYY-MM-DD.
    let trimmed = s.trim();
    if trimmed.len() >= 10 && trimmed.as_bytes()[4] == b'-' && trimmed.as_bytes()[7] == b'-' {
        let date = &trimmed[..10];
        return Some(Value::String(date.to_string()));
    }
    None
}

fn text_to_datetime(value: &Value) -> Option<Value> {
    let s = value.as_str()?;
    let trimmed = s.trim();
    // Accept anything that looks like a date with T.
    if trimmed.contains('T') && trimmed.len() >= 10 {
        return Some(Value::String(trimmed.to_string()));
    }
    // Try date-only and append midnight.
    text_to_date(value).map(|v| {
        let d = v.as_str().unwrap();
        Value::String(format!("{d}T00:00:00Z"))
    })
}

fn truncate_for_warning(value: &Value) -> String {
    let s = value.to_string();
    if s.len() > 50 {
        format!("{}...", &s[..47])
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identity_conversion() {
        let values = vec![json!(1), json!(2), json!(3)];
        let results = convert_field_type(&values, FieldType::Number, FieldType::Number);
        assert!(
            results
                .iter()
                .all(|r| r.value.is_some() && r.warning.is_none())
        );
    }

    #[test]
    fn number_to_text() {
        let values = vec![json!(42), json!(3.14)];
        let results = convert_field_type(&values, FieldType::Number, FieldType::SingleLineText);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!("42"));
        assert_eq!(results[1].value.as_ref().unwrap(), &json!("3.14"));
    }

    #[test]
    fn text_to_number_valid() {
        let values = vec![json!("123"), json!("45.6")];
        let results = convert_field_type(&values, FieldType::SingleLineText, FieldType::Number);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!(123.0));
        assert_eq!(results[1].value.as_ref().unwrap(), &json!(45.6));
    }

    #[test]
    fn text_to_number_invalid() {
        let values = vec![json!("not a number")];
        let results = convert_field_type(&values, FieldType::SingleLineText, FieldType::Number);
        assert!(results[0].value.is_none());
        assert!(results[0].warning.is_some());
    }

    #[test]
    fn bool_to_text() {
        let values = vec![json!(true), json!(false)];
        let results = convert_field_type(&values, FieldType::Checkbox, FieldType::SingleLineText);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!("true"));
        assert_eq!(results[1].value.as_ref().unwrap(), &json!("false"));
    }

    #[test]
    fn text_to_bool() {
        let values = vec![json!("yes"), json!("false"), json!("maybe")];
        let results = convert_field_type(&values, FieldType::SingleLineText, FieldType::Checkbox);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!(true));
        assert_eq!(results[1].value.as_ref().unwrap(), &json!(false));
        assert!(results[2].value.is_none());
    }

    #[test]
    fn number_to_checkbox() {
        let values = vec![json!(0), json!(1), json!(42)];
        let results = convert_field_type(&values, FieldType::Number, FieldType::Checkbox);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!(false));
        assert_eq!(results[1].value.as_ref().unwrap(), &json!(true));
        assert_eq!(results[2].value.as_ref().unwrap(), &json!(true));
    }

    #[test]
    fn checkbox_to_number() {
        let values = vec![json!(true), json!(false)];
        let results = convert_field_type(&values, FieldType::Checkbox, FieldType::Number);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!(1));
        assert_eq!(results[1].value.as_ref().unwrap(), &json!(0));
    }

    #[test]
    fn date_to_datetime() {
        let values = vec![json!("2025-01-15")];
        let results = convert_field_type(&values, FieldType::Date, FieldType::DateTime);
        assert_eq!(
            results[0].value.as_ref().unwrap(),
            &json!("2025-01-15T00:00:00Z")
        );
    }

    #[test]
    fn datetime_to_date() {
        let values = vec![json!("2025-01-15T10:30:00Z")];
        let results = convert_field_type(&values, FieldType::DateTime, FieldType::Date);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!("2025-01-15"));
    }

    #[test]
    fn single_select_to_multi_select() {
        let values = vec![json!("Active")];
        let results = convert_field_type(&values, FieldType::SingleSelect, FieldType::MultiSelect);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!(["Active"]));
    }

    #[test]
    fn multi_select_to_single_select() {
        let values = vec![json!(["A", "B"])];
        let results = convert_field_type(&values, FieldType::MultiSelect, FieldType::SingleSelect);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!("A"));
    }

    #[test]
    fn number_to_rating() {
        let values = vec![json!(3.7), json!(-1.0), json!(10.0)];
        let results = convert_field_type(&values, FieldType::Number, FieldType::Rating);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!(4));
        assert_eq!(results[1].value.as_ref().unwrap(), &json!(0));
        assert_eq!(results[2].value.as_ref().unwrap(), &json!(5));
    }

    #[test]
    fn null_values_pass_through() {
        let values = vec![Value::Null];
        let results = convert_field_type(&values, FieldType::Number, FieldType::SingleLineText);
        assert_eq!(results[0].value.as_ref().unwrap(), &Value::Null);
    }

    #[test]
    fn lossy_conversion_attachment_to_number() {
        let values = vec![json!({"url": "https://x.com/f.pdf"})];
        let results = convert_field_type(&values, FieldType::Attachment, FieldType::Number);
        assert!(results[0].value.is_none());
        assert!(results[0].warning.is_some());
    }

    #[test]
    fn summarise_mixed_results() {
        let results = vec![
            ConversionResult {
                value: Some(json!(1)),
                warning: None,
            },
            ConversionResult {
                value: None,
                warning: Some("failed".into()),
            },
            ConversionResult {
                value: Some(json!(3)),
                warning: None,
            },
        ];
        let summary = summarise(&results);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.success, 2);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.warnings.len(), 1);
    }

    #[test]
    fn text_to_number_strips_currency() {
        // text_to_number helper strips non-numeric chars.
        let result = text_to_number(&json!("$1,234.56"));
        assert!(result.is_some());
        assert_eq!(result.unwrap(), json!(1234.56));
    }

    #[test]
    fn to_text_array() {
        let v = json!(["a", "b", "c"]);
        let result = to_text(&v);
        assert_eq!(result, json!("a, b, c"));
    }

    #[test]
    fn number_currency_interchangeable() {
        let values = vec![json!(100.50)];
        let r1 = convert_field_type(&values, FieldType::Number, FieldType::Currency);
        assert_eq!(r1[0].value.as_ref().unwrap(), &json!(100.50));

        let r2 = convert_field_type(&values, FieldType::Currency, FieldType::Number);
        assert_eq!(r2[0].value.as_ref().unwrap(), &json!(100.50));
    }

    #[test]
    fn text_to_select() {
        let values = vec![json!("Active")];
        let results =
            convert_field_type(&values, FieldType::SingleLineText, FieldType::SingleSelect);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!("Active"));
    }

    #[test]
    fn text_to_multi_select_wraps() {
        let values = vec![json!("Tag1")];
        let results =
            convert_field_type(&values, FieldType::SingleLineText, FieldType::MultiSelect);
        assert_eq!(results[0].value.as_ref().unwrap(), &json!(["Tag1"]));
    }
}
