//! Expression evaluator: walks the AST and produces a [`serde_json::Value`].
//!
//! Supports 30+ built-in functions covering text, math, logic, date, and
//! error-handling categories.  Type coercion follows spreadsheet conventions:
//! strings that look like numbers are auto-promoted in arithmetic contexts,
//! and numbers are stringified in concatenation contexts.

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate, Utc};
use serde_json::Value;

use crate::error::{DarshJError, Result};
use crate::formulas::parser::{Expr, Op};

// ── Error sentinel values ──────────────────────────────────────────

const ERROR_VALUE: &str = "#ERROR";
#[allow(dead_code)]
const ERROR_REF: &str = "#REF";
const ERROR_TYPE: &str = "#VALUE";
const ERROR_DIV0: &str = "#DIV/0";

/// Contextual data available during formula evaluation for a single record.
#[derive(Debug, Clone)]
pub struct RecordContext {
    /// Field name → current value mapping.
    pub field_values: HashMap<String, Value>,
    /// The entity / record id (UUID as string).
    pub record_id: Option<String>,
}

impl RecordContext {
    /// Create an empty context.
    pub fn new() -> Self {
        Self {
            field_values: HashMap::new(),
            record_id: None,
        }
    }

    /// Create a context with pre-populated field values.
    pub fn with_fields(fields: HashMap<String, Value>) -> Self {
        Self {
            field_values: fields,
            record_id: None,
        }
    }
}

impl Default for RecordContext {
    fn default() -> Self {
        Self::new()
    }
}

// ── Coercion helpers ───────────────────────────────────────────────

fn to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn to_bool(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            // Render integers without decimal point
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

fn is_error(v: &Value) -> bool {
    matches!(v, Value::String(s) if s.starts_with('#'))
}

fn error_val(code: &str) -> Value {
    Value::String(code.into())
}

fn num_val(n: f64) -> Value {
    serde_json::json!(n)
}

fn str_val(s: impl Into<String>) -> Value {
    Value::String(s.into())
}

// ── Main evaluator ─────────────────────────────────────────────────

/// Evaluate an expression AST against a record context.
pub fn evaluate(expr: &Expr, context: &RecordContext) -> Result<Value> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),

        Expr::FieldRef(name) => Ok(context
            .field_values
            .get(name)
            .cloned()
            .unwrap_or(Value::Null)),

        Expr::UnaryOp(op, operand) => {
            let val = evaluate(operand, context)?;
            match op {
                Op::Not => Ok(Value::Bool(!to_bool(&val))),
                Op::Sub => match to_f64(&val) {
                    Some(n) => Ok(num_val(-n)),
                    None => Ok(error_val(ERROR_TYPE)),
                },
                _ => Err(DarshJError::InvalidQuery(format!(
                    "unsupported unary operator: {op}"
                ))),
            }
        }

        Expr::BinaryOp(op, lhs, rhs) => {
            let left = evaluate(lhs, context)?;
            let right = evaluate(rhs, context)?;

            // Propagate errors
            if is_error(&left) {
                return Ok(left);
            }
            if is_error(&right) {
                return Ok(right);
            }

            match op {
                // Arithmetic
                Op::Add => match (to_f64(&left), to_f64(&right)) {
                    (Some(a), Some(b)) => Ok(num_val(a + b)),
                    _ => Ok(error_val(ERROR_TYPE)),
                },
                Op::Sub => match (to_f64(&left), to_f64(&right)) {
                    (Some(a), Some(b)) => Ok(num_val(a - b)),
                    _ => Ok(error_val(ERROR_TYPE)),
                },
                Op::Mul => match (to_f64(&left), to_f64(&right)) {
                    (Some(a), Some(b)) => Ok(num_val(a * b)),
                    _ => Ok(error_val(ERROR_TYPE)),
                },
                Op::Div => match (to_f64(&left), to_f64(&right)) {
                    (Some(_), Some(b)) if b == 0.0 => Ok(error_val(ERROR_DIV0)),
                    (Some(a), Some(b)) => Ok(num_val(a / b)),
                    _ => Ok(error_val(ERROR_TYPE)),
                },
                Op::Mod => match (to_f64(&left), to_f64(&right)) {
                    (Some(_), Some(b)) if b == 0.0 => Ok(error_val(ERROR_DIV0)),
                    (Some(a), Some(b)) => Ok(num_val(a % b)),
                    _ => Ok(error_val(ERROR_TYPE)),
                },

                // Comparison
                Op::Eq => Ok(Value::Bool(left == right)),
                Op::Neq => Ok(Value::Bool(left != right)),
                Op::Gt => Ok(Value::Bool(
                    compare_values(&left, &right) == Some(std::cmp::Ordering::Greater),
                )),
                Op::Gte => Ok(Value::Bool(matches!(
                    compare_values(&left, &right),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                ))),
                Op::Lt => Ok(Value::Bool(
                    compare_values(&left, &right) == Some(std::cmp::Ordering::Less),
                )),
                Op::Lte => Ok(Value::Bool(matches!(
                    compare_values(&left, &right),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                ))),

                // Logical
                Op::And => Ok(Value::Bool(to_bool(&left) && to_bool(&right))),
                Op::Or => Ok(Value::Bool(to_bool(&left) || to_bool(&right))),

                // Concatenation
                Op::Concat => {
                    let mut s = to_string(&left);
                    s.push_str(&to_string(&right));
                    Ok(str_val(s))
                }

                _ => Err(DarshJError::InvalidQuery(format!(
                    "unsupported binary operator: {op}"
                ))),
            }
        }

        Expr::If(cond, then_br, else_br) => {
            let cond_val = evaluate(cond, context)?;
            if to_bool(&cond_val) {
                evaluate(then_br, context)
            } else {
                evaluate(else_br, context)
            }
        }

        Expr::FunctionCall(name, args) => eval_function(name, args, context),
    }
}

