//! REST API endpoints for plugin management.
//!
//! All endpoints live under `/api/plugins` and require admin auth.
//!
//! ```text
//! GET    /api/plugins               — List installed plugins
//! POST   /api/plugins               — Install plugin (manifest)
//! GET    /api/plugins/:id           — Get plugin details
//! PATCH  /api/plugins/:id           — Activate, deactivate, or update
//! DELETE /api/plugins/:id           — Uninstall plugin
//! POST   /api/plugins/:id/configure — Set plugin configuration
//! GET    /api/plugins/marketplace   — List available plugins (stub)
//! ```

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::plugin::{PluginManifest, PluginState};
use super::registry::PluginRegistry;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Shared state for plugin API handlers.
#[derive(Clone)]
pub struct PluginApiState {
    pub registry: Arc<PluginRegistry>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Summary returned in list endpoints.
#[derive(Serialize)]
pub struct PluginSummary {
    pub id: Uuid,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub state: PluginState,
    pub capabilities: Vec<String>,
}

impl PluginSummary {
    fn from_manifest(manifest: &PluginManifest, state: PluginState) -> Self {
        Self {
            id: manifest.id,
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            author: manifest.author.clone(),
            description: manifest.description.clone(),
            state,
            capabilities: manifest
                .capabilities
                .iter()
                .map(|c| format!("{}:{}", c.kind(), c.name()))
                .collect(),
        }
    }
}

/// Detailed plugin response.
#[derive(Serialize)]
pub struct PluginDetail {
    pub manifest: PluginManifest,
    pub state: PluginState,
    pub config: Value,
}

/// Request body for PATCH (activate/deactivate).
#[derive(Deserialize)]
pub struct PluginPatchRequest {
    /// Set to `"active"` or `"disabled"` to change state.
    #[serde(default)]
    pub action: Option<String>,
}

/// Request body for configure endpoint.
#[derive(Deserialize)]
pub struct PluginConfigureRequest {
    pub config: Value,
}

/// Marketplace plugin listing (stub).
#[derive(Serialize)]
pub struct MarketplacePlugin {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub downloads: u64,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the plugin management router.
///
/// Mount under `/api/plugins` in the main application router.
pub fn plugin_routes(state: PluginApiState) -> Router {
    Router::new()
        .route("/", get(list_plugins).post(install_plugin))
        .route("/marketplace", get(marketplace))
        .route(
            "/{id}",
            get(get_plugin).patch(patch_plugin).delete(delete_plugin),
        )
        .route("/{id}/configure", post(configure_plugin))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/plugins` — List all installed plugins.
async fn list_plugins(State(state): State<PluginApiState>) -> impl IntoResponse {
    let manifests = state.registry.list();
    let summaries: Vec<PluginSummary> = manifests
        .iter()
        .map(|m| {
            let plugin_state = state
                .registry
                .get_state(m.id)
                .unwrap_or(PluginState::Installed);
            PluginSummary::from_manifest(m, plugin_state)
        })
        .collect();

    Json(serde_json::json!({
        "plugins": summaries,
        "count": summaries.len(),
    }))
}

/// `POST /api/plugins` — Install a plugin by manifest.
async fn install_plugin(
    State(state): State<PluginApiState>,
    Json(manifest): Json<PluginManifest>,
) -> impl IntoResponse {
    // Check for duplicate.
    if state.registry.get(manifest.id).is_some() {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": {
                    "code": "CONFLICT",
                    "message": format!("Plugin {} is already installed", manifest.id),
                }
            })),
        );
    }

    let id = manifest.id;
    let name = manifest.name.clone();
    state.registry.register(manifest);

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "id": id,
            "name": name,
            "state": "installed",
        })),
    )
}

/// `GET /api/plugins/:id` — Get plugin details.
async fn get_plugin(
    State(state): State<PluginApiState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let manifest = match state.registry.get(id) {
        Some(m) => m,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": {
                        "code": "NOT_FOUND",
                        "message": format!("Plugin {id} not found"),
                    }
                })),
            );
        }
    };

    let plugin_state = state
        .registry
        .get_state(id)
        .unwrap_or(PluginState::Installed);
    let config = state
        .registry
        .get_config(id)
        .unwrap_or(serde_json::json!({}));

    (
        StatusCode::OK,
        Json(serde_json::json!(PluginDetail {
            manifest,
            state: plugin_state,
            config,
        })),
    )
}

/// `PATCH /api/plugins/:id` — Activate or deactivate a plugin.
async fn patch_plugin(
    State(state): State<PluginApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PluginPatchRequest>,
) -> impl IntoResponse {
    if state.registry.get(id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "code": "NOT_FOUND",
                    "message": format!("Plugin {id} not found"),
                }
            })),
        );
    }

    match body.action.as_deref() {
        Some("activate") => match state.registry.activate(id).await {
            Ok(()) => (
                StatusCode::OK,
                Json(serde_json::json!({ "id": id, "state": "active" })),
            ),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": { "code": "INTERNAL", "message": e }
                })),
            ),
        },
        Some("deactivate") => match state.registry.deactivate(id).await {
            Ok(()) => (
                StatusCode::OK,
                Json(serde_json::json!({ "id": id, "state": "disabled" })),
            ),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": { "code": "INTERNAL", "message": e }
                })),
            ),
        },
        Some(other) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "code": "BAD_REQUEST",
                    "message": format!("Unknown action: {other}. Use 'activate' or 'deactivate'."),
                }
            })),
        ),
        None => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "code": "BAD_REQUEST",
                    "message": "Missing 'action' field. Use 'activate' or 'deactivate'.",
                }
            })),
        ),
    }
}

/// `DELETE /api/plugins/:id` — Uninstall a plugin.
async fn delete_plugin(
    State(state): State<PluginApiState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if state.registry.unregister(id).await {
        (
            StatusCode::OK,
            Json(serde_json::json!({ "id": id, "uninstalled": true })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "code": "NOT_FOUND",
                    "message": format!("Plugin {id} not found"),
                }
            })),
        )
    }
}

/// `POST /api/plugins/:id/configure` — Set plugin configuration.
async fn configure_plugin(
    State(state): State<PluginApiState>,
    Path(id): Path<Uuid>,
    Json(body): Json<PluginConfigureRequest>,
) -> impl IntoResponse {
    match state.registry.configure(id, body.config).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "id": id, "configured": true })),
        ),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": {
                    "code": "NOT_FOUND",
                    "message": e,
                }
            })),
        ),
    }
}

/// `GET /api/plugins/marketplace` — List available plugins (stub).
async fn marketplace() -> impl IntoResponse {
    let plugins: Vec<MarketplacePlugin> = vec![
        MarketplacePlugin {
            name: "slack-notifications".into(),
            version: "1.0.0".into(),
            author: "DarshJDB".into(),
            description: "Send Slack notifications on record changes".into(),
            downloads: 0,
        },
        MarketplacePlugin {
            name: "data-validation".into(),
            version: "1.0.0".into(),
            author: "DarshJDB".into(),
            description: "Custom validation rules beyond field-level constraints".into(),
            downloads: 0,
        },
        MarketplacePlugin {
            name: "audit-log".into(),
            version: "1.0.0".into(),
            author: "DarshJDB".into(),
            description: "Enhanced audit logging with user action tracking".into(),
            downloads: 0,
        },
    ];

    Json(serde_json::json!({
        "marketplace": plugins,
        "count": plugins.len(),
        "note": "Marketplace is a planned feature. These are built-in plugins.",
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::hooks::HookRegistry;
    use crate::plugins::plugin::PluginCapability;
    use axum::body::Body;
    use axum::http::Request;
    use http::header::CONTENT_TYPE;
    use tower::ServiceExt;

    fn make_state() -> PluginApiState {
        let hooks = Arc::new(HookRegistry::new());
        PluginApiState {
            registry: Arc::new(PluginRegistry::new(hooks)),
        }
    }

    fn make_manifest() -> PluginManifest {
        PluginManifest {
            id: Uuid::new_v4(),
            name: "test-plugin".into(),
            version: "1.0.0".into(),
            author: "test".into(),
            description: "Test".into(),
            homepage: None,
            capabilities: vec![PluginCapability::CustomField("rating".into())],
            config_schema: serde_json::json!({}),
            entry_point: String::new(),
        }
    }

    #[tokio::test]
    async fn list_empty() {
        let state = make_state();
        let app = plugin_routes(state);

        let resp = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 0);
    }

    #[tokio::test]
    async fn install_and_get() {
        let state = make_state();
        let manifest = make_manifest();
        let id = manifest.id;
        let app = plugin_routes(state.clone());

        // Install.
        let resp = app
            .clone()
            .oneshot(
                Request::post("/")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&manifest).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Get.
        let app = plugin_routes(state);
        let resp = app
            .oneshot(Request::get(format!("/{id}")).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn install_duplicate_conflicts() {
        let state = make_state();
        let manifest = make_manifest();

        // First install.
        state.registry.register(manifest.clone());

        let app = plugin_routes(state);
        let resp = app
            .oneshot(
                Request::post("/")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&manifest).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn get_not_found() {
        let state = make_state();
        let app = plugin_routes(state);
        let fake_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::get(format!("/{fake_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_plugin_handler() {
        let state = make_state();
        let manifest = make_manifest();
        let id = manifest.id;
        state.registry.register(manifest);

        let app = plugin_routes(state);
        let resp = app
            .oneshot(
                Request::delete(format!("/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn marketplace_stub() {
        let state = make_state();
        let app = plugin_routes(state);

        let resp = app
            .oneshot(Request::get("/marketplace").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["count"], 3);
    }
}
