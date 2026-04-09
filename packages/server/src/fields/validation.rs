//! Field-level value validation and coercion.
//!
//! [`validate_value`] checks that a JSON value conforms to a field's
//! type and options, applying lightweight coercion where safe (e.g.
//! trimming whitespace from emails, normalising date formats).

use serde_json::Value;

use crate::error::{DarshJError, Result};

use super::{FieldConfig, FieldOptions, FieldType};

// ── Public API ─────────────────────────────────────────────────────

/// Validate and coerce `value` according to the field definition.
///
/// Returns the (possibly coerced) value on success, or a descriptive
/// error when validation fails.
///
/// # Coercion Rules
///
/// - Strings are trimmed for email/url/phone fields.
/// - Numbers supplied as strings are parsed.
/// - Booleans accept `0`/`1` and `"true"`/`"false"`.
/// - Dates accept multiple common formats and normalise to RFC 3339.
/// - Select values are checked against the configured choices.
pub fn validate_value(field: &FieldConfig, value: &Value) -> Result<Value> {
    // Null handling: if the field is required, null is rejected;
    // otherwise null passes through as-is.
    if value.is_null() {
        if field.required {
            return Err(DarshJError::InvalidAttribute(format!(
                "field '{}' is required",
                field.name
            )));
        }
        return Ok(Value::Null);
    }

    match field.field_type {
        FieldType::SingleLineText => validate_single_line_text(value),
        FieldType::LongText => validate_long_text(value),
        FieldType::Number => validate_number(value, field.options.as_ref()),
        FieldType::Checkbox => validate_checkbox(value),
        FieldType::Date => validate_date(value),
        FieldType::DateTime => validate_datetime(value),
        FieldType::Email => validate_email(value),
        FieldType::Url => validate_url(value),
        FieldType::Phone => validate_phone(value),
        FieldType::Currency => validate_currency(value, field.options.as_ref()),
        FieldType::Percent => validate_percent(value),
        FieldType::Duration => validate_duration(value),
        FieldType::Rating => validate_rating(value, field.options.as_ref()),
        FieldType::SingleSelect => validate_single_select(value, field.options.as_ref()),
        FieldType::MultiSelect => validate_multi_select(value, field.options.as_ref()),
        FieldType::Attachment => validate_attachment(value),
        FieldType::Link => validate_link(value),

        // Computed fields should not accept user input.
        ft if ft.is_computed() => Err(DarshJError::InvalidAttribute(format!(
            "field '{}' is computed and cannot be set directly",
            field.name
        ))),

        // Catch-all for any future types -- accept as-is.
        _ => Ok(value.clone()),
    }
}

// ── Validators ─────────────────────────────────────────────────────

fn validate_single_line_text(value: &Value) -> Result<Value> {
    match value {
        Value::String(s) => {
            if s.contains('\n') {
                return Err(DarshJError::InvalidAttribute(
                    "single-line text must not contain newlines".into(),
                ));
            }
            Ok(value.clone())
        }
        // Coerce numbers/bools to string.
        Value::Number(n) => Ok(Value::String(n.to_string())),
        Value::Bool(b) => Ok(Value::String(b.to_string())),
        _ => Err(type_err("string", value)),
    }
}

fn validate_long_text(value: &Value) -> Result<Value> {
    match value {
        Value::String(_) => Ok(value.clone()),
        Value::Number(n) => Ok(Value::String(n.to_string())),
        Value::Bool(b) => Ok(Value::String(b.to_string())),
        _ => Err(type_err("string", value)),
    }
}

fn validate_number(value: &Value, options: Option<&FieldOptions>) -> Result<Value> {
    let n = coerce_to_f64(value)?;

    let precision = match options {
        Some(FieldOptions::Number { precision, .. }) => *precision,
        _ => 10, // default: no truncation
    };

    let factor = 10_f64.powi(precision as i32);
    let rounded = (n * factor).round() / factor;
    Ok(serde_json::json!(rounded))
}

fn validate_checkbox(value: &Value) -> Result<Value> {
    match value {
        Value::Bool(_) => Ok(value.clone()),
        Value::Number(n) => {
            let v = n.as_f64().unwrap_or(0.0);
            Ok(Value::Bool(v != 0.0))
        }
        Value::String(s) => match s.to_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(Value::Bool(true)),
            "false" | "0" | "no" => Ok(Value::Bool(false)),
            _ => Err(type_err("boolean", value)),
        },
        _ => Err(type_err("boolean", value)),
    }
}

