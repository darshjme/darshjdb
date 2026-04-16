//! Shared helper functions used across multiple handler modules.

use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use uuid::Uuid;

use axum::http::header::{ACCEPT, CONTENT_TYPE};

use crate::api::error::{ApiError, ErrorCode};
use crate::api::rest::AppState;
use crate::auth::{
    AuthContext, Operation, PermissionEngine, evaluate_rule_public, get_rule_with_fallback,
};
use crate::storage::StorageError;
use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Content negotiation helpers
// ---------------------------------------------------------------------------

/// Returns `true` when the client prefers MessagePack over JSON.
pub fn wants_msgpack(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("application/msgpack"))
        .unwrap_or(false)
}

/// Serialize `value` as JSON or MessagePack depending on the `Accept` header.
pub fn negotiate_response(headers: &HeaderMap, value: &impl Serialize) -> Response {
    if wants_msgpack(headers) {
        match rmp_serde::to_vec(value) {
            Ok(bytes) => {
                let mut resp = (StatusCode::OK, bytes).into_response();
                resp.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/msgpack"),
                );
                resp
            }
            Err(e) => ApiError::internal(format!("msgpack encode: {e}")).into_response(),
        }
    } else {
        axum::Json(value).into_response()
    }
}

/// Serialize `value` with a specific status code, respecting content negotiation.
pub fn negotiate_response_status(
    headers: &HeaderMap,
    status: StatusCode,
    value: &impl Serialize,
) -> Response {
    if wants_msgpack(headers) {
        match rmp_serde::to_vec(value) {
            Ok(bytes) => {
                let mut resp = (status, bytes).into_response();
                resp.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/msgpack"),
                );
                resp
            }
            Err(e) => ApiError::internal(format!("msgpack encode: {e}")).into_response(),
        }
    } else {
        (status, axum::Json(value)).into_response()
    }
}

// ---------------------------------------------------------------------------
// Auth helpers
// ---------------------------------------------------------------------------

/// Extract a Bearer token from the `Authorization` header.
///
/// Returns `Err(ApiError)` with a 401 status if the header is missing
/// or malformed.
pub fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let header = headers
        .get(http::header::AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthenticated("Missing Authorization header"))?;

    let value = header
        .to_str()
        .map_err(|_| ApiError::unauthenticated("Invalid Authorization header encoding"))?;

    if !value.starts_with("Bearer ") {
        return Err(ApiError::unauthenticated(
            "Authorization header must use Bearer scheme",
        ));
    }

    let token = value[7..].trim().to_string();
    if token.is_empty() {
        return Err(ApiError::unauthenticated("Bearer token is empty"));
    }

    Ok(token)
}

/// Verify the authenticated user holds the "admin" role by decoding JWT claims.
pub fn require_admin_role(headers: &HeaderMap) -> Result<(), ApiError> {
    let ctx = decode_jwt_claims(headers)?;
    if ctx.roles.iter().any(|r| r == "admin") {
        Ok(())
    } else {
        Err(ApiError::permission_denied(
            "Admin role required for this endpoint",
        ))
    }
}

