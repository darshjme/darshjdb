//! Time-series REST API — Phase 5.1.
//!
//! Author: Darshankumar Joshi
//!
//! Thin REST facade over the `time_series` hypertable (see
//! `migrations/20260414090000_timescale.sql`). Gives clients a familiar
//! entity-scoped shape — `/api/ts/:entity_type` — while delegating all
//! storage to TimescaleDB under the hood.
//!
//! ## Routes
//!
//! | Verb | Path                                    | Purpose                                   |
//! | ---- | --------------------------------------- | ----------------------------------------- |
//! | POST | `/api/ts/:entity_type`                  | Insert a single point                     |
//! | GET  | `/api/ts/:entity_type`                  | Range scan (optional `time_bucket` group) |
//! | GET  | `/api/ts/:entity_type/agg`              | Aggregation bucket (avg/sum/min/max/cnt)  |
//! | GET  | `/api/ts/:entity_type/latest`           | Latest value per entity (DISTINCT ON)     |
//!
//! ## SQL safety
//!
//! Every user-supplied value is passed as a bind parameter. Where an
//! identifier is required (bucket interval, aggregate function) the
//! input is matched against a closed whitelist before interpolation.
//!
//! ## TimescaleDB graceful degradation
//!
//! The handlers fall back to plain SQL when `time_bucket` is unavailable
//! so they keep working against vanilla Postgres in CI and local dev.
//! Aggregation buckets detect `undefined_function` (SQLSTATE `42883`)
//! and retry with `date_trunc`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::error::ApiError;
use super::rest::{negotiate_response_pub, AppState};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the `/ts/*` sub-router. Mounted from `build_router` so it
/// inherits rate limiting and auth middleware like every other
/// protected endpoint.
pub fn ts_routes() -> Router<AppState> {
    Router::new()
        .route("/ts/{entity_type}", post(ts_insert).get(ts_range))
        .route("/ts/{entity_type}/agg", get(ts_aggregate))
        .route("/ts/{entity_type}/latest", get(ts_latest))
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Validate an entity_type (table-like identifier). Mirrors the rules
/// used by `validate_entity_name` in `rest.rs` but is duplicated here
/// so this module stays independent.
fn validate_entity_type(name: &str) -> Result<(), ApiError> {
    if name.is_empty() {
        return Err(ApiError::bad_request("entity_type is required"));
    }
    if name.len() > 128 {
        return Err(ApiError::bad_request(
            "entity_type too long (max 128 chars)",
        ));
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(ApiError::bad_request(
            "entity_type must start with a letter or underscore",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "entity_type may only contain alphanumerics, underscores, and hyphens",
        ));
    }
    Ok(())
}

/// Validate an attribute name. Tolerates dots and colons so callers can
/// mirror triple-store attributes like `sensor/temp` or `:db/type`.
fn validate_attribute(name: &str) -> Result<(), ApiError> {
    if name.is_empty() {
        return Err(ApiError::bad_request("attribute is required"));
    }
    if name.len() > 256 {
        return Err(ApiError::bad_request("attribute too long (max 256 chars)"));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/'))
    {
        return Err(ApiError::bad_request(
            "attribute contains forbidden characters",
        ));
    }
    Ok(())
}

/// Whitelist of bucket intervals accepted by the range + agg endpoints.
/// Using a whitelist keeps the literal out of untrusted territory while
/// letting us inline it into the SQL fragment required by `time_bucket`
/// (TimescaleDB does not accept bucket widths as bind parameters).
fn canonicalize_bucket(raw: &str) -> Result<&'static str, ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1m" | "1 minute" | "minute" => Ok("1 minute"),
        "5m" | "5 minutes" => Ok("5 minutes"),
        "15m" | "15 minutes" => Ok("15 minutes"),
        "30m" | "30 minutes" => Ok("30 minutes"),
        "1h" | "1 hour" | "hour" => Ok("1 hour"),
        "6h" | "6 hours" => Ok("6 hours"),
        "12h" | "12 hours" => Ok("12 hours"),
        "1d" | "1 day" | "day" => Ok("1 day"),
        "7d" | "1w" | "1 week" | "week" => Ok("7 days"),
        "30d" | "1mo" | "1 month" | "month" => Ok("30 days"),
        other => Err(ApiError::bad_request(format!(
            "unsupported bucket '{other}' — use one of: 1m,5m,15m,30m,1h,6h,12h,1d,7d,30d"
        ))),
    }
}