fn compare_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    // Try numeric comparison first
    if let (Some(na), Some(nb)) = (to_f64(a), to_f64(b)) {
        return na.partial_cmp(&nb);
    }
    // Fall back to string comparison
    match (a, b) {
        (Value::String(sa), Value::String(sb)) => Some(sa.cmp(sb)),
        _ => None,
    }
}

// ── Function dispatch ──────────────────────────────────────────────

fn eval_function(name: &str, args: &[Expr], ctx: &RecordContext) -> Result<Value> {
    match name {
        // ── Logic ──────────────────────────────────────────────
        "AND" => {
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if !to_bool(&v) {
                    return Ok(Value::Bool(false));
                }
            }
            Ok(Value::Bool(true))
        }
        "OR" => {
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if to_bool(&v) {
                    return Ok(Value::Bool(true));
                }
            }
            Ok(Value::Bool(false))
        }
        "NOT" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            Ok(Value::Bool(!to_bool(&v)))
        }
        "SWITCH" => {
            // SWITCH(expr, pattern1, result1, pattern2, result2, ..., default)
            if args.len() < 3 {
                return Err(DarshJError::InvalidQuery(
                    "SWITCH requires at least 3 arguments".into(),
                ));
            }
            let test = evaluate(&args[0], ctx)?;
            let pairs = &args[1..];
            let mut i = 0;
            while i + 1 < pairs.len() {
                let pattern = evaluate(&pairs[i], ctx)?;
                if test == pattern {
                    return evaluate(&pairs[i + 1], ctx);
                }
                i += 2;
            }
            // If odd remaining, it's the default
            if i < pairs.len() {
                evaluate(&pairs[i], ctx)
            } else {
                Ok(Value::Null)
            }
        }

        // ── Text ───────────────────────────────────────────────
        "CONCAT" => {
            let mut result = String::new();
            for arg in args {
                let v = evaluate(arg, ctx)?;
                result.push_str(&to_string(&v));
            }
            Ok(str_val(result))
        }
        "LEN" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            Ok(num_val(to_string(&v).len() as f64))
        }
        "LOWER" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            Ok(str_val(to_string(&v).to_lowercase()))
        }
        "UPPER" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            Ok(str_val(to_string(&v).to_uppercase()))
        }
        "TRIM" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            Ok(str_val(to_string(&v).trim().to_string()))
        }
        "LEFT" => {
            expect_arity(name, args, 2)?;
            let s = to_string(&evaluate(&args[0], ctx)?);
            let n = to_f64(&evaluate(&args[1], ctx)?).unwrap_or(0.0) as usize;
            let result: String = s.chars().take(n).collect();
            Ok(str_val(result))
        }
        "RIGHT" => {
            expect_arity(name, args, 2)?;
            let s = to_string(&evaluate(&args[0], ctx)?);
            let n = to_f64(&evaluate(&args[1], ctx)?).unwrap_or(0.0) as usize;
            let chars: Vec<char> = s.chars().collect();
            let start = chars.len().saturating_sub(n);
            let result: String = chars[start..].iter().collect();
            Ok(str_val(result))
        }
        "MID" => {
            expect_arity(name, args, 3)?;
            let s = to_string(&evaluate(&args[0], ctx)?);
            let start = to_f64(&evaluate(&args[1], ctx)?).unwrap_or(1.0) as usize;
            let count = to_f64(&evaluate(&args[2], ctx)?).unwrap_or(0.0) as usize;
            // 1-based index like Excel
            let chars: Vec<char> = s.chars().collect();
            let begin = start.saturating_sub(1).min(chars.len());
            let end = (begin + count).min(chars.len());
            let result: String = chars[begin..end].iter().collect();
            Ok(str_val(result))
        }
        "FIND" => {
            expect_arity(name, args, 2)?;
            let needle = to_string(&evaluate(&args[0], ctx)?);
            let haystack = to_string(&evaluate(&args[1], ctx)?);
            match haystack.find(&needle) {
                Some(pos) => Ok(num_val((pos + 1) as f64)), // 1-based
                None => Ok(num_val(0.0)),
            }
        }
        "SUBSTITUTE" => {
            if args.len() < 3 || args.len() > 4 {
                return Err(DarshJError::InvalidQuery(
                    "SUBSTITUTE requires 3 or 4 arguments".into(),
                ));
            }
            let text = to_string(&evaluate(&args[0], ctx)?);
            let old = to_string(&evaluate(&args[1], ctx)?);
            let new = to_string(&evaluate(&args[2], ctx)?);
            if args.len() == 4 {
                // Replace only the nth occurrence
                let n = to_f64(&evaluate(&args[3], ctx)?).unwrap_or(1.0) as usize;
                let mut count = 0usize;
                let mut result = String::new();
                let mut remaining = text.as_str();
                while let Some(pos) = remaining.find(&old) {
                    count += 1;
                    if count == n {
                        result.push_str(&remaining[..pos]);
                        result.push_str(&new);
                        result.push_str(&remaining[pos + old.len()..]);
                        return Ok(str_val(result));
                    }
                    result.push_str(&remaining[..pos + old.len()]);
                    remaining = &remaining[pos + old.len()..];
                }
                result.push_str(remaining);
                Ok(str_val(result))
            } else {
                Ok(str_val(text.replace(&old, &new)))
            }
        }

        // ── Math ───────────────────────────────────────────────
        "ROUND" => {
            expect_arity(name, args, 2)?;
            let n = require_number(name, &evaluate(&args[0], ctx)?)?;
            let decimals = to_f64(&evaluate(&args[1], ctx)?).unwrap_or(0.0) as i32;
            let factor = 10f64.powi(decimals);
            Ok(num_val((n * factor).round() / factor))
        }
        "FLOOR" => {
            expect_arity(name, args, 1)?;
            let n = require_number(name, &evaluate(&args[0], ctx)?)?;
            Ok(num_val(n.floor()))
        }
        "CEIL" => {
            expect_arity(name, args, 1)?;
            let n = require_number(name, &evaluate(&args[0], ctx)?)?;
            Ok(num_val(n.ceil()))
        }
        "ABS" => {
            expect_arity(name, args, 1)?;
            let n = require_number(name, &evaluate(&args[0], ctx)?)?;
            Ok(num_val(n.abs()))
        }
        "MIN" => {
            if args.is_empty() {
                return Ok(error_val(ERROR_VALUE));
            }
            let mut min = f64::INFINITY;
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if let Some(n) = to_f64(&v)
                    && n < min
                {
                    min = n;
                }
            }
            Ok(num_val(min))
        }
        "MAX" => {
            if args.is_empty() {
                return Ok(error_val(ERROR_VALUE));
            }
            let mut max = f64::NEG_INFINITY;
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if let Some(n) = to_f64(&v)
                    && n > max
                {
                    max = n;
                }
            }
            Ok(num_val(max))
        }
        "SUM" => {
            let mut total = 0.0;
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if let Some(n) = to_f64(&v) {
                    total += n;
                }
            }
            Ok(num_val(total))
        }
        "AVERAGE" => {
            if args.is_empty() {
                return Ok(error_val(ERROR_DIV0));
            }
            let mut total = 0.0;
            let mut count = 0u64;
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if let Some(n) = to_f64(&v) {
                    total += n;
                    count += 1;
                }
            }
            if count == 0 {
                Ok(error_val(ERROR_DIV0))
            } else {
                Ok(num_val(total / count as f64))
            }
        }
        "COUNT" => {
            // Count numeric values only
            let mut count = 0u64;
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if to_f64(&v).is_some() {
                    count += 1;
                }
            }
            Ok(num_val(count as f64))
        }
        "COUNTA" => {
            // Count non-empty values
            let mut count = 0u64;
            for arg in args {
                let v = evaluate(arg, ctx)?;
                if !v.is_null() {
                    count += 1;
                }
            }
            Ok(num_val(count as f64))
        }

        // ── Date ───────────────────────────────────────────────
        "NOW" => {
            expect_arity(name, args, 0)?;
            Ok(str_val(Utc::now().to_rfc3339()))
        }
        "TODAY" => {
            expect_arity(name, args, 0)?;
            Ok(str_val(Utc::now().format("%Y-%m-%d").to_string()))
        }
        "YEAR" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            let s = to_string(&v);
            match parse_date(&s) {
                Some(d) => Ok(num_val(d.year() as f64)),
                None => Ok(error_val(ERROR_VALUE)),
            }
        }
        "MONTH" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            let s = to_string(&v);
            match parse_date(&s) {
                Some(d) => Ok(num_val(d.month() as f64)),
                None => Ok(error_val(ERROR_VALUE)),
            }
        }
        "DAY" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            let s = to_string(&v);
            match parse_date(&s) {
                Some(d) => Ok(num_val(d.day() as f64)),
                None => Ok(error_val(ERROR_VALUE)),
            }
        }
        "DATEADD" => {
            expect_arity(name, args, 3)?;
            let v = evaluate(&args[0], ctx)?;
            let amount = to_f64(&evaluate(&args[1], ctx)?).unwrap_or(0.0) as i64;
            let unit = to_string(&evaluate(&args[2], ctx)?).to_lowercase();
            let s = to_string(&v);
            match parse_date(&s) {
                Some(d) => {
                    let result = match unit.as_str() {
                        "days" | "day" | "d" => d + chrono::Duration::days(amount),
                        "weeks" | "week" | "w" => d + chrono::Duration::weeks(amount),
                        "months" | "month" | "m" => {
                            // Approximate: 30 days per month
                            d + chrono::Duration::days(amount * 30)
                        }
                        "years" | "year" | "y" => d + chrono::Duration::days(amount * 365),
                        _ => return Ok(error_val(ERROR_VALUE)),
                    };
                    Ok(str_val(result.format("%Y-%m-%d").to_string()))
                }
                None => Ok(error_val(ERROR_VALUE)),
            }
        }
        "DATEDIFF" => {
            expect_arity(name, args, 3)?;
            let s1 = to_string(&evaluate(&args[0], ctx)?);
            let s2 = to_string(&evaluate(&args[1], ctx)?);
            let unit = to_string(&evaluate(&args[2], ctx)?).to_lowercase();
            match (parse_date(&s1), parse_date(&s2)) {
                (Some(d1), Some(d2)) => {
                    let days = (d2 - d1).num_days();
                    let result = match unit.as_str() {
                        "days" | "day" | "d" => days as f64,
                        "weeks" | "week" | "w" => days as f64 / 7.0,
                        "months" | "month" | "m" => days as f64 / 30.0,
                        "years" | "year" | "y" => days as f64 / 365.0,
                        _ => return Ok(error_val(ERROR_VALUE)),
                    };
                    Ok(num_val(result))
                }
                _ => Ok(error_val(ERROR_VALUE)),
            }
        }

        // ── Error handling ─────────────────────────────────────
        "BLANK" => {
            expect_arity(name, args, 0)?;
            Ok(Value::Null)
        }
        "ERROR" => {
            expect_arity(name, args, 1)?;
            let msg = to_string(&evaluate(&args[0], ctx)?);
            Ok(error_val(&format!("#ERROR: {msg}")))
        }
        "ISERROR" => {
            expect_arity(name, args, 1)?;
            let v = evaluate(&args[0], ctx)?;
            Ok(Value::Bool(is_error(&v)))
        }

        // ── Record ─────────────────────────────────────────────
        "RECORD_ID" => {
            expect_arity(name, args, 0)?;
            match &ctx.record_id {
                Some(id) => Ok(str_val(id.clone())),
                None => Ok(Value::Null),
            }
        }

        _ => Err(DarshJError::InvalidQuery(format!(
            "unknown function: {name}"
        ))),
    }
}