fn validate_date(value: &Value) -> Result<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| type_err("date string", value))?;
    let parsed = parse_date(s)?;
    Ok(Value::String(parsed))
}

fn validate_datetime(value: &Value) -> Result<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| type_err("datetime string", value))?;
    let parsed = parse_datetime(s)?;
    Ok(Value::String(parsed))
}

fn validate_email(value: &Value) -> Result<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| type_err("email string", value))?;
    let trimmed = s.trim().to_lowercase();

    // Minimal email validation: must have exactly one `@` with
    // non-empty local and domain parts, and domain must contain a dot.
    let parts: Vec<&str> = trimmed.splitn(2, '@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(DarshJError::InvalidAttribute(format!(
            "invalid email address: '{s}'"
        )));
    }
    let domain = parts[1];
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return Err(DarshJError::InvalidAttribute(format!(
            "invalid email domain: '{domain}'"
        )));
    }

    Ok(Value::String(trimmed))
}

fn validate_url(value: &Value) -> Result<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| type_err("URL string", value))?;
    let trimmed = s.trim();

    // Must start with a valid scheme.
    if !trimmed.starts_with("http://")
        && !trimmed.starts_with("https://")
        && !trimmed.starts_with("ftp://")
    {
        return Err(DarshJError::InvalidAttribute(format!(
            "URL must start with http://, https://, or ftp://: '{trimmed}'"
        )));
    }

    // Must have a host after the scheme.
    let after_scheme = if trimmed.starts_with("https://") {
        &trimmed[8..]
    } else if trimmed.starts_with("http://") {
        &trimmed[7..]
    } else {
        &trimmed[6..]
    };

    if after_scheme.is_empty() || after_scheme.starts_with('/') {
        return Err(DarshJError::InvalidAttribute(format!(
            "URL missing host: '{trimmed}'"
        )));
    }

    Ok(Value::String(trimmed.to_string()))
}

fn validate_phone(value: &Value) -> Result<Value> {
    let s = value
        .as_str()
        .ok_or_else(|| type_err("phone string", value))?;
    let trimmed = s.trim();

    // Strip all non-digit characters except leading +.
    let digits: String = trimmed
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect();

    if digits.is_empty() {
        return Err(DarshJError::InvalidAttribute(format!(
            "invalid phone number: '{s}'"
        )));
    }

    // Must have at least 7 digits (shortest valid phone numbers).
    let digit_count = digits.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count < 7 {
        return Err(DarshJError::InvalidAttribute(format!(
            "phone number too short (need >= 7 digits): '{s}'"
        )));
    }

    Ok(Value::String(digits))
}

fn validate_currency(value: &Value, options: Option<&FieldOptions>) -> Result<Value> {
    let n = coerce_to_f64(value)?;

    let precision = match options {
        Some(FieldOptions::Currency { precision, .. }) => *precision,
        _ => 2,
    };

    let factor = 10_f64.powi(precision as i32);
    let rounded = (n * factor).round() / factor;
    Ok(serde_json::json!(rounded))
}

fn validate_percent(value: &Value) -> Result<Value> {
    let n = coerce_to_f64(value)?;
    Ok(serde_json::json!(n))
}

fn validate_duration(value: &Value) -> Result<Value> {
    // Accept number (seconds) or string.
    match value {
        Value::Number(_) => {
            let n = coerce_to_f64(value)?;
            if n < 0.0 {
                return Err(DarshJError::InvalidAttribute(
                    "duration cannot be negative".into(),
                ));
            }
            Ok(serde_json::json!(n))
        }
        Value::String(_) => Ok(value.clone()),
        _ => Err(type_err("number or string", value)),
    }
}

fn validate_rating(value: &Value, options: Option<&FieldOptions>) -> Result<Value> {
    let n = coerce_to_f64(value)?;
    let max = match options {
        Some(FieldOptions::Rating { max, .. }) => *max as f64,
        _ => 5.0,
    };

    if n < 0.0 || n > max {
        return Err(DarshJError::InvalidAttribute(format!(
            "rating must be between 0 and {max}"
        )));
    }

    // Round to integer.
    Ok(serde_json::json!(n.round() as u64))
}

fn validate_single_select(value: &Value, options: Option<&FieldOptions>) -> Result<Value> {
    let s = value.as_str().ok_or_else(|| type_err("string", value))?;

    if let Some(FieldOptions::Select { choices }) = options
        && !choices.iter().any(|c| c.name == s || c.id == s)
    {
        return Err(DarshJError::InvalidAttribute(format!(
            "value '{s}' is not a valid choice"
        )));
    }

    Ok(value.clone())
}

