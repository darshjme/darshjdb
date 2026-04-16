//! Graph handlers: SurrealDB-style record links and traversal.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;

use crate::api::error::{ApiError, ErrorCode};
use crate::api::rest::AppState;
use crate::graph::{Edge, EdgeInput, GraphEngine, RecordId, TraversalConfig};

use super::helpers::{negotiate_response, negotiate_response_status};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract the graph engine from state, returning 501 if not configured.
fn require_graph_engine(state: &AppState) -> Result<&GraphEngine, ApiError> {
    state
        .graph_engine
        .as_ref()
        .map(|g| g.as_ref())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::Internal,
                "Graph engine is not enabled on this server",
            )
        })
}

/// Serialize a list of edges into the standard JSON representation.
fn serialize_edges(edges: &[Edge]) -> Vec<serde_json::Value> {
    edges
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.id,
                "from": format!("{}:{}", e.from_table, e.from_id),
                "edge_type": e.edge_type,
                "to": format!("{}:{}", e.to_table, e.to_id),
                "data": e.data,
                "created_at": e.created_at,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Relate
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct GraphRelateRequest {
    from: String,
    edge_type: String,
    to: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// `POST /graph/relate` -- Create a directed edge between two records.
pub async fn graph_relate(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<GraphRelateRequest>,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;

    let input = EdgeInput {
        from: body.from,
        edge_type: body.edge_type,
        to: body.to,
        data: body.data,
    };

    let edge = engine
        .relate(&input)
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "edge": {
            "id": edge.id,
            "from": format!("{}:{}", edge.from_table, edge.from_id),
            "edge_type": edge.edge_type,
            "to": format!("{}:{}", edge.to_table, edge.to_id),
            "data": edge.data,
            "created_at": edge.created_at,
        }
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Traverse
// ---------------------------------------------------------------------------

/// `POST /graph/traverse` -- Execute a graph traversal from a starting node.
pub async fn graph_traverse(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(config): axum::Json<TraversalConfig>,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;

    let result = engine
        .traverse(&config)
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    Ok(negotiate_response(&headers, &result))
}

// ---------------------------------------------------------------------------
// Neighbors / outgoing / incoming
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct GraphEdgeQuery {
    #[serde(default)]
    edge_type: Option<String>,
}

/// `GET /graph/neighbors/:table/:id` -- Get all edges (both directions) for a record.
pub async fn graph_neighbors(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, String)>,
    Query(query): Query<GraphEdgeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;
    let record = RecordId::new(&table, &id);

    let edges = engine
        .neighbors(&record, query.edge_type.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "record": record.to_string_repr(),
        "edges": serialize_edges(&edges),
        "count": edges.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /graph/outgoing/:table/:id` -- Get outgoing edges from a record.
pub async fn graph_outgoing(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, String)>,
    Query(query): Query<GraphEdgeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;
    let record = RecordId::new(&table, &id);

    let edges = engine
        .outgoing(&record, query.edge_type.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "record": record.to_string_repr(),
        "direction": "out",
        "edges": serialize_edges(&edges),
        "count": edges.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

/// `GET /graph/incoming/:table/:id` -- Get incoming edges to a record.
pub async fn graph_incoming(
    State(state): State<AppState>,
    Path((table, id)): Path<(String, String)>,
    Query(query): Query<GraphEdgeQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;
    let record = RecordId::new(&table, &id);

    let edges = engine
        .incoming(&record, query.edge_type.as_deref())
        .await
        .map_err(|e| ApiError::bad_request(format!("{e}")))?;

    let response = serde_json::json!({
        "record": record.to_string_repr(),
        "direction": "in",
        "edges": serialize_edges(&edges),
        "count": edges.len(),
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Delete edge
// ---------------------------------------------------------------------------

/// `DELETE /graph/edge/:edge_id` -- Delete an edge by its UUID.
pub async fn graph_delete_edge(
    State(state): State<AppState>,
    Path(edge_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let engine = require_graph_engine(&state)?;

    let deleted = engine
        .delete_edge(edge_id)
        .await
        .map_err(|e| ApiError::internal(format!("{e}")))?;

    if !deleted {
        return Err(ApiError::not_found(format!("edge {edge_id} not found")));
    }

    let response = serde_json::json!({
        "deleted": true,
        "edge_id": edge_id,
    });

    Ok(negotiate_response(&headers, &response))
}
