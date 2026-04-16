//! OAuth, magic-link, and token verification handlers.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::auth::{
    AuthOutcome, MagicLinkProvider, OAuth2Provider, OAuthProviderKind, OAuthUserInfo,
};

use super::helpers::negotiate_response;

// ---------------------------------------------------------------------------
// Magic link
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct MagicLinkRequest {
    email: String,
}

/// `POST /api/auth/magic-link` -- Send a passwordless sign-in link.
pub async fn auth_magic_link(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<MagicLinkRequest>,
) -> Result<Response, ApiError> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::bad_request("Invalid email address"));
    }

    // Look up the user. If not found, still return 200 to prevent enumeration.
    let user_row: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE email = $1 AND deleted_at IS NULL")
            .bind(&email)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| ApiError::internal(format!("Database error: {e}")))?;

    if let Some((user_id,)) = user_row {
        let magic_link = MagicLinkProvider::generate(&state.pool, user_id)
            .await
            .map_err(|e| ApiError::internal(format!("Failed to generate magic link: {e}")))?;

        tracing::debug!(user_id = %user_id, expires_at = %magic_link.expires_at, "magic link generated");

        // In dev mode, include the token in the response for testing.
        if state.dev_mode {
            return Ok((
                StatusCode::OK,
                axum::Json(serde_json::json!({
                    "message": "If an account exists, a magic link has been sent.",
                    "_dev_token": magic_link.token,
                    "_dev_expires_at": magic_link.expires_at.to_rfc3339(),
                })),
            )
                .into_response());
        }
    }

    // Always return 200 to prevent email enumeration.
    Ok((
        StatusCode::OK,
        axum::Json(serde_json::json!({
            "message": "If an account exists, a magic link has been sent."
        })),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Verify (magic link / MFA)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct VerifyRequest {
    token: String,
    #[serde(rename = "mfa_code")]
    #[allow(dead_code)]
    _mfa_code: Option<String>,
}

/// `POST /api/auth/verify` -- Verify a magic-link token or MFA code.
pub async fn auth_verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<VerifyRequest>,
) -> Result<Response, ApiError> {
    if body.token.is_empty() {
        return Err(ApiError::bad_request("Token is required"));
    }

    let outcome = MagicLinkProvider::verify(&state.pool, &body.token)
        .await
        .map_err(|e| ApiError::internal(format!("Token verification failed: {e}")))?;

    match outcome {
        AuthOutcome::Success { user_id, roles } => {
            let ip = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            let ua = headers
                .get("user-agent")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown");
            let dfp = headers
                .get("x-device-fingerprint")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");

            let token_pair = state
                .session_manager
                .create_session(user_id, roles, ip, ua, dfp)
                .await
                .map_err(|e| ApiError::internal(format!("Session creation failed: {e}")))?;

            let response = serde_json::json!({
                "user_id": user_id,
                "access_token": token_pair.access_token,
                "refresh_token": token_pair.refresh_token,
                "expires_in": token_pair.expires_in,
                "token_type": token_pair.token_type,
            });
            Ok(negotiate_response(&headers, &response))
        }
        AuthOutcome::MfaRequired { user_id, mfa_token } => {
            let response = serde_json::json!({
                "mfa_required": true,
                "user_id": user_id,
                "mfa_token": mfa_token,
            });
            Ok(negotiate_response(&headers, &response))
        }
        AuthOutcome::Failed { reason } => Err(ApiError::unauthenticated(format!(
            "Verification failed: {reason}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// OAuth
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct OAuthRequest {
    code: Option<String>,
    state: Option<String>,
    pkce_verifier: Option<String>,
}

/// `POST /api/auth/oauth/:provider` -- Generate an OAuth2 authorize URL or exchange code.
pub async fn auth_oauth(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<OAuthRequest>,
) -> Result<Response, ApiError> {
    let kind = OAuthProviderKind::from_name(&provider)
        .ok_or_else(|| ApiError::bad_request(format!("Unsupported OAuth provider: {provider}")))?;

    let oauth_provider = state.oauth_providers.get(&kind).ok_or_else(|| {
        ApiError::bad_request(format!(
            "OAuth provider '{}' is not configured on this server",
            provider
        ))
    })?;

    // If a code is provided, do inline exchange (SPA / backward-compat flow).
    if let Some(code) = body.code.as_deref().filter(|c| !c.is_empty()) {
        let oauth_state = body
            .state
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("state parameter required for code exchange"))?;
        let verifier = body
            .pkce_verifier
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("pkce_verifier required for code exchange"))?;

        let user_info = oauth_provider
            .exchange_code(code, oauth_state, verifier, &state.oauth_state_secret)
            .await
            .map_err(|e| ApiError::bad_request(format!("OAuth exchange failed: {e}")))?;

        let (user_id, roles) = find_or_create_oauth_user(&state.pool, &user_info).await?;

        let ip = headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        let ua = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");
        let dfp = headers
            .get("x-device-fingerprint")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let token_pair = state
            .session_manager
            .create_session(user_id, roles, ip, ua, dfp)
            .await
            .map_err(|e| ApiError::internal(format!("Session creation failed: {e}")))?;

        let response = serde_json::json!({
            "user_id": user_id,
            "access_token": token_pair.access_token,
            "refresh_token": token_pair.refresh_token,
            "expires_in": token_pair.expires_in,
            "token_type": token_pair.token_type,
        });

        return Ok(negotiate_response(&headers, &response));
    }

    // No code supplied -- generate the authorize URL.
    let (url, csrf_state, pkce_verifier) = oauth_provider
        .authorization_url(&state.oauth_state_secret)
        .map_err(|e| ApiError::internal(format!("Failed to build authorize URL: {e}")))?;

    let response = serde_json::json!({
        "authorize_url": url,
        "state": csrf_state,
        "pkce_verifier": pkce_verifier,
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// OAuth callback
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct OAuthCallbackQuery {
    code: String,
    state: String,
}

/// `GET /api/auth/oauth/:provider/callback` -- OAuth2 redirect callback.
pub async fn auth_oauth_callback(
    State(app): State<AppState>,
    Path(provider): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let kind = OAuthProviderKind::from_name(&provider)
        .ok_or_else(|| ApiError::bad_request(format!("Unsupported OAuth provider: {provider}")))?;

    let oauth_provider = app.oauth_providers.get(&kind).ok_or_else(|| {
        ApiError::bad_request(format!(
            "OAuth provider '{}' is not configured on this server",
            provider
        ))
    })?;

    let pkce_verifier = headers
        .get("x-pkce-verifier")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let user_info = oauth_provider
        .exchange_code(
            &params.code,
            &params.state,
            pkce_verifier,
            &app.oauth_state_secret,
        )
        .await
        .map_err(|e| ApiError::bad_request(format!("OAuth callback failed: {e}")))?;

    let (user_id, roles) = find_or_create_oauth_user(&app.pool, &user_info).await?;

    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token_pair = app
        .session_manager
        .create_session(user_id, roles, ip, ua, dfp)
        .await
        .map_err(|e| ApiError::internal(format!("Session creation failed: {e}")))?;

    let response = serde_json::json!({
        "user_id": user_id,
        "access_token": token_pair.access_token,
        "refresh_token": token_pair.refresh_token,
        "expires_in": token_pair.expires_in,
        "token_type": token_pair.token_type,
    });

    Ok(negotiate_response(&headers, &response))
}

/// Find or create a user from an OAuth identity.
async fn find_or_create_oauth_user(
    pool: &PgPool,
    info: &OAuthUserInfo,
) -> Result<(Uuid, Vec<String>), ApiError> {
    let provider_str = info.provider.to_string();
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM oauth_identities WHERE provider = $1 AND provider_user_id = $2",
    )
    .bind(&provider_str)
    .bind(&info.provider_user_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| ApiError::internal(format!("OAuth lookup failed: {e}")))?;
    if let Some((user_id,)) = existing {
        let roles: Vec<String> =
            sqlx::query_scalar("SELECT roles FROM users WHERE id = $1 AND deleted_at IS NULL")
                .bind(user_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .and_then(|v: serde_json::Value| serde_json::from_value(v).ok())
                .unwrap_or_else(|| vec!["user".to_string()]);
        return Ok((user_id, roles));
    }
    let user_id = Uuid::new_v4();
    let email = info
        .email
        .as_deref()
        .map(|e| e.trim().to_lowercase())
        .unwrap_or_else(|| format!("{}@oauth.{}", info.provider_user_id, provider_str));
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(user_id)
    .bind(&email)
    .bind("!oauth-only")
    .bind(serde_json::json!(["user"]))
    .execute(pool)
    .await
    .map_err(|e| ApiError::internal(format!("User creation failed: {e}")))?;
    sqlx::query(
        "INSERT INTO oauth_identities (user_id, provider, provider_user_id, email, name, avatar_url) VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(user_id)
    .bind(&provider_str)
    .bind(&info.provider_user_id)
    .bind(&info.email)
    .bind(&info.name)
    .bind(&info.avatar_url)
    .execute(pool)
    .await
    .map_err(|e| ApiError::internal(format!("OAuth link failed: {e}")))?;
    Ok((user_id, vec!["user".to_string()]))
}