fn validate_multi_select(value: &Value, options: Option<&FieldOptions>) -> Result<Value> {
    let arr = value
        .as_array()
        .ok_or_else(|| type_err("array of strings", value))?;

    if let Some(FieldOptions::Select { choices }) = options {
        for item in arr {
            let s = item
                .as_str()
                .ok_or_else(|| type_err("string element", item))?;
            if !choices.iter().any(|c| c.name == s || c.id == s) {
                return Err(DarshJError::InvalidAttribute(format!(
                    "value '{s}' is not a valid choice"
                )));
            }
        }
    }

    Ok(value.clone())
}

fn validate_attachment(value: &Value) -> Result<Value> {
    // Attachments are stored as JSON objects or arrays of objects.
    match value {
        Value::Object(_) | Value::Array(_) => Ok(value.clone()),
        _ => Err(type_err("object or array", value)),
    }
}

fn validate_link(value: &Value) -> Result<Value> {
    // Links are UUIDs (strings) or arrays of UUIDs.
    match value {
        Value::String(_) | Value::Array(_) => Ok(value.clone()),
        _ => Err(type_err("string UUID or array of UUIDs", value)),
    }
}

// ── Helpers ────────────────────────────────────────────────────────

fn coerce_to_f64(value: &Value) -> Result<f64> {
    match value {
        Value::Number(n) => n.as_f64().ok_or_else(|| type_err("finite number", value)),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| DarshJError::TypeMismatch {
                expected: "number".into(),
                actual: format!("string '{s}'"),
            }),
        _ => Err(type_err("number", value)),
    }
}

fn type_err(expected: &str, actual: &Value) -> DarshJError {
    DarshJError::TypeMismatch {
        expected: expected.into(),
        actual: json_type_name(actual).into(),
    }
}

fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Parse a date string into `YYYY-MM-DD` format.
///
/// Accepted formats:
/// - `YYYY-MM-DD`
/// - `MM/DD/YYYY`
/// - `DD.MM.YYYY`
/// - `YYYY/MM/DD`
fn parse_date(s: &str) -> Result<String> {
    let trimmed = s.trim();

    // Try YYYY-MM-DD
    if let Some((y, rest)) = try_split_date(trimmed, '-')
        && y.len() == 4
        && let Some((m, d)) = try_split_date(rest, '-')
    {
        return validate_ymd(y, m, d);
    }

    // Try YYYY/MM/DD
    if let Some((y, rest)) = try_split_date(trimmed, '/')
        && y.len() == 4
        && let Some((m, d)) = try_split_date(rest, '/')
    {
        return validate_ymd(y, m, d);
    }

    // Try MM/DD/YYYY
    if let Some((m, rest)) = try_split_date(trimmed, '/')
        && m.len() <= 2
        && let Some((d, y)) = try_split_date(rest, '/')
        && y.len() == 4
    {
        return validate_ymd(y, m, d);
    }

    // Try DD.MM.YYYY
    if let Some((d, rest)) = try_split_date(trimmed, '.')
        && d.len() <= 2
        && let Some((m, y)) = try_split_date(rest, '.')
        && y.len() == 4
    {
        return validate_ymd(y, m, d);
    }

    Err(DarshJError::InvalidAttribute(format!(
        "unable to parse date: '{trimmed}'"
    )))
}

/// Parse a datetime string into RFC 3339 format.
fn parse_datetime(s: &str) -> Result<String> {
    let trimmed = s.trim();

    // If it already contains a T or space separator, try splitting.
    if let Some(pos) = trimmed.find('T').or_else(|| trimmed.find(' ')) {
        let date_part = &trimmed[..pos];
        let time_part = &trimmed[pos + 1..];
        let date = parse_date(date_part)?;

        // Validate time has at least HH:MM.
        let time_trimmed = time_part.trim_end_matches('Z');
        let parts: Vec<&str> = time_trimmed.split(':').collect();
        if parts.len() < 2 {
            return Err(DarshJError::InvalidAttribute(format!(
                "invalid time component: '{time_part}'"
            )));
        }

        // Keep the original time portion.
        let suffix = if time_part.ends_with('Z') && !time_part.contains('+') {
            ""
        } else {
            ""
        };
        return Ok(format!("{date}T{time_part}{suffix}"));
    }

    // Fall back to date-only with midnight.
    let date = parse_date(trimmed)?;
    Ok(format!("{date}T00:00:00Z"))
}