/// Whitelist of aggregate functions → (SQL fragment, numeric?).
fn canonicalize_agg(raw: &str) -> Result<(&'static str, bool), ApiError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "avg" => Ok(("AVG(value_num)", true)),
        "sum" => Ok(("SUM(value_num)", true)),
        "min" => Ok(("MIN(value_num)", true)),
        "max" => Ok(("MAX(value_num)", true)),
        "count" | "cnt" => Ok(("COUNT(*)", false)),
        other => Err(ApiError::bad_request(format!(
            "unsupported fn '{other}' — use one of: avg,sum,min,max,count"
        ))),
    }
}

/// `date_trunc` field name matching a canonical interval. Only used
/// as the fallback path when TimescaleDB's `time_bucket` is missing.
/// Returns `None` for intervals date_trunc cannot express (e.g. 5m).
fn bucket_to_date_trunc_field(bucket: &str) -> Option<&'static str> {
    match bucket {
        "1 minute" => Some("minute"),
        "1 hour" => Some("hour"),
        "1 day" => Some("day"),
        "7 days" => Some("week"),
        "30 days" => Some("month"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Insert
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InsertBody {
    entity_id: Uuid,
    attribute: String,
    #[serde(default)]
    time: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    value_num: Option<f64>,
    #[serde(default)]
    value_text: Option<String>,
    #[serde(default)]
    value_json: Option<Value>,
    #[serde(default)]
    tags: Option<Value>,
}

/// `POST /api/ts/:entity_type` — insert a single point.
///
/// At least one of `value_num`, `value_text`, `value_json` must be set.
/// `time` defaults to `now()` when omitted. `tags` defaults to `{}`.
async fn ts_insert(
    State(state): State<AppState>,
    Path(entity_type): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<InsertBody>,
) -> Result<Response, ApiError> {
    validate_entity_type(&entity_type)?;
    validate_attribute(&body.attribute)?;

    if body.value_num.is_none() && body.value_text.is_none() && body.value_json.is_none() {
        return Err(ApiError::bad_request(
            "at least one of value_num, value_text, value_json is required",
        ));
    }

    let ts = body.time.unwrap_or_else(chrono::Utc::now);
    let tags = body.tags.unwrap_or_else(|| json!({}));
    if !tags.is_object() {
        return Err(ApiError::bad_request("tags must be a JSON object"));
    }

    sqlx::query(
        r#"
        INSERT INTO time_series
            (time, entity_id, entity_type, attribute, value_num, value_text, value_json, tags)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (entity_type, entity_id, attribute, time) DO UPDATE SET
            value_num  = EXCLUDED.value_num,
            value_text = EXCLUDED.value_text,
            value_json = EXCLUDED.value_json,
            tags       = EXCLUDED.tags
        "#,
    )
    .bind(ts)
    .bind(body.entity_id)
    .bind(&entity_type)
    .bind(&body.attribute)
    .bind(body.value_num)
    .bind(body.value_text.as_deref())
    .bind(&body.value_json)
    .bind(&tags)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("ts insert failed: {e}")))?;

    let body = json!({
        "ok": true,
        "entity_type": entity_type,
        "entity_id": body.entity_id,
        "attribute": body.attribute,
        "time": ts,
    });
    let mut resp = negotiate_response_pub(&headers, &body);
    *resp.status_mut() = StatusCode::CREATED;
    Ok(resp)
}

