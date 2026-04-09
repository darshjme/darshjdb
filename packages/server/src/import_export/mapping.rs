//! Field mapping utilities for import operations.
//!
//! Provides fuzzy matching of CSV headers to existing EAV attributes,
//! automatic type inference from sample values, and import preview
//! generation.

use std::collections::HashMap;

use serde_json::Value;

use crate::triple_store::schema::ValueType;

// ── Auto-mapping ──────────────────────────────────────────────────────

/// Fuzzy-match CSV column headers to existing EAV attribute names.
///
/// Returns a map from column index to the matched attribute name.
/// Matching strategy (in order of precedence):
/// 1. Exact match (case-insensitive).
/// 2. Suffix match — the attribute ends with `/<header>` (e.g. `user/email` matches `email`).
/// 3. Normalized match — underscores/hyphens/spaces collapsed, case-insensitive.
///
/// Columns that don't match any attribute are omitted from the result.
pub fn auto_map_csv_headers(
    headers: &[String],
    existing_attributes: &[String],
) -> HashMap<usize, String> {
    let mut result = HashMap::new();

    for (idx, header) in headers.iter().enumerate() {
        let header_lower = header.trim().to_lowercase();
        let header_norm = normalize(&header_lower);

        // 1. Exact case-insensitive match.
        if let Some(attr) = existing_attributes
            .iter()
            .find(|a| a.to_lowercase() == header_lower)
        {
            result.insert(idx, attr.clone());
            continue;
        }

        // 2. Suffix match: attribute ends with `/<header>`.
        if let Some(attr) = existing_attributes
            .iter()
            .find(|a| a.to_lowercase().ends_with(&format!("/{}", header_lower)))
        {
            result.insert(idx, attr.clone());
            continue;
        }

        // 3. Normalized match (strip underscores/hyphens/spaces).
        if let Some(attr) = existing_attributes
            .iter()
            .find(|a| normalize(&a.to_lowercase()) == header_norm)
        {
            result.insert(idx, attr.clone());
            continue;
        }
    }

    result
}

/// Normalize a string by removing underscores, hyphens, and spaces.
fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| *c != '_' && *c != '-' && *c != ' ')
        .collect()
}

// ── Type inference ────────────────────────────────────────────────────

/// Infer [`ValueType`] for each column from a sample of rows.
///
/// For each column position, examines all non-empty sample values and
/// picks the most specific type that fits all of them. Precedence
/// (most to least specific): Boolean > Integer > Float > Timestamp > String.
pub fn infer_types(sample: &[Vec<String>]) -> Vec<ValueType> {
    if sample.is_empty() {
        return Vec::new();
    }

    let num_cols = sample.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut types = Vec::with_capacity(num_cols);

    for col in 0..num_cols {
        let values: Vec<&str> = sample
            .iter()
            .filter_map(|row| row.get(col).map(|s| s.as_str()))
            .filter(|s| !s.is_empty())
            .collect();

        types.push(infer_column_type(&values));
    }

    types
}

/// Infer the type of a single column from its non-empty values.
fn infer_column_type(values: &[&str]) -> ValueType {
    if values.is_empty() {
        return ValueType::String;
    }

    // Check if all values are booleans.
    if values.iter().all(|v| {
        matches!(
            v.to_lowercase().as_str(),
            "true" | "false" | "1" | "0" | "yes" | "no"
        )
    }) {
        return ValueType::Boolean;
    }

    // Check if all values are integers.
    if values.iter().all(|v| v.parse::<i64>().is_ok()) {
        return ValueType::Integer;
    }

    // Check if all values are floats.
    if values.iter().all(|v| v.parse::<f64>().is_ok()) {
        return ValueType::Float;
    }

    // Check if all values are RFC 3339 timestamps.
    if values.iter().all(|v| {
        chrono::DateTime::parse_from_rfc3339(v).is_ok()
            || chrono::NaiveDate::parse_from_str(v, "%Y-%m-%d").is_ok()
    }) {
        return ValueType::Timestamp;
    }

    ValueType::String
}

/// Infer the [`ValueType`] discriminator (`i16`) for a single string value.
pub fn infer_value_type_from_str(value: &str) -> (Value, i16) {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        return (Value::Null, ValueType::String as i16);
    }

    // Boolean
    match trimmed.to_lowercase().as_str() {
        "true" | "yes" => return (Value::Bool(true), ValueType::Boolean as i16),
        "false" | "no" => return (Value::Bool(false), ValueType::Boolean as i16),
        _ => {}
    }

    // Integer
    if let Ok(n) = trimmed.parse::<i64>() {
        return (Value::Number(n.into()), ValueType::Integer as i16);
    }

    // Float
    if let Ok(n) = trimmed.parse::<f64>()
        && let Some(num) = serde_json::Number::from_f64(n)
    {
        return (Value::Number(num), ValueType::Float as i16);
    }

    // Timestamp (RFC 3339)
    if chrono::DateTime::parse_from_rfc3339(trimmed).is_ok() {
        return (
            Value::String(trimmed.to_string()),
            ValueType::Timestamp as i16,
        );
    }

    // Date (YYYY-MM-DD)
    if chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").is_ok() {
        return (
            Value::String(trimmed.to_string()),
            ValueType::Timestamp as i16,
        );
    }

    // UUID (reference)
    if trimmed.len() == 36 && uuid::Uuid::parse_str(trimmed).is_ok() {
        return (
            Value::String(trimmed.to_string()),
            ValueType::Reference as i16,
        );
    }

    // JSON object or array
    if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
        && let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
    {
        return (parsed, ValueType::Json as i16);
    }

    // Default: string
    (Value::String(trimmed.to_string()), ValueType::String as i16)
}

