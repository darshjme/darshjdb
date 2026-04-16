//! OpenAPI spec and documentation viewer handlers.

use axum::extract::State;
use axum::response::{Html, IntoResponse};

use crate::api::openapi;
use crate::api::rest::AppState;

/// `GET /api/openapi.json` -- Serve the OpenAPI 3.1 specification.
pub async fn openapi_json(State(state): State<AppState>) -> impl IntoResponse {
    axum::Json(state.openapi_spec.as_ref().clone())
}

/// `GET /api/docs` -- Interactive Scalar API documentation viewer.
pub async fn docs(State(_state): State<AppState>) -> impl IntoResponse {
    Html(openapi::docs_html("/api/openapi.json"))
}