// ---------------------------------------------------------------------------
// Range scan
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct RangeParams {
    #[serde(default)]
    from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    entity_id: Option<Uuid>,
    #[serde(default)]
    attribute: Option<String>,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// `GET /api/ts/:entity_type` — range scan with optional bucket grouping.
///
/// Without `bucket`, returns raw rows ordered by time. With `bucket`, groups
/// by `time_bucket` (or `date_trunc` as a fallback) and emits
/// `{bucket, count, avg, min, max, sum}` per interval.
async fn ts_range(
    State(state): State<AppState>,
    Path(entity_type): Path<String>,
    Query(params): Query<RangeParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    validate_entity_type(&entity_type)?;
    if let Some(attr) = &params.attribute {
        validate_attribute(attr)?;
    }
    let limit = params.limit.unwrap_or(1000).clamp(1, 10_000);

    // Bucketed path — aggregation over `value_num`.
    if let Some(bucket_raw) = &params.bucket {
        let bucket = canonicalize_bucket(bucket_raw)?;
        return run_bucketed(&state.pool, &entity_type, &params, bucket, limit, &headers).await;
    }

    // Raw path — select matching rows directly.
    let rows = sqlx::query(
        r#"
        SELECT time, entity_id, entity_type, attribute, value_num, value_text, value_json, tags
        FROM time_series
        WHERE entity_type = $1
          AND ($2::timestamptz IS NULL OR time >= $2)
          AND ($3::timestamptz IS NULL OR time <  $3)
          AND ($4::uuid         IS NULL OR entity_id = $4)
          AND ($5::text         IS NULL OR attribute = $5)
        ORDER BY time DESC
        LIMIT $6
        "#,
    )
    .bind(&entity_type)
    .bind(params.from)
    .bind(params.to)
    .bind(params.entity_id)
    .bind(params.attribute.as_deref())
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("ts range failed: {e}")))?;

    let data: Vec<Value> = rows.iter().map(row_to_json).collect();
    let body = json!({ "data": data, "count": data.len() });
    Ok(negotiate_response_pub(&headers, &body))
}

/// Helper: serialize a `time_series` row to JSON.
fn row_to_json(row: &PgRow) -> Value {
    let time: chrono::DateTime<chrono::Utc> = row.try_get("time").unwrap_or_else(|_| chrono::Utc::now());
    let entity_id: Uuid = row.try_get("entity_id").unwrap_or_else(|_| Uuid::nil());
    let entity_type: String = row.try_get("entity_type").unwrap_or_default();
    let attribute: String = row.try_get("attribute").unwrap_or_default();
    let value_num: Option<f64> = row.try_get("value_num").ok();
    let value_text: Option<String> = row.try_get("value_text").ok();
    let value_json: Option<Value> = row.try_get("value_json").ok();
    let tags: Value = row.try_get("tags").unwrap_or_else(|_| json!({}));
    json!({
        "time": time,
        "entity_id": entity_id,
        "entity_type": entity_type,
        "attribute": attribute,
        "value_num": value_num,
        "value_text": value_text,
        "value_json": value_json,
        "tags": tags,
    })
}