fn try_split_date(s: &str, sep: char) -> Option<(&str, &str)> {
    let idx = s.find(sep)?;
    Some((&s[..idx], &s[idx + 1..]))
}

fn validate_ymd(y: &str, m: &str, d: &str) -> Result<String> {
    let year: u32 = y
        .parse()
        .map_err(|_| DarshJError::InvalidAttribute(format!("invalid year: '{y}'")))?;
    let month: u32 = m
        .parse()
        .map_err(|_| DarshJError::InvalidAttribute(format!("invalid month: '{m}'")))?;
    let day: u32 = d
        .parse()
        .map_err(|_| DarshJError::InvalidAttribute(format!("invalid day: '{d}'")))?;

    if !(1..=12).contains(&month) {
        return Err(DarshJError::InvalidAttribute(format!(
            "month out of range: {month}"
        )));
    }
    if !(1..=31).contains(&day) {
        return Err(DarshJError::InvalidAttribute(format!(
            "day out of range: {day}"
        )));
    }
    if !(1..=9999).contains(&year) {
        return Err(DarshJError::InvalidAttribute(format!(
            "year out of range: {year}"
        )));
    }

    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::*;
    use serde_json::json;

    fn text_field(required: bool) -> FieldConfig {
        FieldConfig {
            id: FieldId::new(),
            name: "name".into(),
            field_type: FieldType::SingleLineText,
            table_entity_type: "user".into(),
            description: None,
            required,
            unique: false,
            default_value: None,
            options: None,
            order: 0,
        }
    }

    fn field_of(ft: FieldType, opts: Option<FieldOptions>) -> FieldConfig {
        FieldConfig {
            id: FieldId::new(),
            name: "test".into(),
            field_type: ft,
            table_entity_type: "t".into(),
            description: None,
            required: false,
            unique: false,
            default_value: None,
            options: opts,
            order: 0,
        }
    }

    // ── Null handling ──────────────────────────────────────────────

    #[test]
    fn null_allowed_when_not_required() {
        let f = text_field(false);
        let result = validate_value(&f, &Value::Null);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[test]
    fn null_rejected_when_required() {
        let f = text_field(true);
        let result = validate_value(&f, &Value::Null);
        assert!(result.is_err());
    }

    // ── SingleLineText ─────────────────────────────────────────────

    #[test]
    fn single_line_text_accepts_string() {
        let f = text_field(false);
        let v = json!("hello");
        assert_eq!(validate_value(&f, &v).unwrap(), json!("hello"));
    }

    #[test]
    fn single_line_text_rejects_newline() {
        let f = text_field(false);
        let v = json!("line1\nline2");
        assert!(validate_value(&f, &v).is_err());
    }

    #[test]
    fn single_line_text_coerces_number() {
        let f = text_field(false);
        let v = json!(42);
        assert_eq!(validate_value(&f, &v).unwrap(), json!("42"));
    }

    // ── Number ─────────────────────────────────────────────────────

    #[test]
    fn number_accepts_integer() {
        let f = field_of(FieldType::Number, None);
        let v = json!(42);
        assert!(validate_value(&f, &v).is_ok());
    }

    #[test]
    fn number_precision_enforced() {
        let f = field_of(
            FieldType::Number,
            Some(FieldOptions::Number {
                precision: 2,
                format: "decimal".into(),
            }),
        );
        let v = json!(3.14159);
        let result = validate_value(&f, &v).unwrap();
        assert_eq!(result, json!(3.14));
    }

    #[test]
    fn number_from_string() {
        let f = field_of(FieldType::Number, None);
        let v = json!("123.45");
        assert!(validate_value(&f, &v).is_ok());
    }

    #[test]
    fn number_rejects_text() {
        let f = field_of(FieldType::Number, None);
        let v = json!("not a number");
        assert!(validate_value(&f, &v).is_err());
    }

    // ── Checkbox ───────────────────────────────────────────────────

    #[test]
    fn checkbox_accepts_bool() {
        let f = field_of(FieldType::Checkbox, None);
        assert_eq!(validate_value(&f, &json!(true)).unwrap(), json!(true));
    }

    #[test]
    fn checkbox_coerces_string() {
        let f = field_of(FieldType::Checkbox, None);
        assert_eq!(validate_value(&f, &json!("yes")).unwrap(), json!(true));
        assert_eq!(validate_value(&f, &json!("false")).unwrap(), json!(false));
    }

    #[test]
    fn checkbox_coerces_number() {
        let f = field_of(FieldType::Checkbox, None);
        assert_eq!(validate_value(&f, &json!(1)).unwrap(), json!(true));
        assert_eq!(validate_value(&f, &json!(0)).unwrap(), json!(false));
    }

    // ── Email ──────────────────────────────────────────────────────

    #[test]
    fn email_valid() {
        let f = field_of(FieldType::Email, None);
        let result = validate_value(&f, &json!("User@Example.COM")).unwrap();
        assert_eq!(result, json!("user@example.com"));
    }

    #[test]
    fn email_invalid_no_at() {
        let f = field_of(FieldType::Email, None);
        assert!(validate_value(&f, &json!("nope")).is_err());
    }

    #[test]
    fn email_invalid_no_dot_in_domain() {
        let f = field_of(FieldType::Email, None);
        assert!(validate_value(&f, &json!("user@localhost")).is_err());
    }

    // ── URL ────────────────────────────────────────────────────────

    #[test]
    fn url_valid() {
        let f = field_of(FieldType::Url, None);
        let v = json!("https://darshj.me");
        assert!(validate_value(&f, &v).is_ok());
    }

    #[test]
    fn url_rejects_no_scheme() {
        let f = field_of(FieldType::Url, None);
        assert!(validate_value(&f, &json!("darshj.me")).is_err());
    }

    #[test]
    fn url_rejects_empty_host() {
        let f = field_of(FieldType::Url, None);
        assert!(validate_value(&f, &json!("https://")).is_err());
    }

    // ── Phone ──────────────────────────────────────────────────────

    #[test]
    fn phone_valid() {
        let f = field_of(FieldType::Phone, None);
        let result = validate_value(&f, &json!("+1 (555) 123-4567")).unwrap();
        assert_eq!(result, json!("+15551234567"));
    }

    #[test]
    fn phone_too_short() {
        let f = field_of(FieldType::Phone, None);
        assert!(validate_value(&f, &json!("123")).is_err());
    }

    // ── Date ───────────────────────────────────────────────────────

    #[test]
    fn date_iso_format() {
        let f = field_of(FieldType::Date, None);
        let v = json!("2025-01-15");
        assert_eq!(validate_value(&f, &v).unwrap(), json!("2025-01-15"));
    }

    #[test]
    fn date_us_format() {
        let f = field_of(FieldType::Date, None);
        let v = json!("01/15/2025");
        assert_eq!(validate_value(&f, &v).unwrap(), json!("2025-01-15"));
    }

    #[test]
    fn date_eu_format() {
        let f = field_of(FieldType::Date, None);
        let v = json!("15.01.2025");
        assert_eq!(validate_value(&f, &v).unwrap(), json!("2025-01-15"));
    }

    #[test]
    fn date_invalid() {
        let f = field_of(FieldType::Date, None);
        assert!(validate_value(&f, &json!("not-a-date")).is_err());
    }

    // ── DateTime ───────────────────────────────────────────────────

    #[test]
    fn datetime_with_t_separator() {
        let f = field_of(FieldType::DateTime, None);
        let v = json!("2025-01-15T10:30:00Z");
        let result = validate_value(&f, &v).unwrap();
        assert_eq!(result, json!("2025-01-15T10:30:00Z"));
    }

    #[test]
    fn datetime_date_only_adds_midnight() {
        let f = field_of(FieldType::DateTime, None);
        let v = json!("2025-01-15");
        let result = validate_value(&f, &v).unwrap();
        assert_eq!(result, json!("2025-01-15T00:00:00Z"));
    }

    // ── Rating ─────────────────────────────────────────────────────

    #[test]
    fn rating_valid() {
        let f = field_of(
            FieldType::Rating,
            Some(FieldOptions::Rating {
                max: 5,
                icon: "star".into(),
            }),
        );
        assert_eq!(validate_value(&f, &json!(3)).unwrap(), json!(3));
    }

    #[test]
    fn rating_out_of_range() {
        let f = field_of(
            FieldType::Rating,
            Some(FieldOptions::Rating {
                max: 5,
                icon: "star".into(),
            }),
        );
        assert!(validate_value(&f, &json!(6)).is_err());
    }

    // ── SingleSelect ───────────────────────────────────────────────

    #[test]
    fn single_select_valid_choice() {
        let f = field_of(
            FieldType::SingleSelect,
            Some(FieldOptions::Select {
                choices: vec![
                    SelectChoice {
                        id: "1".into(),
                        name: "Active".into(),
                        color: "#green".into(),
                    },
                    SelectChoice {
                        id: "2".into(),
                        name: "Inactive".into(),
                        color: "#red".into(),
                    },
                ],
            }),
        );
        assert!(validate_value(&f, &json!("Active")).is_ok());
    }

    #[test]
    fn single_select_invalid_choice() {
        let f = field_of(
            FieldType::SingleSelect,
            Some(FieldOptions::Select {
                choices: vec![SelectChoice {
                    id: "1".into(),
                    name: "Active".into(),
                    color: "#green".into(),
                }],
            }),
        );
        assert!(validate_value(&f, &json!("Unknown")).is_err());
    }

    // ── MultiSelect ────────────────────────────────────────────────

    #[test]
    fn multi_select_valid() {
        let f = field_of(
            FieldType::MultiSelect,
            Some(FieldOptions::Select {
                choices: vec![
                    SelectChoice {
                        id: "1".into(),
                        name: "A".into(),
                        color: "#a".into(),
                    },
                    SelectChoice {
                        id: "2".into(),
                        name: "B".into(),
                        color: "#b".into(),
                    },
                ],
            }),
        );
        assert!(validate_value(&f, &json!(["A", "B"])).is_ok());
    }

    #[test]
    fn multi_select_invalid_item() {
        let f = field_of(
            FieldType::MultiSelect,
            Some(FieldOptions::Select {
                choices: vec![SelectChoice {
                    id: "1".into(),
                    name: "A".into(),
                    color: "#a".into(),
                }],
            }),
        );
        assert!(validate_value(&f, &json!(["A", "Z"])).is_err());
    }

    // ── Computed fields ────────────────────────────────────────────

    #[test]
    fn computed_field_rejects_value() {
        let f = field_of(FieldType::AutoNumber, None);
        assert!(validate_value(&f, &json!(1)).is_err());
    }

    // ── Currency ───────────────────────────────────────────────────

    #[test]
    fn currency_precision() {
        let f = field_of(
            FieldType::Currency,
            Some(FieldOptions::Currency {
                symbol: "$".into(),
                precision: 2,
            }),
        );
        let result = validate_value(&f, &json!(19.999)).unwrap();
        assert_eq!(result, json!(20.0));
    }

    // ── Duration ───────────────────────────────────────────────────

    #[test]
    fn duration_accepts_number() {
        let f = field_of(FieldType::Duration, None);
        assert!(validate_value(&f, &json!(3600)).is_ok());
    }

    #[test]
    fn duration_rejects_negative() {
        let f = field_of(FieldType::Duration, None);
        assert!(validate_value(&f, &json!(-1)).is_err());
    }

    // ── Percent ────────────────────────────────────────────────────

    #[test]
    fn percent_accepts_number() {
        let f = field_of(FieldType::Percent, None);
        assert_eq!(validate_value(&f, &json!(0.75)).unwrap(), json!(0.75));
    }

    // ── Attachment ─────────────────────────────────────────────────

    #[test]
    fn attachment_accepts_object() {
        let f = field_of(FieldType::Attachment, None);
        let v = json!({"url": "https://example.com/file.pdf", "name": "file.pdf"});
        assert!(validate_value(&f, &v).is_ok());
    }

    #[test]
    fn attachment_rejects_string() {
        let f = field_of(FieldType::Attachment, None);
        assert!(validate_value(&f, &json!("nope")).is_err());
    }

    // ── Link ───────────────────────────────────────────────────────

    #[test]
    fn link_accepts_uuid_string() {
        let f = field_of(FieldType::Link, None);
        let v = json!("550e8400-e29b-41d4-a716-446655440000");
        assert!(validate_value(&f, &v).is_ok());
    }

    #[test]
    fn link_accepts_array() {
        let f = field_of(FieldType::Link, None);
        let v = json!(["550e8400-e29b-41d4-a716-446655440000"]);
        assert!(validate_value(&f, &v).is_ok());
    }

    // ── Helper tests ───────────────────────────────────────────────

    #[test]
    fn parse_date_slash_format() {
        assert_eq!(parse_date("2025/03/15").unwrap(), "2025-03-15");
    }

    #[test]
    fn validate_ymd_rejects_month_13() {
        assert!(validate_ymd("2025", "13", "01").is_err());
    }

    #[test]
    fn validate_ymd_rejects_day_32() {
        assert!(validate_ymd("2025", "01", "32").is_err());
    }
}
