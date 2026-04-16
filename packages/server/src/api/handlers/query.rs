//! DarshQL query execution handler.

use std::time::Instant;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::rest::AppState;

use super::helpers::{extract_auth_context, negotiate_response};

// ---------------------------------------------------------------------------
// DarshQL handler
// ---------------------------------------------------------------------------

/// Request body for the `/sql` endpoint.
#[derive(Deserialize)]
pub struct DarshQLRequest {
    /// The DarshQL query string (one or more statements separated by `;`).
    query: String,
}

/// `POST /api/sql` -- Execute DarshQL statements.
///
/// Accepts a `{ "query": "SELECT * FROM users WHERE age > 18" }` body
/// and returns the results of each statement.
pub async fn darshql_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<DarshQLRequest>,
) -> Result<Response, ApiError> {
    let _auth_ctx = extract_auth_context(&headers, &state)?;

    let start = Instant::now();

    // Parse DarshQL into AST.
    let statements = crate::query::darshql::Parser::parse(&body.query)
        .map_err(|e| ApiError::bad_request(format!("DarshQL parse error: {e}")))?;

    if statements.is_empty() {
        return Err(ApiError::bad_request("empty query".to_string()));
    }

    // Execute all statements.
    let results = crate::query::darshql::execute(&state.pool, statements)
        .await
        .map_err(|e| ApiError::internal(format!("DarshQL execution error: {e}")))?;

    let elapsed = start.elapsed();
    let response_body = serde_json::json!({
        "results": results,
        "time": format!("{}ms", elapsed.as_millis()),
    });

    Ok(negotiate_response(&headers, &response_body))
}
