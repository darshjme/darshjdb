//! Authentication and authorization engine for DarshJDB.
//!
//! Provides a complete security stack: credential verification, session
//! management, multi-factor authentication, fine-grained permissions,
//! and Axum middleware for HTTP request pipelines.
//!
//! # Architecture
//!
//! ```text
//! Request ──▶ Middleware ──▶ JWT Validation ──▶ AuthContext
//!                │                                  │
//!                ▼                                  ▼
//!           Rate Limiter                    Permission Engine
//!                                                  │
//!                                          ┌───────┴───────┐
//!                                          ▼               ▼
//!                                     Read Path       Write Path
//!                                   (WHERE inject)  (pre-tx check)
//! ```
//!
//! - **Providers**: Pluggable authentication backends (password, magic link, OAuth2).
//! - **Session**: JWT issuance, refresh rotation, key management.
//! - **MFA**: TOTP, recovery codes, and WebAuthn stubs.
//! - **Permissions**: Rule-based access control with query-level injection.
//! - **Middleware**: Axum layers for token extraction, rate limiting, and context building.

pub mod default_permissions;
pub mod mfa;
pub mod middleware;
pub mod permissions;
pub mod providers;
pub mod session;

pub use default_permissions::{build_default_engine, get_rule_with_fallback};
pub use mfa::{RecoveryCodeManager, TotpManager, WebAuthnStub};
pub use middleware::{AuthLayer, RateLimiter, auth_middleware};
pub use permissions::{
    Operation, PermissionEngine, PermissionResult, PermissionRule, evaluate_permission,
    evaluate_rule_public,
};
pub use providers::{
    GenericOAuth2Provider, MagicLinkProvider, OAuth2Provider, OAuthConfig, OAuthProviderKind,
    OAuthUserInfo, PasswordProvider,
};
pub use session::{KeyManager, SessionManager, SessionRecord, TokenPair};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identity context attached to every authenticated request.
///
/// Built by the auth middleware after JWT validation and carried
/// through the request lifecycle for permission evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    /// Unique user identifier.
    pub user_id: Uuid,
    /// Session identifier for this particular login.
    pub session_id: Uuid,
    /// Roles assigned to the user (e.g., `["admin", "editor"]`).
    pub roles: Vec<String>,
    /// IP address of the originating request.
    pub ip: String,
    /// User-Agent header value.
    pub user_agent: String,
    /// Device fingerprint bound to the refresh token.
    pub device_fingerprint: String,
}

/// Outcome of an authentication attempt.
#[derive(Debug)]
pub enum AuthOutcome {
    /// Credentials valid, proceed to issue tokens.
    Success { user_id: Uuid, roles: Vec<String> },
    /// Credentials valid but MFA is required before token issuance.
    MfaRequired { user_id: Uuid, mfa_token: String },
    /// Authentication failed with a reason safe for logging.
    Failed { reason: String },
}

/// Errors specific to the auth subsystem.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The provided credentials are invalid.
    #[error("invalid credentials")]
    InvalidCredentials,

    /// The token has expired or is malformed.
    #[error("token expired or invalid: {0}")]
    TokenInvalid(String),

    /// The refresh token does not match the expected device fingerprint.
    #[error("device fingerprint mismatch")]
    DeviceMismatch,

    /// The session has been revoked.
    #[error("session revoked")]
    SessionRevoked,

    /// MFA verification failed.
    #[error("MFA verification failed: {0}")]
    MfaFailed(String),

    /// The user does not have permission for the requested operation.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Rate limit exceeded.
    #[error("rate limit exceeded, retry after {retry_after_secs}s")]
    RateLimited {
        /// Seconds until the next request will be accepted.
        retry_after_secs: u64,
    },

    /// An OAuth2 flow error.
    #[error("OAuth2 error: {0}")]
    OAuth2(String),

    /// A cryptographic or encoding error.
    #[error("crypto error: {0}")]
    Crypto(String),

    /// Database interaction failed.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// An unexpected internal error.
    #[error("internal auth error: {0}")]
    Internal(String),
}

impl AuthError {
    /// Map this error to an HTTP status code.
    pub fn status_code(&self) -> http::StatusCode {
        match self {
            Self::InvalidCredentials | Self::MfaFailed(_) => http::StatusCode::UNAUTHORIZED,
            Self::TokenInvalid(_) | Self::DeviceMismatch | Self::SessionRevoked => {
                http::StatusCode::UNAUTHORIZED
            }
            Self::PermissionDenied(_) => http::StatusCode::FORBIDDEN,
            Self::RateLimited { .. } => http::StatusCode::TOO_MANY_REQUESTS,
            Self::OAuth2(_) | Self::Crypto(_) => http::StatusCode::BAD_REQUEST,
            Self::Database(_) | Self::Internal(_) => http::StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
