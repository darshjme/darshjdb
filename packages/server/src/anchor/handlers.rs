//! HTTP handlers for the blockchain anchor audit trail.
//!
//! Exposes one admin endpoint:
//!
//! - `GET /api/admin/audit/anchors?limit=&offset=` — paginated list of
//!   every anchor receipt, newest first.
//!
//! Author: Darshankumar Joshi

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use crate::api::rest::AppState;

/// Query-string params for the paginated anchor list endpoint.
#[derive(Debug, Deserialize)]
pub struct AnchorListParams {
    /// Page size. Capped at 500 so a misconfigured client can't DOS
    /// the server by asking for the full table.
    #[serde(default = "default_limit")]
    pub limit: i64,
    /// Pagination offset in rows, newest-first.
    #[serde(default)]
    pub offset: i64,
}

fn default_limit() -> i64 {
    50
}

/// `GET /api/admin/audit/anchors`
///
/// Lists `anchor_receipts` newest-first. Requires the caller to hold
/// the `admin` role — enforced by [`crate::api::rest::require_admin_role`]
/// the same way the existing Merkle audit endpoints are.
pub async fn admin_list_anchors(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<AnchorListParams>,
) -> Result<Response, StatusCode> {
    // Admin-only: delegate to the shared role check in rest.rs so this
    // endpoint stays consistent with every other `/admin/*` route.
    crate::api::rest::require_admin_role(&headers).map_err(|_| StatusCode::FORBIDDEN)?;

    // Clamp parameters to sane bounds before touching SQL.
    let limit = params.limit.clamp(1, 500);
    let offset = params.offset.max(0);

    let receipts = super::list_receipts(&state.pool, limit, offset)
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to list anchor receipts");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let count = receipts.len();
    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "receipts": receipts,
            "count": count,
            "limit": limit,
            "offset": offset,
        })),
    )
        .into_response())
}
