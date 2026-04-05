//! Consistent API error formatting for DarshanDB.
//!
//! All error responses follow a uniform JSON envelope:
//!
//! ```json
//! {
//!   "error": {
//!     "code": "PERMISSION_DENIED",
//!     "message": "You do not have access to this resource.",
//!     "status": 403
//!   }
//! }
//! ```
//!
//! [`ApiError`] implements [`axum::response::IntoResponse`] so handlers
//! can return `Result<T, ApiError>` directly.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::auth::AuthError;
use crate::error::DarshanError;

/// Structured error code included in every API error response.
///
/// Codes are stable across versions and safe for client-side matching.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// The request could not be parsed or is semantically invalid.
    BadRequest,
    /// Authentication credentials are missing or invalid.
    Unauthenticated,
    /// The caller lacks permission for the requested operation.
    PermissionDenied,
    /// The requested resource does not exist.
    NotFound,
    /// The request conflicts with current resource state.
    Conflict,
    /// Request payload exceeds allowed size.
    PayloadTooLarge,
    /// Too many requests; the caller has been rate-limited.
    RateLimited,
    /// A query or DarshanQL expression is malformed.
    InvalidQuery,
    /// A value could not be coerced to the expected type.
    TypeMismatch,
    /// Schema migration would cause a conflict.
    SchemaConflict,
    /// An internal server error occurred.
    Internal,
}

impl ErrorCode {
    /// Map this code to its canonical HTTP status.
    fn status(self) -> StatusCode {
        match self {
            Self::BadRequest | Self::InvalidQuery | Self::TypeMismatch => StatusCode::BAD_REQUEST,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::PermissionDenied => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict | Self::SchemaConflict => StatusCode::CONFLICT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Top-level API error type.
///
/// Carries a machine-readable [`ErrorCode`], a human-readable message,
/// and an optional `retry_after_secs` hint for rate-limit errors.
#[derive(Debug)]
pub struct ApiError {
    /// Machine-readable error code.
    pub code: ErrorCode,
    /// Human-readable explanation.
    pub message: String,
    /// Seconds until the client should retry (rate-limit only).
    pub retry_after_secs: Option<u64>,
}

impl ApiError {
    /// Convenience constructor for a simple code + message error.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    /// Convenience for `400 Bad Request`.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    /// Convenience for `401 Unauthenticated`.
    pub fn unauthenticated(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unauthenticated, message)
    }

    /// Convenience for `403 Permission Denied`.
    pub fn permission_denied(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PermissionDenied, message)
    }

    /// Convenience for `404 Not Found`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    /// Convenience for `429 Rate Limited` with retry hint.
    pub fn rate_limited(retry_after_secs: u64) -> Self {
        Self {
            code: ErrorCode::RateLimited,
            message: format!("Rate limit exceeded. Retry after {retry_after_secs}s."),
            retry_after_secs: Some(retry_after_secs),
        }
    }

    /// Convenience for `500 Internal`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }
}

/// JSON envelope for error responses.
#[derive(Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

/// Inner body of the error envelope.
#[derive(Serialize)]
struct ErrorBody {
    code: ErrorCode,
    message: String,
    status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.code.status();
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: self.message,
                status: status.as_u16(),
                retry_after_secs: self.retry_after_secs,
            },
        };

        let mut response = (status, axum::Json(body)).into_response();

        // Add Retry-After header for rate-limit responses.
        if let Some(secs) = self.retry_after_secs {
            if let Ok(val) = http::HeaderValue::from_str(&secs.to_string()) {
                response.headers_mut().insert("Retry-After", val);
            }
        }

        response
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

// ---------------------------------------------------------------------------
// Conversions from domain errors
// ---------------------------------------------------------------------------

impl From<AuthError> for ApiError {
    fn from(err: AuthError) -> Self {
        let code = match &err {
            AuthError::InvalidCredentials | AuthError::MfaFailed(_) => ErrorCode::Unauthenticated,
            AuthError::TokenInvalid(_) | AuthError::DeviceMismatch | AuthError::SessionRevoked => {
                ErrorCode::Unauthenticated
            }
            AuthError::PermissionDenied(_) => ErrorCode::PermissionDenied,
            AuthError::RateLimited { retry_after_secs } => {
                return ApiError {
                    code: ErrorCode::RateLimited,
                    message: err.to_string(),
                    retry_after_secs: Some(*retry_after_secs),
                };
            }
            AuthError::OAuth2(_) | AuthError::Crypto(_) => ErrorCode::BadRequest,
            AuthError::Database(_) | AuthError::Internal(_) => ErrorCode::Internal,
        };

        Self::new(code, err.to_string())
    }
}

impl From<DarshanError> for ApiError {
    fn from(err: DarshanError) -> Self {
        let code = match &err {
            DarshanError::Database(_) | DarshanError::Internal(_) => ErrorCode::Internal,
            DarshanError::InvalidQuery(_) => ErrorCode::InvalidQuery,
            DarshanError::EntityNotFound(_) => ErrorCode::NotFound,
            DarshanError::InvalidAttribute(_) => ErrorCode::BadRequest,
            DarshanError::TypeMismatch { .. } => ErrorCode::TypeMismatch,
            DarshanError::SchemaConflict(_) => ErrorCode::SchemaConflict,
            DarshanError::Serialization(_) => ErrorCode::BadRequest,
        };

        Self::new(code, err.to_string())
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        Self::bad_request(format!("JSON parse error: {err}"))
    }
}