/// Bucketed range query — tries `time_bucket` first, falls back to
/// `date_trunc` when the TimescaleDB extension is absent.
async fn run_bucketed(
    pool: &PgPool,
    entity_type: &str,
    params: &RangeParams,
    bucket: &'static str,
    limit: i64,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    // Primary: TimescaleDB `time_bucket`.
    let sql = format!(
        r#"
        SELECT
            time_bucket(INTERVAL '{bucket}', time) AS bucket,
            COUNT(*)            AS count,
            AVG(value_num)      AS avg,
            MIN(value_num)      AS min,
            MAX(value_num)      AS max,
            SUM(value_num)      AS sum
        FROM time_series
        WHERE entity_type = $1
          AND ($2::timestamptz IS NULL OR time >= $2)
          AND ($3::timestamptz IS NULL OR time <  $3)
          AND ($4::uuid         IS NULL OR entity_id = $4)
          AND ($5::text         IS NULL OR attribute = $5)
        GROUP BY bucket
        ORDER BY bucket DESC
        LIMIT $6
        "#,
    );

    let primary = sqlx::query(&sql)
        .bind(entity_type)
        .bind(params.from)
        .bind(params.to)
        .bind(params.entity_id)
        .bind(params.attribute.as_deref())
        .bind(limit)
        .fetch_all(pool)
        .await;

    let rows = match primary {
        Ok(rows) => rows,
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("42883") => {
            // `time_bucket` missing — fall back to `date_trunc`. Only
            // intervals expressible as a date_trunc field are supported
            // in this degraded path.
            let field = bucket_to_date_trunc_field(bucket).ok_or_else(|| {
                ApiError::bad_request(format!(
                    "bucket '{bucket}' requires TimescaleDB (time_bucket unavailable)"
                ))
            })?;
            let fallback = format!(
                r#"
                SELECT
                    date_trunc('{field}', time) AS bucket,
                    COUNT(*)       AS count,
                    AVG(value_num) AS avg,
                    MIN(value_num) AS min,
                    MAX(value_num) AS max,
                    SUM(value_num) AS sum
                FROM time_series
                WHERE entity_type = $1
                  AND ($2::timestamptz IS NULL OR time >= $2)
                  AND ($3::timestamptz IS NULL OR time <  $3)
                  AND ($4::uuid         IS NULL OR entity_id = $4)
                  AND ($5::text         IS NULL OR attribute = $5)
                GROUP BY bucket
                ORDER BY bucket DESC
                LIMIT $6
                "#,
            );
            sqlx::query(&fallback)
                .bind(entity_type)
                .bind(params.from)
                .bind(params.to)
                .bind(params.entity_id)
                .bind(params.attribute.as_deref())
                .bind(limit)
                .fetch_all(pool)
                .await
                .map_err(|e| ApiError::internal(format!("ts bucket fallback failed: {e}")))?
        }
        Err(e) => return Err(ApiError::internal(format!("ts bucket failed: {e}"))),
    };

    let data: Vec<Value> = rows.iter().map(bucket_row_to_json).collect();
    let body = json!({ "bucket": bucket, "data": data, "count": data.len() });
    Ok(negotiate_response_pub(headers, &body))
}

fn bucket_row_to_json(row: &PgRow) -> Value {
    let bucket: chrono::DateTime<chrono::Utc> =
        row.try_get("bucket").unwrap_or_else(|_| chrono::Utc::now());
    let count: i64 = row.try_get("count").unwrap_or(0);
    let avg: Option<f64> = row.try_get("avg").ok().flatten();
    let min: Option<f64> = row.try_get("min").ok().flatten();
    let max: Option<f64> = row.try_get("max").ok().flatten();
    let sum: Option<f64> = row.try_get("sum").ok().flatten();
    json!({
        "bucket": bucket,
        "count": count,
        "avg": avg,
        "min": min,
        "max": max,
        "sum": sum,
    })
}