/// Extract an [`AuthContext`] by validating the JWT via the [`SessionManager`].
pub fn extract_auth_context(headers: &HeaderMap, state: &AppState) -> Result<AuthContext, ApiError> {
    let token = extract_bearer_token(headers)?;
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let ua = headers
        .get(http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    state
        .session_manager
        .validate_token(&token, ip, ua, dfp)
        .map_err(|e| ApiError::unauthenticated(format!("Invalid token: {e}")))
}

/// Decode JWT claims from the Bearer token without full signature verification.
pub fn decode_jwt_claims(headers: &HeaderMap) -> Result<AuthContext, ApiError> {
    let token = extract_bearer_token(headers)?;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(ApiError::unauthenticated("Malformed JWT"));
    }
    let payload_bytes = data_encoding::BASE64URL_NOPAD
        .decode(parts[1].as_bytes())
        .map_err(|_| ApiError::unauthenticated("Invalid JWT encoding"))?;
    #[derive(Deserialize)]
    struct Claims {
        sub: String,
        sid: String,
        #[serde(default)]
        roles: Vec<String>,
    }
    let claims: Claims = serde_json::from_slice(&payload_bytes)
        .map_err(|_| ApiError::unauthenticated("Invalid JWT claims"))?;
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::unauthenticated("Invalid user_id in JWT"))?;
    let session_id = Uuid::parse_str(&claims.sid)
        .map_err(|_| ApiError::unauthenticated("Invalid session_id in JWT"))?;
    Ok(AuthContext {
        user_id,
        session_id,
        roles: claims.roles,
        ip: headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(),
        user_agent: headers
            .get(http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string(),
        device_fingerprint: headers
            .get("x-device-fingerprint")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string(),
    })
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// Evaluate permission for an entity operation and return the result.
///
/// Uses the permission engine from `AppState`, falling back to wildcard
/// rules if no entity-specific rule is configured.
///
/// Returns `Err(ApiError)` with 403 if the operation is denied.
pub fn check_permission(
    auth_ctx: &AuthContext,
    entity_type: &str,
    operation: Operation,
    engine: &PermissionEngine,
) -> Result<crate::auth::PermissionResult, ApiError> {
    let rule = match get_rule_with_fallback(engine, entity_type, operation) {
        Some(r) => r,
        None => {
            return Err(ApiError::permission_denied(format!(
                "No permission rule configured for {entity_type}.{operation:?}"
            )));
        }
    };

    let result = evaluate_rule_public(auth_ctx, rule);

    if !result.allowed {
        let reason = result
            .denial_reason
            .as_deref()
            .unwrap_or("permission denied");
        return Err(ApiError::permission_denied(format!(
            "Access denied for {entity_type}.{operation:?}: {reason}"
        )));
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Data helpers
// ---------------------------------------------------------------------------

/// Infer the triple store value_type discriminator from a JSON value.
pub fn infer_value_type(value: &Value) -> i16 {
    match value {
        Value::String(s) => {
            // Check if it looks like a UUID (reference).
            if s.len() == 36 && Uuid::parse_str(s).is_ok() {
                5 // Reference
            } else {
                0 // String
            }
        }
        Value::Number(n) => {
            if n.is_f64() && !n.is_i64() && !n.is_u64() {
                2 // Float
            } else {
                1 // Integer
            }
        }
        Value::Bool(_) => 3,                     // Boolean
        Value::Object(_) | Value::Array(_) => 6, // Json
        Value::Null => 0,                        // Default to String for null
    }
}

/// Validate that an entity name is safe and well-formed.
pub fn validate_entity_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty() {
        return Err(ApiError::bad_request("Entity name is required"));
    }
    if name.len() > 128 {
        return Err(ApiError::bad_request(
            "Entity name is too long (max 128 chars)",
        ));
    }
    // Must start with a letter or underscore (not a digit or hyphen).
    if let Some(first) = name.chars().next()
        && !first.is_ascii_alphabetic()
        && first != '_'
    {
        return Err(ApiError::bad_request(
            "Entity name must start with a letter or underscore",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ApiError::bad_request(
            "Entity name may only contain alphanumeric characters, underscores, and hyphens",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Storage helpers
// ---------------------------------------------------------------------------

/// Convert a [`StorageError`] into an [`ApiError`].
pub fn storage_err_to_api(err: StorageError) -> ApiError {
    match &err {
        StorageError::NotFound(_) => ApiError::not_found(err.to_string()),
        StorageError::InvalidPath(_) => ApiError::bad_request(err.to_string()),
        StorageError::Rejected(_) => ApiError::new(ErrorCode::PayloadTooLarge, err.to_string()),
        StorageError::SignatureExpired | StorageError::InvalidSignature => {
            ApiError::new(ErrorCode::Unauthenticated, err.to_string())
        }
        _ => ApiError::internal(err.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Embedding helpers
// ---------------------------------------------------------------------------

/// Format a slice of f32 values as a pgvector literal string: `[0.1,0.2,0.3]`.
#[allow(dead_code)]
pub fn format_pgvector_literal(vec: &[f32]) -> String {
    let mut s = String::with_capacity(vec.len() * 8 + 2);
    s.push('[');
    for (i, v) in vec.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&v.to_string());
    }
    s.push(']');
    s
}
