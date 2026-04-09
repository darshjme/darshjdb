//! HTTP API endpoints for relational field operations.
//!
//! All handlers follow DarshJDB's REST conventions:
//! - JSON request/response bodies
//! - `ApiError` for uniform error envelopes
//! - Auth via the `require_auth_middleware` layer

use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::error::{ApiError, ErrorCode};
use crate::api::rest::AppState;

use super::cascade;
use super::link::{self, Relationship};
use super::lookup::{self, LookupConfig};
use super::rollup::{self, RollupConfig, RollupFn};

// ── Request / response types ───────────────────────────────────────

/// Request body for adding a link.
#[derive(Debug, Deserialize)]
pub struct AddLinkRequest {
    /// The target entity ID to link to.
    pub target_id: Uuid,
    /// The link attribute name.
    pub attribute: String,
    /// Relationship type (defaults to one_to_many).
    #[serde(default = "default_relationship")]
    pub relationship: Relationship,
    /// Whether to create a symmetric backlink.
    #[serde(default)]
    pub symmetric: bool,
    /// Backlink attribute name (required if symmetric is true).
    pub backlink_name: Option<String>,
}

fn default_relationship() -> Relationship {
    Relationship::OneToMany
}

/// Request body for removing a link.
#[derive(Debug, Deserialize)]
pub struct RemoveLinkRequest {
    /// The target entity ID to unlink.
    pub target_id: Uuid,
    /// The link attribute name.
    pub attribute: String,
    /// Relationship type.
    #[serde(default = "default_relationship")]
    pub relationship: Relationship,
    /// Whether the link is symmetric (needs backlink cleanup).
    #[serde(default)]
    pub symmetric: bool,
    /// Backlink attribute name.
    pub backlink_name: Option<String>,
}