// ---------------------------------------------------------------------------
// Aggregate
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AggParams {
    #[serde(rename = "fn")]
    func: String,
    bucket: String,
    #[serde(default)]
    from: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    attribute: Option<String>,
    #[serde(default)]
    entity_id: Option<Uuid>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct AggRow {
    bucket: chrono::DateTime<chrono::Utc>,
    value: Option<f64>,
    count: i64,
}

/// `GET /api/ts/:entity_type/agg` — focused aggregation endpoint.
async fn ts_aggregate(
    State(state): State<AppState>,
    Path(entity_type): Path<String>,
    Query(params): Query<AggParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    validate_entity_type(&entity_type)?;
    if let Some(attr) = &params.attribute {
        validate_attribute(attr)?;
    }
    let bucket = canonicalize_bucket(&params.bucket)?;
    let (agg_sql, numeric) = canonicalize_agg(&params.func)?;
    let limit = params.limit.unwrap_or(1000).clamp(1, 10_000);

    let sql = format!(
        r#"
        SELECT
            time_bucket(INTERVAL '{bucket}', time) AS bucket,
            {agg_sql}::double precision            AS value,
            COUNT(*)                               AS count
        FROM time_series
        WHERE entity_type = $1
          AND ($2::timestamptz IS NULL OR time >= $2)
          AND ($3::timestamptz IS NULL OR time <  $3)
          AND ($4::uuid         IS NULL OR entity_id = $4)
          AND ($5::text         IS NULL OR attribute = $5)
        GROUP BY bucket
        ORDER BY bucket DESC
        LIMIT $6
        "#,
    );

    let primary = sqlx::query(&sql)
        .bind(&entity_type)
        .bind(params.from)
        .bind(params.to)
        .bind(params.entity_id)
        .bind(params.attribute.as_deref())
        .bind(limit)
        .fetch_all(&state.pool)
        .await;

    let rows = match primary {
        Ok(rows) => rows,
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("42883") => {
            let field = bucket_to_date_trunc_field(bucket).ok_or_else(|| {
                ApiError::bad_request(format!(
                    "bucket '{bucket}' requires TimescaleDB (time_bucket unavailable)"
                ))
            })?;
            let fallback = format!(
                r#"
                SELECT
                    date_trunc('{field}', time)          AS bucket,
                    {agg_sql}::double precision          AS value,
                    COUNT(*)                             AS count
                FROM time_series
                WHERE entity_type = $1
                  AND ($2::timestamptz IS NULL OR time >= $2)
                  AND ($3::timestamptz IS NULL OR time <  $3)
                  AND ($4::uuid         IS NULL OR entity_id = $4)
                  AND ($5::text         IS NULL OR attribute = $5)
                GROUP BY bucket
                ORDER BY bucket DESC
                LIMIT $6
                "#,
            );
            sqlx::query(&fallback)
                .bind(&entity_type)
                .bind(params.from)
                .bind(params.to)
                .bind(params.entity_id)
                .bind(params.attribute.as_deref())
                .bind(limit)
                .fetch_all(&state.pool)
                .await
                .map_err(|e| ApiError::internal(format!("ts agg fallback failed: {e}")))?
        }
        Err(e) => return Err(ApiError::internal(format!("ts agg failed: {e}"))),
    };

    let data: Vec<AggRow> = rows
        .iter()
        .map(|row| AggRow {
            bucket: row
                .try_get("bucket")
                .unwrap_or_else(|_| chrono::Utc::now()),
            value: row.try_get("value").ok().flatten(),
            count: row.try_get("count").unwrap_or(0),
        })
        .collect();

    let body = json!({
        "entity_type": entity_type,
        "fn": params.func,
        "numeric": numeric,
        "bucket": bucket,
        "data": data,
        "count": data.len(),
    });
    Ok(negotiate_response_pub(&headers, &body))
}

// ---------------------------------------------------------------------------
// Latest per entity
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
struct LatestParams {
    #[serde(default)]
    attribute: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

/// `GET /api/ts/:entity_type/latest` — last value per entity.
async fn ts_latest(
    State(state): State<AppState>,
    Path(entity_type): Path<String>,
    Query(params): Query<LatestParams>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    validate_entity_type(&entity_type)?;
    if let Some(attr) = &params.attribute {
        validate_attribute(attr)?;
    }
    let limit = params.limit.unwrap_or(1000).clamp(1, 10_000);

    let rows = sqlx::query(
        r#"
        SELECT DISTINCT ON (entity_id)
            time, entity_id, entity_type, attribute, value_num, value_text, value_json, tags
        FROM time_series
        WHERE entity_type = $1
          AND ($2::text IS NULL OR attribute = $2)
        ORDER BY entity_id, time DESC
        LIMIT $3
        "#,
    )
    .bind(&entity_type)
    .bind(params.attribute.as_deref())
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("ts latest failed: {e}")))?;

    let data: Vec<Value> = rows.iter().map(row_to_json).collect();
    let body = json!({ "data": data, "count": data.len() });
    Ok(negotiate_response_pub(&headers, &body))
}

