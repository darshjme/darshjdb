//! Server-side function invocation handler.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::rest::AppState;

use super::helpers::{extract_bearer_token, negotiate_response};

/// `POST /api/fn/:name` -- Invoke a registered server-side function.
///
/// Looks up the function by name in the [`FunctionRegistry`], validates
/// arguments, executes via the [`FunctionRuntime`], and returns the result.
pub async fn fn_invoke(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
    axum::Json(args): axum::Json<Value>,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(&headers).ok();

    if name.is_empty() {
        return Err(ApiError::bad_request("Function name is required"));
    }

    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || c == ':' || c == '/')
    {
        return Err(ApiError::bad_request(
            "Function name contains invalid characters",
        ));
    }

    let registry = state
        .function_registry
        .as_ref()
        .ok_or_else(|| ApiError::internal("Function registry not initialized"))?;
    let runtime = state
        .function_runtime
        .as_ref()
        .ok_or_else(|| ApiError::internal("Function runtime not initialized"))?;

    let function_def = match registry.get(&name).await {
        Some(def) => def,
        None => {
            let all = registry.list().await;
            all.into_iter()
                .find(|f| f.export_name == name || f.name.ends_with(&format!(":{name}")))
                .ok_or_else(|| ApiError::not_found(format!("Function `{name}` not found")))?
        }
    };

    let result = runtime
        .execute(&function_def, args, token)
        .await
        .map_err(|e| ApiError::internal(format!("Function execution failed: {e}")))?;

    let response = serde_json::json!({
        "result": result.value,
        "duration_ms": result.duration_ms,
        "logs": result.logs,
    });

    Ok(negotiate_response(&headers, &response))
}