/// Response for link operations.
#[derive(Debug, Serialize)]
pub struct LinkResponse {
    pub ok: bool,
    pub tx_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Response for linked entity queries.
#[derive(Debug, Serialize)]
pub struct LinkedEntitiesResponse {
    pub entity_id: Uuid,
    pub attribute: String,
    pub linked_ids: Vec<Uuid>,
    pub count: usize,
}

/// Response for lookup resolution.
#[derive(Debug, Serialize)]
pub struct LookupResponse {
    pub entity_id: Uuid,
    pub field: String,
    pub values: Vec<serde_json::Value>,
}

/// Query parameters for lookup resolution.
#[derive(Debug, Deserialize)]
pub struct LookupQuery {
    /// The link attribute to follow.
    pub link_field: String,
    /// The field to read from the linked entity.
    pub lookup_field: String,
}

/// Query parameters for rollup computation.
#[derive(Debug, Deserialize)]
pub struct RollupQuery {
    /// The link attribute to follow.
    pub link_field: String,
    /// The field to aggregate from linked entities.
    pub rollup_field: String,
    /// Aggregation function name.
    pub function: String,
    /// Separator for array_join (optional).
    pub separator: Option<String>,
}

/// Response for rollup computation.
#[derive(Debug, Serialize)]
pub struct RollupResponse {
    pub entity_id: Uuid,
    pub field: String,
    pub function: String,
    pub value: serde_json::Value,
}

// ── Handlers ───────────────────────────────────────────────────────

/// `POST /api/data/{entity}/{id}/link`
///
/// Add a link from the source entity to a target entity.
pub async fn add_link_handler(
    State(state): State<AppState>,
    Path((_entity, id)): Path<(String, Uuid)>,
    Json(body): Json<AddLinkRequest>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    let tx_id = link::add_link(
        &state.pool,
        id,
        body.target_id,
        &body.attribute,
        body.relationship,
        body.symmetric,
        body.backlink_name.as_deref(),
    )
    .await
    .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

    // Cascade: invalidate caches for both sides of the link.
    let _ = cascade::cascade_link_change(
        &state.pool,
        id,
        body.target_id,
        &body.attribute,
        None, // TODO: wire lookup cache from AppState
        Some(&state.change_tx),
    )
    .await;

    Ok(Json(LinkResponse {
        ok: true,
        tx_id: Some(tx_id),
        message: None,
    }))
}

/// `DELETE /api/data/{entity}/{id}/link`
///
/// Remove a link from the source entity to a target entity.
pub async fn remove_link_handler(
    State(state): State<AppState>,
    Path((_entity, id)): Path<(String, Uuid)>,
    Json(body): Json<RemoveLinkRequest>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    link::remove_link(
        &state.pool,
        id,
        body.target_id,
        &body.attribute,
        body.relationship,
        body.symmetric,
        body.backlink_name.as_deref(),
    )
    .await
    .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

    // Cascade.
    let _ = cascade::cascade_link_change(
        &state.pool,
        id,
        body.target_id,
        &body.attribute,
        None,
        Some(&state.change_tx),
    )
    .await;

    Ok(Json(LinkResponse {
        ok: true,
        tx_id: None,
        message: Some("link removed".into()),
    }))
}

/// `GET /api/data/{entity}/{id}/linked/{attribute}`
///
/// Get all entity IDs linked from the source via the given attribute.
pub async fn get_linked_handler(
    State(state): State<AppState>,
    Path((_entity, id, attribute)): Path<(String, Uuid, String)>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    let linked_ids = link::get_linked(&state.pool, id, &attribute)
        .await
        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

    let count = linked_ids.len();
    Ok(Json(LinkedEntitiesResponse {
        entity_id: id,
        attribute,
        linked_ids,
        count,
    }))
}

/// `GET /api/data/{entity}/{id}/lookup/{field}`
///
/// Resolve a lookup field value by following a link and reading the
/// target entity's field.
///
/// Query params: `link_field` and `lookup_field`.
pub async fn resolve_lookup_handler(
    State(state): State<AppState>,
    Path((_entity, id, _field)): Path<(String, Uuid, String)>,
    axum::extract::Query(query): axum::extract::Query<LookupQuery>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    let config = LookupConfig {
        link_field: query.link_field.clone(),
        lookup_field: query.lookup_field.clone(),
    };

    let values = lookup::resolve_lookup(&state.pool, id, &config, None)
        .await
        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

    Ok(Json(LookupResponse {
        entity_id: id,
        field: query.lookup_field,
        values,
    }))
}

/// `GET /api/data/{entity}/{id}/rollup/{field}`
///
/// Compute a rollup field value by following a link, collecting target
/// field values, and applying an aggregation function.
///
/// Query params: `link_field`, `rollup_field`, `function`, optional `separator`.
pub async fn compute_rollup_handler(
    State(state): State<AppState>,
    Path((_entity, id, _field)): Path<(String, Uuid, String)>,
    axum::extract::Query(query): axum::extract::Query<RollupQuery>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    let function = parse_rollup_fn(&query.function, query.separator.as_deref())?;

    let config = RollupConfig {
        link_field: query.link_field.clone(),
        rollup_field: query.rollup_field.clone(),
        function: function.clone(),
    };

    let value = rollup::compute_rollup(&state.pool, id, &config)
        .await
        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

    Ok(Json(RollupResponse {
        entity_id: id,
        field: query.rollup_field,
        function: query.function,
        value,
    }))
}

// ── Route builder ──────────────────────────────────────────────────

/// Build the relation routes to be merged into the main router.
///
/// All routes are nested under `/api/data/{entity}/{id}/...`.
pub fn relation_routes() -> axum::Router<AppState> {
    use axum::routing::{get, post};

    axum::Router::new()
        .route(
            "/data/{entity}/{id}/link",
            post(add_link_handler).delete(remove_link_handler),
        )
        .route(
            "/data/{entity}/{id}/linked/{attribute}",
            get(get_linked_handler),
        )
        .route(
            "/data/{entity}/{id}/lookup/{field}",
            get(resolve_lookup_handler),
        )
        .route(
            "/data/{entity}/{id}/rollup/{field}",
            get(compute_rollup_handler),
        )
}

// ── Helpers ────────────────────────────────────────────────────────

/// Parse a rollup function name string into the enum.
fn parse_rollup_fn(name: &str, separator: Option<&str>) -> std::result::Result<RollupFn, ApiError> {
    match name {
        "count" => Ok(RollupFn::Count),
        "sum" => Ok(RollupFn::Sum),
        "average" | "avg" => Ok(RollupFn::Average),
        "min" => Ok(RollupFn::Min),
        "max" => Ok(RollupFn::Max),
        "count_all" => Ok(RollupFn::CountAll),
        "count_values" => Ok(RollupFn::CountValues),
        "count_empty" => Ok(RollupFn::CountEmpty),
        "array_join" => {
            let sep = separator.unwrap_or(", ").to_string();
            Ok(RollupFn::ArrayJoin(sep))
        }
        "concatenate" | "concat" => Ok(RollupFn::Concatenate),
        other => Err(ApiError::bad_request(format!(
            "unknown rollup function: '{other}'"
        ))),
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rollup_fn_all_variants() {
        assert_eq!(parse_rollup_fn("count", None).unwrap(), RollupFn::Count);
        assert_eq!(parse_rollup_fn("sum", None).unwrap(), RollupFn::Sum);
        assert_eq!(parse_rollup_fn("average", None).unwrap(), RollupFn::Average);
        assert_eq!(parse_rollup_fn("avg", None).unwrap(), RollupFn::Average);
        assert_eq!(parse_rollup_fn("min", None).unwrap(), RollupFn::Min);
        assert_eq!(parse_rollup_fn("max", None).unwrap(), RollupFn::Max);
        assert_eq!(
            parse_rollup_fn("count_all", None).unwrap(),
            RollupFn::CountAll
        );
        assert_eq!(
            parse_rollup_fn("count_values", None).unwrap(),
            RollupFn::CountValues
        );
        assert_eq!(
            parse_rollup_fn("count_empty", None).unwrap(),
            RollupFn::CountEmpty
        );
        assert_eq!(
            parse_rollup_fn("array_join", Some(" | ")).unwrap(),
            RollupFn::ArrayJoin(" | ".into())
        );
        assert_eq!(
            parse_rollup_fn("array_join", None).unwrap(),
            RollupFn::ArrayJoin(", ".into())
        );
        assert_eq!(
            parse_rollup_fn("concatenate", None).unwrap(),
            RollupFn::Concatenate
        );
        assert_eq!(
            parse_rollup_fn("concat", None).unwrap(),
            RollupFn::Concatenate
        );
    }

    #[test]
    fn parse_rollup_fn_unknown() {
        let err = parse_rollup_fn("foobar", None).unwrap_err();
        assert!(matches!(err.code, ErrorCode::BadRequest));
        assert!(err.message.contains("foobar"));
    }

    #[test]
    fn add_link_request_defaults() {
        let json = r#"{"target_id": "00000000-0000-0000-0000-000000000001", "attribute": "tasks"}"#;
        let req: AddLinkRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.relationship, Relationship::OneToMany);
        assert!(!req.symmetric);
        assert!(req.backlink_name.is_none());
    }

    #[test]
    fn add_link_request_full() {
        let json = r#"{
            "target_id": "00000000-0000-0000-0000-000000000001",
            "attribute": "tasks",
            "relationship": "many_to_many",
            "symmetric": true,
            "backlink_name": "project"
        }"#;
        let req: AddLinkRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.relationship, Relationship::ManyToMany);
        assert!(req.symmetric);
        assert_eq!(req.backlink_name.as_deref(), Some("project"));
    }

    #[test]
    fn link_response_serialization() {
        let resp = LinkResponse {
            ok: true,
            tx_id: Some(42),
            message: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json["ok"].as_bool().unwrap());
        assert_eq!(json["tx_id"], 42);
        // message should be absent when None.
        assert!(json.get("message").is_none());
    }

    #[test]
    fn linked_entities_response_serialization() {
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let resp = LinkedEntitiesResponse {
            entity_id: Uuid::nil(),
            attribute: "tasks".into(),
            linked_ids: vec![id1, id2],
            count: 2,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["count"], 2);
        assert_eq!(json["linked_ids"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn rollup_response_serialization() {
        let resp = RollupResponse {
            entity_id: Uuid::nil(),
            field: "hours".into(),
            function: "sum".into(),
            value: serde_json::json!(42.5),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["function"], "sum");
        assert_eq!(json["value"], 42.5);
    }
}
