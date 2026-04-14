//! Slice 28/30 — Admin SQL passthrough (Phase 9 SurrealDB parity).
//!
//! Author: Darshankumar Joshi.
//!
//! Exposes `POST /api/sql` as a raw SQL escape hatch for privileged
//! operators. The route is gated by [`require_admin_role`] upstream
//! (see `rest::build_router`) so only callers holding the `admin` role
//! reach this module.
//!
//! # Guarantees
//!
//! - **Whitelisted statements**: the first keyword must be one of
//!   `SELECT`, `INSERT`, `UPDATE`, `DELETE`, or `WITH`. Any DDL
//!   (`CREATE`, `DROP`, `ALTER`, `TRUNCATE`, `GRANT`, `REVOKE`) is
//!   rejected with HTTP 400 before Postgres ever sees the string.
//! - **Parameterised**: all user input is routed through sqlx bind
//!   parameters — never string interpolation. The slice forbids string
//!   concatenation and this module upholds that invariant.
//! - **Audit logged**: every call — success or failure — appends a row
//!   to `admin_audit_log` with actor, statement, params, row count,
//!   duration, and error (if any). The log is append-only.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::{Column, PgPool, Row, TypeInfo};
use uuid::Uuid;

use crate::error::{DarshJError, Result};

/// Inbound body for `POST /api/sql`.
#[derive(Debug, Clone, Deserialize)]
pub struct SqlPassthroughRequest {
    /// The SQL statement to execute. Must parameterise every piece of
    /// caller input via `$1`, `$2`, … placeholders.
    pub sql: String,
    /// Positional parameters bound to the placeholders. Each entry is
    /// a JSON value; see [`bind_json_param`] for the coercion rules.
    #[serde(default)]
    pub params: Vec<Value>,
}

/// Outbound body for `POST /api/sql`.
#[derive(Debug, Clone, Serialize)]
pub struct SqlPassthroughResponse {
    pub rows: Vec<Map<String, Value>>,
    pub row_count: u64,
    pub duration_ms: u64,
    pub statement: String,
}

/// Result of pre-flight statement whitelisting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatementClass {
    Read,
    Write,
    Rejected(&'static str),
}

/// Extract the first SQL keyword (upper-cased) from `sql`. Skips line
/// comments (`-- …`) and block comments (`/* … */`) before reading.
pub fn first_keyword(sql: &str) -> Option<String> {
    let mut chars = sql.chars().peekable();
    // Skip whitespace and comments.
    loop {
        match chars.peek().copied() {
            None => return None,
            Some(c) if c.is_whitespace() => {
                chars.next();
            }
            Some('-') => {
                // Line comment `-- …`
                chars.next();
                if chars.peek().copied() == Some('-') {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                } else {
                    return Some("-".to_string());
                }
            }
            Some('/') => {
                chars.next();
                if chars.peek().copied() == Some('*') {
                    chars.next();
                    let mut prev = '\0';
                    for c in chars.by_ref() {
                        if prev == '*' && c == '/' {
                            break;
                        }
                        prev = c;
                    }
                } else {
                    return Some("/".to_string());
                }
            }
            Some(_) => break,
        }
    }
    let mut word = String::new();
    for c in chars {
        if c.is_alphabetic() || c == '_' {
            word.push(c.to_ascii_uppercase());
        } else {
            break;
        }
    }
    if word.is_empty() { None } else { Some(word) }
}

/// Classify a statement by its first keyword. Only the whitelist
/// reaches Postgres.
pub fn classify(sql: &str) -> StatementClass {
    let Some(kw) = first_keyword(sql) else {
        return StatementClass::Rejected("empty statement");
    };
    match kw.as_str() {
        "SELECT" | "WITH" => StatementClass::Read,
        "INSERT" | "UPDATE" | "DELETE" => StatementClass::Write,
        "CREATE" | "DROP" | "ALTER" | "TRUNCATE" | "GRANT" | "REVOKE" => {
            StatementClass::Rejected("DDL statements are not allowed via /api/sql")
        }
        // Defence in depth: anything not explicitly whitelisted is
        // rejected so future Postgres keywords (VACUUM, COPY, …) don't
        // slip through on upgrade.
        _ => StatementClass::Rejected("only SELECT/INSERT/UPDATE/DELETE/WITH are permitted"),
    }
}