// ---------------------------------------------------------------------------
// DarshanQL keyword extension — TIMESERIES(type,from,to,bucket,fn)
// ---------------------------------------------------------------------------
//
// TODO(Phase 5.2, Darshankumar Joshi): wire a real TIMESERIES keyword
// into `packages/server/src/query/darshql/parser.rs`. The lexer there
// uses a hand-written `lex_ident_or_kw` match table and the executor
// maps AST nodes onto the triple store — both need a new statement
// variant. Until that lands, `darshql_timeseries_sql` below exposes a
// direct SQL path so server-side functions / admin tooling can still
// run the underlying query without going through the REST layer.

/// Build a parameterized aggregation SQL for the TIMESERIES shorthand.
///
/// Returns `(sql, bucket_canonical, fn_fragment)`. Caller binds
/// `$1 = entity_type`, `$2 = from`, `$3 = to` in that order.
pub fn darshql_timeseries_sql(
    bucket: &str,
    func: &str,
) -> Result<(String, &'static str, &'static str), ApiError> {
    let bucket = canonicalize_bucket(bucket)?;
    let (agg_sql, _numeric) = canonicalize_agg(func)?;
    let sql = format!(
        r#"
        SELECT
            time_bucket(INTERVAL '{bucket}', time) AS bucket,
            {agg_sql}::double precision            AS value,
            COUNT(*)                               AS count
        FROM time_series
        WHERE entity_type = $1
          AND ($2::timestamptz IS NULL OR time >= $2)
          AND ($3::timestamptz IS NULL OR time <  $3)
        GROUP BY bucket
        ORDER BY bucket ASC
        "#,
    );
    Ok((sql, bucket, agg_sql))
}

// ---------------------------------------------------------------------------
// Unit tests (pure helpers — no DB required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn validates_entity_type() {
        assert!(validate_entity_type("sensor").is_ok());
        assert!(validate_entity_type("sensor_temp").is_ok());
        assert!(validate_entity_type("sensor-1").is_ok());
        assert!(validate_entity_type("").is_err());
        assert!(validate_entity_type("1sensor").is_err());
        assert!(validate_entity_type("sensor;DROP TABLE").is_err());
    }

    #[test]
    fn canonicalizes_bucket() {
        assert_eq!(canonicalize_bucket("1h").unwrap(), "1 hour");
        assert_eq!(canonicalize_bucket("HOUR").unwrap(), "1 hour");
        assert_eq!(canonicalize_bucket("7d").unwrap(), "7 days");
        assert!(canonicalize_bucket("10 years").is_err());
    }

    #[test]
    fn canonicalizes_agg() {
        assert_eq!(canonicalize_agg("avg").unwrap().0, "AVG(value_num)");
        assert_eq!(canonicalize_agg("COUNT").unwrap().0, "COUNT(*)");
        assert!(canonicalize_agg("median").is_err());
    }

    #[test]
    fn darshql_sql_builds_with_whitelisted_inputs() {
        let (sql, bucket, _agg) = darshql_timeseries_sql("1h", "avg").unwrap();
        assert!(sql.contains("time_bucket(INTERVAL '1 hour'"));
        assert!(sql.contains("AVG(value_num)"));
        assert_eq!(bucket, "1 hour");
    }

    #[test]
    fn darshql_sql_rejects_bogus_inputs() {
        assert!(darshql_timeseries_sql("1y", "avg").is_err());
        assert!(darshql_timeseries_sql("1h", "median").is_err());
    }
}
