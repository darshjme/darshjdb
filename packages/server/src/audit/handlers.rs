//! HTTP handlers for the Merkle tree audit trail.
//!
//! Provides three admin endpoints:
//!
//! - `GET /api/admin/audit/verify/:tx_id` — verify a single tx's root
//! - `GET /api/admin/audit/chain` — verify the entire hash chain
//! - `GET /api/admin/audit/proof/:entity_id` — Merkle proof for an entity

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

use crate::api::rest::AppState;

/// `GET /api/admin/audit/verify/:tx_id`
///
/// Recomputes the Merkle root for the given transaction from its stored
/// triples and compares against the recorded root. Returns whether the
/// transaction data is intact.
pub async fn audit_verify_tx(
    State(state): State<AppState>,
    Path(tx_id): Path<i64>,
    _headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let result = super::verify_tx(&state.pool, tx_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let status = if result.valid {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };

    Ok((
        status,
        axum::Json(json!({
            "tx_id": result.tx_id,
            "valid": result.valid,
            "detail": result.detail,
            "stored_root": result.stored_root,
            "computed_root": result.computed_root,
            "triple_count": result.triple_count,
        })),
    )
        .into_response())
}

/// `GET /api/admin/audit/chain`
///
/// Walks the entire `tx_merkle_roots` table and verifies that each
/// transaction's `prev_root` matches the previous transaction's
/// `chained_root`. This is the Bitcoin-style chain verification.
pub async fn audit_verify_chain(
    State(state): State<AppState>,
    _headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let result = super::verify_chain(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let status = if result.valid {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };

    Ok((
        status,
        axum::Json(json!({
            "valid": result.valid,
            "total_transactions": result.total_transactions,
            "first_broken_tx": result.first_broken_tx,
            "detail": result.detail,
        })),
    )
        .into_response())
}

/// `GET /api/admin/audit/proof/:entity_id`
///
/// Returns Merkle inclusion proofs for every triple belonging to the
/// given entity, grouped by transaction. Each proof can be independently
/// verified against the transaction's Merkle root.
pub async fn audit_entity_proof(
    State(state): State<AppState>,
    Path(entity_id): Path<Uuid>,
    _headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let proofs = super::entity_proof(&state.pool, entity_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if proofs.is_empty() {
        return Ok((
            StatusCode::NOT_FOUND,
            axum::Json(json!({
                "error": "No triples found for this entity",
                "entity_id": entity_id.to_string(),
            })),
        )
            .into_response());
    }

    Ok((StatusCode::OK, axum::Json(json!({ "proofs": proofs }))).into_response())
}