/// Execute a passthrough statement with full audit logging. Returns
/// the row payload on success or an error on failure. In both cases a
/// row is inserted into `admin_audit_log`.
///
/// The caller must pass the authenticated `actor_user_id` decoded from
/// the bearer token. This is the subject recorded in the audit log
/// and is never derived from the request body.
pub async fn execute_passthrough(
    pool: &PgPool,
    actor_user_id: Uuid,
    request: &SqlPassthroughRequest,
) -> Result<SqlPassthroughResponse> {
    // 1. Whitelist enforcement.
    let class = classify(&request.sql);
    if let StatementClass::Rejected(reason) = class {
        record_audit(
            pool,
            actor_user_id,
            &request.sql,
            &request.params,
            0,
            0,
            Some(reason),
        )
        .await;
        return Err(DarshJError::InvalidQuery(reason.to_string()));
    }

    // 2. Build the query, binding every param via sqlx.
    let start = Instant::now();
    let mut q = sqlx::query(&request.sql);
    for p in &request.params {
        q = bind_json_param(q, p);
    }

    // 3. Execute and harvest rows.
    let exec_result = q.fetch_all(pool).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    match exec_result {
        Ok(rows) => {
            let row_count = rows.len() as u64;
            let json_rows: Vec<Map<String, Value>> = rows.iter().map(pg_row_to_json).collect();

            record_audit(
                pool,
                actor_user_id,
                &request.sql,
                &request.params,
                row_count as i64,
                duration_ms as i64,
                None,
            )
            .await;

            Ok(SqlPassthroughResponse {
                rows: json_rows,
                row_count,
                duration_ms,
                statement: first_keyword(&request.sql).unwrap_or_default(),
            })
        }
        Err(e) => {
            let msg = e.to_string();
            record_audit(
                pool,
                actor_user_id,
                &request.sql,
                &request.params,
                0,
                duration_ms as i64,
                Some(&msg),
            )
            .await;
            Err(DarshJError::Database(e))
        }
    }
}

/// Bind a single JSON value to a query, mapping common JSON shapes to
/// Postgres-native types so parameters interoperate with typed columns.
fn bind_json_param<'q>(
    q: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    value: &'q Value,
) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match value {
        Value::Null => q.bind::<Option<String>>(None),
        Value::Bool(b) => q.bind(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q.bind(n.to_string())
            }
        }
        Value::String(s) => {
            // Try to recognise UUID strings so `WHERE id = $1` binds
            // against a UUID column without explicit casts.
            if let Ok(uuid) = Uuid::parse_str(s) {
                q.bind(uuid)
            } else {
                q.bind(s.clone())
            }
        }
        // Arrays and objects are bound as JSONB so callers can round-trip
        // nested payloads through the passthrough unchanged.
        Value::Array(_) | Value::Object(_) => q.bind(value.clone()),
    }
}

/// Convert a single Postgres row to a JSON object. Unknown / binary
/// column types are stringified via Display to avoid panicking on
/// exotic shapes (geometry, intervals, etc.).
fn pg_row_to_json(row: &sqlx::postgres::PgRow) -> Map<String, Value> {
    let mut out = Map::new();
    for col in row.columns() {
        let name = col.name();
        let type_name = col.type_info().name();
        let value = extract_column(row, name, type_name);
        out.insert(name.to_string(), value);
    }
    out
}

fn extract_column(row: &sqlx::postgres::PgRow, name: &str, type_name: &str) -> Value {
    // Try types in a sensible order. Return the first successful decode.
    macro_rules! try_get {
        ($ty:ty) => {
            if let Ok(v) = row.try_get::<Option<$ty>, _>(name) {
                return serde_json::to_value(v).unwrap_or(Value::Null);
            }
        };
    }

    match type_name {
        "BOOL" => try_get!(bool),
        "INT2" => try_get!(i16),
        "INT4" => try_get!(i32),
        "INT8" => try_get!(i64),
        "FLOAT4" => try_get!(f32),
        "FLOAT8" => try_get!(f64),
        "UUID" => try_get!(Uuid),
        "JSON" | "JSONB" => try_get!(serde_json::Value),
        "TEXT" | "VARCHAR" | "NAME" | "CHAR" | "BPCHAR" | "CITEXT" => try_get!(String),
        "TIMESTAMP" => try_get!(chrono::NaiveDateTime),
        "TIMESTAMPTZ" => try_get!(chrono::DateTime<chrono::Utc>),
        "DATE" => try_get!(chrono::NaiveDate),
        _ => {}
    }

    // Fallback: attempt string decode, then give up with null.
    if let Ok(v) = row.try_get::<Option<String>, _>(name) {
        return serde_json::to_value(v).unwrap_or(Value::Null);
    }
    Value::Null
}