// ── Preview generation ────────────────────────────────────────────────

/// Generate a preview of how the first N rows would be imported.
///
/// Each row is returned as a `HashMap<attribute_name, JSON_value>`,
/// showing the caller exactly what triples would be created.
pub fn generate_import_preview(
    sample: &[Vec<String>],
    mapping: &HashMap<usize, String>,
) -> Vec<HashMap<String, Value>> {
    sample
        .iter()
        .map(|row| {
            let mut record = HashMap::new();
            for (col_idx, attr_name) in mapping {
                if let Some(raw) = row.get(*col_idx) {
                    let (value, _type_tag) = infer_value_type_from_str(raw);
                    record.insert(attr_name.clone(), value);
                }
            }
            record
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_map_exact_match() {
        let headers = vec!["email".into(), "name".into(), "unknown_col".into()];
        let attrs = vec!["email".into(), "name".into(), "age".into()];
        let mapped = auto_map_csv_headers(&headers, &attrs);
        assert_eq!(mapped.get(&0), Some(&"email".to_string()));
        assert_eq!(mapped.get(&1), Some(&"name".to_string()));
        assert!(mapped.get(&2).is_none());
    }

    #[test]
    fn auto_map_suffix_match() {
        let headers = vec!["email".into()];
        let attrs = vec!["user/email".into()];
        let mapped = auto_map_csv_headers(&headers, &attrs);
        assert_eq!(mapped.get(&0), Some(&"user/email".to_string()));
    }

    #[test]
    fn auto_map_normalized_match() {
        let headers = vec!["first_name".into()];
        let attrs = vec!["first-name".into()];
        let mapped = auto_map_csv_headers(&headers, &attrs);
        assert_eq!(mapped.get(&0), Some(&"first-name".to_string()));
    }

    #[test]
    fn infer_types_boolean_column() {
        let sample = vec![
            vec!["true".into(), "42".into()],
            vec!["false".into(), "7".into()],
        ];
        let types = infer_types(&sample);
        assert_eq!(types[0], ValueType::Boolean);
        assert_eq!(types[1], ValueType::Integer);
    }

    #[test]
    fn infer_types_float_column() {
        let sample = vec![vec!["3.14".into()], vec!["2.71".into()]];
        let types = infer_types(&sample);
        assert_eq!(types[0], ValueType::Float);
    }

    #[test]
    fn infer_types_timestamp_column() {
        let sample = vec![
            vec!["2024-01-15T10:30:00Z".into()],
            vec!["2024-06-20T08:00:00+05:30".into()],
        ];
        let types = infer_types(&sample);
        assert_eq!(types[0], ValueType::Timestamp);
    }

    #[test]
    fn infer_value_type_from_str_variants() {
        let (v, t) = infer_value_type_from_str("true");
        assert_eq!(v, Value::Bool(true));
        assert_eq!(t, ValueType::Boolean as i16);

        let (v, t) = infer_value_type_from_str("42");
        assert_eq!(v, Value::Number(42.into()));
        assert_eq!(t, ValueType::Integer as i16);

        let (v, t) = infer_value_type_from_str("3.14");
        assert_eq!(t, ValueType::Float as i16);
        assert!(v.is_number());

        let (v, t) = infer_value_type_from_str("hello world");
        assert_eq!(v, Value::String("hello world".into()));
        assert_eq!(t, ValueType::String as i16);
    }

    #[test]
    fn generate_preview_applies_mapping() {
        let sample = vec![
            vec!["alice".into(), "30".into(), "true".into()],
            vec!["bob".into(), "25".into(), "false".into()],
        ];
        let mut mapping = HashMap::new();
        mapping.insert(0, "user/name".into());
        mapping.insert(1, "user/age".into());
        // Column 2 intentionally unmapped.

        let preview = generate_import_preview(&sample, &mapping);
        assert_eq!(preview.len(), 2);
        assert_eq!(preview[0]["user/name"], Value::String("alice".into()));
        assert_eq!(preview[0]["user/age"], Value::Number(30.into()));
        assert!(preview[0].get("user/active").is_none());
    }
}