fn expect_arity(name: &str, args: &[Expr], expected: usize) -> Result<()> {
    if args.len() != expected {
        return Err(DarshJError::InvalidQuery(format!(
            "{name} expects {expected} argument(s), got {}",
            args.len()
        )));
    }
    Ok(())
}

fn require_number(fn_name: &str, v: &Value) -> Result<f64> {
    to_f64(v).ok_or_else(|| {
        DarshJError::InvalidQuery(format!("{fn_name}: expected number, got {}", to_string(v)))
    })
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    // Try ISO 8601 date or datetime
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    // Try datetime with T separator (take date part)
    if s.len() >= 10
        && let Ok(d) = NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d")
    {
        return Some(d);
    }
    None
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formulas::parser::parse;

    fn eval(formula: &str) -> Value {
        eval_with(formula, HashMap::new())
    }

    fn eval_with(formula: &str, fields: HashMap<String, Value>) -> Value {
        let expr = parse(formula).unwrap();
        let ctx = RecordContext::with_fields(fields);
        evaluate(&expr, &ctx).unwrap()
    }

    fn fields(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    // ── Arithmetic ─────────────────────────────────────────────
    #[test]
    fn test_basic_arithmetic() {
        assert_eq!(eval("2 + 3"), serde_json::json!(5.0));
        assert_eq!(eval("10 - 4"), serde_json::json!(6.0));
        assert_eq!(eval("3 * 7"), serde_json::json!(21.0));
        assert_eq!(eval("15 / 3"), serde_json::json!(5.0));
        assert_eq!(eval("17 % 5"), serde_json::json!(2.0));
    }

    #[test]
    fn test_division_by_zero() {
        assert_eq!(eval("10 / 0"), serde_json::json!("#DIV/0"));
    }

    #[test]
    fn test_operator_precedence_eval() {
        assert_eq!(eval("2 + 3 * 4"), serde_json::json!(14.0));
        assert_eq!(eval("(2 + 3) * 4"), serde_json::json!(20.0));
    }

    #[test]
    fn test_negation() {
        let f = fields(&[("X", serde_json::json!(5))]);
        assert_eq!(eval_with("-{X}", f), serde_json::json!(-5.0));
    }

    // ── Comparison ─────────────────────────────────────────────
    #[test]
    fn test_comparisons() {
        assert_eq!(eval("5 > 3"), Value::Bool(true));
        assert_eq!(eval("5 < 3"), Value::Bool(false));
        assert_eq!(eval("5 >= 5"), Value::Bool(true));
        assert_eq!(eval("5 <= 4"), Value::Bool(false));
        assert_eq!(eval("5 = 5"), Value::Bool(true));
        assert_eq!(eval("5 != 3"), Value::Bool(true));
    }

    // ── Logic ──────────────────────────────────────────────────
    #[test]
    fn test_logic_functions() {
        assert_eq!(eval("AND(TRUE, TRUE)"), Value::Bool(true));
        assert_eq!(eval("AND(TRUE, FALSE)"), Value::Bool(false));
        assert_eq!(eval("OR(FALSE, TRUE)"), Value::Bool(true));
        assert_eq!(eval("OR(FALSE, FALSE)"), Value::Bool(false));
        assert_eq!(eval("NOT(TRUE)"), Value::Bool(false));
    }

    #[test]
    fn test_if_expression() {
        assert_eq!(
            eval(r#"IF(TRUE, "yes", "no")"#),
            Value::String("yes".into())
        );
        assert_eq!(
            eval(r#"IF(FALSE, "yes", "no")"#),
            Value::String("no".into())
        );
    }

    #[test]
    fn test_switch() {
        let f = fields(&[("S", Value::String("B".into()))]);
        let result = eval_with(r#"SWITCH({S}, "A", 1, "B", 2, 0)"#, f);
        assert_eq!(result, serde_json::json!(2.0));
    }

    // ── Text ───────────────────────────────────────────────────
    #[test]
    fn test_text_functions() {
        assert_eq!(
            eval(r#"CONCAT("hello", " ", "world")"#),
            Value::String("hello world".into())
        );
        assert_eq!(eval(r#"LEN("hello")"#), serde_json::json!(5.0));
        assert_eq!(eval(r#"LOWER("HELLO")"#), Value::String("hello".into()));
        assert_eq!(eval(r#"UPPER("hello")"#), Value::String("HELLO".into()));
        assert_eq!(eval(r#"TRIM("  hi  ")"#), Value::String("hi".into()));
    }

    #[test]
    fn test_left_right_mid() {
        assert_eq!(eval(r#"LEFT("hello", 3)"#), Value::String("hel".into()));
        assert_eq!(eval(r#"RIGHT("hello", 2)"#), Value::String("lo".into()));
        assert_eq!(eval(r#"MID("hello", 2, 3)"#), Value::String("ell".into()));
    }

    #[test]
    fn test_find_substitute() {
        assert_eq!(eval(r#"FIND("ll", "hello")"#), serde_json::json!(3.0));
        assert_eq!(eval(r#"FIND("xyz", "hello")"#), serde_json::json!(0.0));
        assert_eq!(
            eval(r#"SUBSTITUTE("hello world", "world", "rust")"#),
            Value::String("hello rust".into())
        );
    }

    // ── Math ───────────────────────────────────────────────────
    #[test]
    fn test_math_functions() {
        assert_eq!(eval("ROUND(3.456, 2)"), serde_json::json!(3.46));
        assert_eq!(eval("FLOOR(3.7)"), serde_json::json!(3.0));
        assert_eq!(eval("CEIL(3.2)"), serde_json::json!(4.0));
        assert_eq!(eval("ABS(-5)"), serde_json::json!(5.0));
    }

    #[test]
    fn test_aggregate_functions() {
        assert_eq!(eval("MIN(3, 1, 4, 1, 5)"), serde_json::json!(1.0));
        assert_eq!(eval("MAX(3, 1, 4, 1, 5)"), serde_json::json!(5.0));
        assert_eq!(eval("SUM(1, 2, 3)"), serde_json::json!(6.0));
        assert_eq!(eval("AVERAGE(2, 4, 6)"), serde_json::json!(4.0));
        assert_eq!(eval("COUNT(1, 2, 3)"), serde_json::json!(3.0));
        assert_eq!(eval(r#"COUNTA(1, "a", TRUE)"#), serde_json::json!(3.0));
    }

    // ── Date ───────────────────────────────────────────────────
    #[test]
    fn test_date_functions() {
        let f = fields(&[("D", Value::String("2025-06-15".into()))]);
        assert_eq!(eval_with("YEAR({D})", f.clone()), serde_json::json!(2025.0));
        assert_eq!(eval_with("MONTH({D})", f.clone()), serde_json::json!(6.0));
        assert_eq!(eval_with("DAY({D})", f), serde_json::json!(15.0));
    }

    #[test]
    fn test_dateadd() {
        let f = fields(&[("D", Value::String("2025-01-01".into()))]);
        assert_eq!(
            eval_with(r#"DATEADD({D}, 10, "days")"#, f),
            Value::String("2025-01-11".into())
        );
    }

    #[test]
    fn test_datediff() {
        assert_eq!(
            eval(r#"DATEDIFF("2025-01-01", "2025-01-11", "days")"#),
            serde_json::json!(10.0)
        );
    }

    // ── Error handling ─────────────────────────────────────────
    #[test]
    fn test_iserror() {
        assert_eq!(eval("ISERROR(10 / 0)"), Value::Bool(true));
        assert_eq!(eval("ISERROR(10 / 2)"), Value::Bool(false));
    }

    #[test]
    fn test_blank() {
        assert_eq!(eval("BLANK()"), Value::Null);
    }

    // ── Field references ───────────────────────────────────────
    #[test]
    fn test_field_ref_missing_is_null() {
        let result = eval("{NonExistent}");
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn test_coercion_string_to_number() {
        let f = fields(&[("X", Value::String("42".into()))]);
        assert_eq!(eval_with("{X} + 8", f), serde_json::json!(50.0));
    }

    #[test]
    fn test_concat_operator() {
        let f = fields(&[
            ("First", Value::String("John".into())),
            ("Last", Value::String("Doe".into())),
        ]);
        assert_eq!(
            eval_with(r#"{First} & " " & {Last}"#, f),
            Value::String("John Doe".into())
        );
    }

    // ── Complex formulas ───────────────────────────────────────
    #[test]
    fn test_complex_formula() {
        let f = fields(&[
            ("Status", Value::String("Done".into())),
            ("Priority", serde_json::json!(5)),
        ]);
        let result = eval_with(
            r#"IF(AND({Status} = "Done", {Priority} > 3), "High", "Low")"#,
            f,
        );
        assert_eq!(result, Value::String("High".into()));
    }

    #[test]
    fn test_record_id() {
        let expr = parse("RECORD_ID()").unwrap();
        let mut ctx = RecordContext::new();
        ctx.record_id = Some("abc-123".into());
        let result = evaluate(&expr, &ctx).unwrap();
        assert_eq!(result, Value::String("abc-123".into()));
    }
}