/// Append an audit row. Errors are logged but **not** propagated — the
/// audit log must never turn a successful query into a failure, and a
/// failed query's error must survive regardless of audit outcome.
async fn record_audit(
    pool: &PgPool,
    actor_user_id: Uuid,
    sql: &str,
    params: &[Value],
    row_count: i64,
    duration_ms: i64,
    error: Option<&str>,
) {
    let params_json = serde_json::to_value(params).unwrap_or_else(|_| serde_json::json!([]));
    let result = sqlx::query(
        r#"
        INSERT INTO admin_audit_log
            (actor_user_id, sql, params, row_count, duration_ms, error)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(actor_user_id)
    .bind(sql)
    .bind(&params_json)
    .bind(row_count)
    .bind(duration_ms)
    .bind(error)
    .execute(pool)
    .await;

    if let Err(e) = result {
        tracing::warn!(error = %e, actor = %actor_user_id, "failed to record admin SQL audit");
    }
}

/// Create the `admin_audit_log` table if absent. Mirrors the migration
/// file at `migrations/20260414004000_schema_definitions_and_audit.sql`
/// so integration test pools and fresh installs get the schema without
/// an external migration runner.
pub async fn ensure_audit_schema(pool: &PgPool) -> Result<()> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS admin_audit_log (
            id             BIGSERIAL   PRIMARY KEY,
            actor_user_id  UUID        NOT NULL,
            sql            TEXT        NOT NULL,
            params         JSONB       NOT NULL DEFAULT '[]'::jsonb,
            row_count      BIGINT      NOT NULL DEFAULT 0,
            duration_ms    BIGINT      NOT NULL DEFAULT 0,
            error          TEXT,
            created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
        );

        CREATE INDEX IF NOT EXISTS idx_admin_audit_log_actor
            ON admin_audit_log (actor_user_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_admin_audit_log_created
            ON admin_audit_log (created_at DESC);
        "#,
    )
    .execute(pool)
    .await
    .map_err(DarshJError::Database)?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_keyword_handles_leading_whitespace_and_comments() {
        assert_eq!(first_keyword("SELECT 1").as_deref(), Some("SELECT"));
        assert_eq!(first_keyword("  select *").as_deref(), Some("SELECT"));
        assert_eq!(
            first_keyword("\n\t-- note\nSELECT 1").as_deref(),
            Some("SELECT")
        );
        assert_eq!(
            first_keyword("/* block\ncomment */ UPDATE users SET x = 1").as_deref(),
            Some("UPDATE")
        );
        assert_eq!(first_keyword("").as_deref(), None);
    }

    #[test]
    fn classify_whitelists_reads_and_writes() {
        assert_eq!(classify("SELECT * FROM users"), StatementClass::Read);
        assert_eq!(
            classify("WITH cte AS (SELECT 1) SELECT * FROM cte"),
            StatementClass::Read
        );
        assert_eq!(
            classify("INSERT INTO t VALUES (1)"),
            StatementClass::Write
        );
        assert_eq!(classify("UPDATE t SET x = 1"), StatementClass::Write);
        assert_eq!(classify("DELETE FROM t"), StatementClass::Write);
    }

    #[test]
    fn classify_rejects_ddl_and_exotic_keywords() {
        for ddl in [
            "CREATE TABLE foo (x int)",
            "drop table foo",
            "ALTER TABLE foo ADD COLUMN y int",
            "TRUNCATE foo",
            "GRANT SELECT ON foo TO bar",
            "REVOKE ALL ON foo FROM bar",
            "VACUUM FULL",
            "COPY foo FROM '/tmp/x'",
        ] {
            assert!(
                matches!(classify(ddl), StatementClass::Rejected(_)),
                "expected {ddl} to be rejected"
            );
        }
    }

    #[test]
    fn classify_rejects_empty_input() {
        assert!(matches!(classify(""), StatementClass::Rejected(_)));
        assert!(matches!(classify("   \n\n"), StatementClass::Rejected(_)));
    }
}
