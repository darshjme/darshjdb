//! HTTP handlers for record history, restore, and snapshot operations.
//!
//! # Endpoints
//!
//! ## Record History
//! - `GET  /api/data/{entity}/{id}/history`            — version history
//! - `GET  /api/data/{entity}/{id}/history/{version}`  — record at version
//! - `POST /api/data/{entity}/{id}/restore/{version}`  — restore to version
//! - `POST /api/data/{entity}/{id}/undo`               — undo last change
//! - `POST /api/data/{entity}/{id}/undelete`            — restore deleted record
//!
//! ## Snapshots
//! - `POST /api/snapshots`                             — create snapshot
//! - `GET  /api/snapshots?type={entity_type}`          — list snapshots
//! - `POST /api/snapshots/{id}/restore`                — restore snapshot
//! - `GET  /api/snapshots/{id}/diff`                   — diff since snapshot

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::api::rest::AppState;

// ── Query / body types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    /// Maximum number of versions to return (default: 50).
    #[serde(default = "default_limit")]
    pub limit: u32,
    /// Point-in-time filter (ISO 8601 timestamp).
    pub at: Option<DateTime<Utc>>,
}

fn default_limit() -> u32 {
    50
}

#[derive(Debug, Deserialize)]
pub struct SnapshotListQuery {
    /// Entity type to filter by (required).
    #[serde(rename = "type")]
    pub entity_type: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateSnapshotBody {
    /// Entity type (e.g. "user", "order").
    pub entity_type: String,
    /// Human-readable snapshot name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
}

// ── Record history handlers ───────────────────────────────────────

/// `GET /api/data/{entity}/{id}/history`
///
/// Returns the version history of a record. Supports `?limit=N` to cap
/// the number of versions returned, and `?at=<ISO8601>` for point-in-time.
pub async fn get_record_history(
    State(state): State<AppState>,
    Path((_entity, id)): Path<(String, Uuid)>,
    Query(query): Query<HistoryQuery>,
) -> Result<Response, StatusCode> {
    if let Some(timestamp) = query.at {
        let snapshot = super::get_at_time(&state.pool, id, timestamp)
            .await
            .map_err(|e| match e {
                crate::error::DarshJError::EntityNotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            })?;

        return Ok((
            StatusCode::OK,
            axum::Json(json!({
                "entity_id": id,
                "at": timestamp,
                "snapshot": snapshot,
            })),
        )
            .into_response());
    }

    let versions = super::get_history(&state.pool, id, query.limit)
        .await
        .map_err(|e| match e {
            crate::error::DarshJError::EntityNotFound(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "entity_id": id,
            "total_versions": versions.len(),
            "versions": versions,
        })),
    )
        .into_response())
}

/// `GET /api/data/{entity}/{id}/history/{version}`
///
/// Returns the complete record state at a specific version number.
pub async fn get_record_version(
    State(state): State<AppState>,
    Path((_entity, id, version)): Path<(String, Uuid, u32)>,
) -> Result<Response, StatusCode> {
    let snapshot = super::get_version(&state.pool, id, version)
        .await
        .map_err(|e| match e {
            crate::error::DarshJError::EntityNotFound(_) => StatusCode::NOT_FOUND,
            crate::error::DarshJError::InvalidQuery(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "entity_id": id,
            "version": version,
            "snapshot": snapshot,
        })),
    )
        .into_response())
}

/// `POST /api/data/{entity}/{id}/restore/{version}`
///
/// Restores a record to a previous version by writing new triples.
pub async fn restore_record_version(
    State(state): State<AppState>,
    Path((_entity, id, version)): Path<(String, Uuid, u32)>,
) -> Result<Response, StatusCode> {
    let tx_id = super::restore_version(&state.pool, id, version)
        .await
        .map_err(|e| match e {
            crate::error::DarshJError::EntityNotFound(_) => StatusCode::NOT_FOUND,
            crate::error::DarshJError::InvalidQuery(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "entity_id": id,
            "restored_to_version": version,
            "tx_id": tx_id,
        })),
    )
        .into_response())
}

/// `POST /api/data/{entity}/{id}/undo`
///
/// Undoes the most recent change to a record.
pub async fn undo_record(
    State(state): State<AppState>,
    Path((_entity, id)): Path<(String, Uuid)>,
) -> Result<Response, StatusCode> {
    let tx_id = super::undo_last(&state.pool, id)
        .await
        .map_err(|e| match e {
            crate::error::DarshJError::EntityNotFound(_) => StatusCode::NOT_FOUND,
            crate::error::DarshJError::InvalidQuery(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "entity_id": id,
            "undone": true,
            "tx_id": tx_id,
        })),
    )
        .into_response())
}

/// `POST /api/data/{entity}/{id}/undelete`
///
/// Restores a soft-deleted record by re-asserting its last known triples.
pub async fn undelete_record(
    State(state): State<AppState>,
    Path((_entity, id)): Path<(String, Uuid)>,
) -> Result<Response, StatusCode> {
    let tx_id = super::restore_deleted(&state.pool, id)
        .await
        .map_err(|e| match e {
            crate::error::DarshJError::EntityNotFound(_) => StatusCode::NOT_FOUND,
            crate::error::DarshJError::InvalidQuery(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "entity_id": id,
            "undeleted": true,
            "tx_id": tx_id,
        })),
    )
        .into_response())
}

// ── Snapshot handlers ─────────────────────────────────────────────

/// `POST /api/snapshots`
///
/// Creates a new snapshot checkpoint for an entity type.
pub async fn create_snapshot_handler(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<CreateSnapshotBody>,
) -> Result<Response, StatusCode> {
    let snapshot = super::create_snapshot(
        &state.pool,
        &body.entity_type,
        &body.name,
        &body.description,
        None, // TODO: extract from auth context
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((StatusCode::CREATED, axum::Json(json!(snapshot))).into_response())
}

/// `GET /api/snapshots?type={entity_type}`
///
/// Lists all snapshots for a given entity type.
pub async fn list_snapshots_handler(
    State(state): State<AppState>,
    Query(query): Query<SnapshotListQuery>,
) -> Result<Response, StatusCode> {
    let snapshots = super::list_snapshots(&state.pool, &query.entity_type)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "entity_type": query.entity_type,
            "count": snapshots.len(),
            "snapshots": snapshots,
        })),
    )
        .into_response())
}

/// `POST /api/snapshots/{id}/restore`
///
/// Restores all records of the snapshot's entity type to their state
/// at the snapshot's tx_id.
pub async fn restore_snapshot_handler(
    State(state): State<AppState>,
    Path(snapshot_id): Path<Uuid>,
) -> Result<Response, StatusCode> {
    let tx_id = super::restore_snapshot(&state.pool, snapshot_id)
        .await
        .map_err(|e| match e {
            crate::error::DarshJError::InvalidQuery(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    Ok((
        StatusCode::OK,
        axum::Json(json!({
            "snapshot_id": snapshot_id,
            "restored": true,
            "tx_id": tx_id,
        })),
    )
        .into_response())
}

/// `GET /api/snapshots/{id}/diff`
///
/// Shows what changed since the snapshot was taken.
pub async fn diff_snapshot_handler(
    State(state): State<AppState>,
    Path(snapshot_id): Path<Uuid>,
) -> Result<Response, StatusCode> {
    let diff = super::diff_snapshot(&state.pool, snapshot_id)
        .await
        .map_err(|e| match e {
            crate::error::DarshJError::InvalidQuery(_) => StatusCode::NOT_FOUND,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        })?;

    Ok((StatusCode::OK, axum::Json(json!(diff))).into_response())
}
