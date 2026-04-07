//! API endpoints for aggregation, summary, and chart queries.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use crate::api::error::ApiError;

use super::chart::{self, ChartQuery};
use super::engine::{AggregateQuery, AggregationEngine};

/// Build the aggregation route group.
///
/// All routes are mounted under `/api/aggregate` by the caller.
pub fn aggregation_routes<S: Clone + Send + Sync + 'static>() -> Router<AggregationState> {
    Router::new()
        .route("/", post(aggregate_handler))
        .route("/summary", post(summary_handler))
        .route("/chart", post(chart_handler))
}

/// Shared state for aggregation handlers.
#[derive(Clone)]
pub struct AggregationState {
    pub engine: AggregationEngine,
    pub pool: sqlx::PgPool,
}

// ── Handlers ───────────────────────────────────────────────────────

/// `POST /api/aggregate` — Execute an aggregation query.
///
/// # Request Body
///
/// ```json
/// {
///   "entity_type": "Order",
///   "group_by": ["status"],
///   "aggregations": [
///     { "field": "amount", "function": { "fn": "Sum" }, "alias": "total_amount" },
///     { "field": "id", "function": { "fn": "Count" }, "alias": "order_count" }
///   ],
///   "filters": [],
///   "having": { "alias": "total_amount", "op": "Gt", "value": 1000 }
/// }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "groups": [
///     {
///       "key": { "status": "completed" },
///       "values": { "total_amount": 45000, "order_count": 120 },
///       "count": 120
///     }
///   ],
///   "totals": { "total_amount": 90000, "order_count": 500 }
/// }
/// ```
async fn aggregate_handler(
    State(state): State<AggregationState>,
    Json(query): Json<AggregateQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let result = state
        .engine
        .execute(&query)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

/// `POST /api/aggregate/summary` — Quick summary for all numeric fields.
///
/// # Request Body
///
/// ```json
/// { "entity_type": "Invoice" }
/// ```
///
/// # Response
///
/// Returns count, sum, avg, min, max for each attribute, plus
/// count_empty and count_filled.
async fn summary_handler(
    State(state): State<AggregationState>,
    Json(body): Json<SummaryRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if body.entity_type.is_empty() {
        return Err(ApiError::bad_request("entity_type must not be empty"));
    }

    let result = state
        .engine
        .summary(&body.entity_type)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

/// `POST /api/aggregate/chart` — Time-series aggregation for charts.
///
/// # Request Body
///
/// ```json
/// {
///   "entity_type": "Order",
///   "date_field": "created_at",
///   "value_field": "amount",
///   "function": "sum",
///   "bucket": "month",
///   "group_by": "region",
///   "filters": []
/// }
/// ```
///
/// # Response
///
/// ```json
/// {
///   "buckets": [
///     {
///       "label": "2025-01-01",
///       "start": "2025-01-01T00:00:00Z",
///       "end": "2025-02-01T00:00:00Z",
///       "value": 15000,
///       "series": "US"
///     }
///   ]
/// }
/// ```
async fn chart_handler(
    State(state): State<AggregationState>,
    Json(query): Json<ChartQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let result = chart::execute_chart_query(&state.pool, &query)
        .await
        .map_err(ApiError::from)?;

    Ok(Json(result))
}

// ── Request types ──────────────────────────────────────────────────

/// Request body for the summary endpoint.
#[derive(serde::Deserialize)]
struct SummaryRequest {
    entity_type: String,
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_request_deserializes() {
        let json = r#"{"entity_type": "Invoice"}"#;
        let req: SummaryRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.entity_type, "Invoice");
    }

    #[test]
    fn aggregate_query_deserializes_full() {
        let json = r#"{
            "entity_type": "Order",
            "group_by": ["status", "region"],
            "aggregations": [
                { "field": "amount", "function": { "fn": "Sum" }, "alias": "total" },
                { "field": "amount", "function": { "fn": "Avg" }, "alias": "average" },
                { "field": "id", "function": { "fn": "Count" }, "alias": "cnt" },
                { "field": "id", "function": { "fn": "CountDistinct" }, "alias": "unique" },
                { "field": "amount", "function": { "fn": "Percentile", "arg": 0.95 }, "alias": "p95" }
            ],
            "filters": [
                { "attribute": "status", "op": "Eq", "value": "active" }
            ],
            "having": { "alias": "total", "op": "Gt", "value": 1000 }
        }"#;

        let query: AggregateQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.entity_type, "Order");
        assert_eq!(query.group_by.len(), 2);
        assert_eq!(query.aggregations.len(), 5);
        assert!(query.having.is_some());
        assert_eq!(query.filters.len(), 1);
    }

    #[test]
    fn chart_query_deserializes() {
        let json = r#"{
            "entity_type": "Order",
            "date_field": "created_at",
            "value_field": "amount",
            "function": "sum",
            "bucket": "month",
            "group_by": "region"
        }"#;

        let query: ChartQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.entity_type, "Order");
        assert_eq!(query.date_field, "created_at");
        assert_eq!(query.bucket, super::super::chart::TimeBucket::Month);
        assert_eq!(query.group_by, Some("region".into()));
    }

    #[test]
    fn aggregate_result_serializes() {
        use std::collections::HashMap;

        let result = AggregateResult {
            groups: vec![super::super::engine::AggGroup {
                key: {
                    let mut m = HashMap::new();
                    m.insert("status".into(), Value::String("active".into()));
                    m
                },
                values: {
                    let mut m = HashMap::new();
                    m.insert("total".into(), Value::Number(5000.into()));
                    m.insert("count".into(), Value::Number(25.into()));
                    m
                },
                count: 25,
            }],
            totals: {
                let mut m = HashMap::new();
                m.insert("total".into(), Value::Number(10000.into()));
                m
            },
        };

        let json = serde_json::to_value(&result).unwrap();
        assert!(json["groups"].is_array());
        assert_eq!(json["groups"][0]["count"], 25);
        assert!(json["totals"]["total"].is_number());
    }
}
