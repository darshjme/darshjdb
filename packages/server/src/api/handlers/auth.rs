//! Core authentication handlers: signup, signin, refresh, signout, me.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::api::error::ApiError;
use crate::api::rest::AppState;
use crate::auth::{AuthOutcome, PasswordProvider};
use crate::triple_store::{TripleInput, TripleStore};

use super::helpers::{
    extract_bearer_token, negotiate_response, negotiate_response_status,
};

// ---------------------------------------------------------------------------
// Schema bootstrap
// ---------------------------------------------------------------------------

/// Ensure the `users` and `sessions` tables exist for the auth subsystem.
pub async fn ensure_auth_schema(pool: &PgPool) -> std::result::Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id              UUID PRIMARY KEY,
            email           TEXT NOT NULL UNIQUE,
            password_hash   TEXT NOT NULL,
            roles           JSONB NOT NULL DEFAULT '["user"]'::jsonb,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            deleted_at      TIMESTAMPTZ
        );
        CREATE INDEX IF NOT EXISTS idx_users_email ON users (email) WHERE deleted_at IS NULL;
        CREATE TABLE IF NOT EXISTS sessions (
            session_id          UUID PRIMARY KEY,
            user_id             UUID NOT NULL REFERENCES users(id),
            device_fingerprint  TEXT NOT NULL DEFAULT '',
            ip                  TEXT NOT NULL DEFAULT '',
            user_agent          TEXT NOT NULL DEFAULT '',
            created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
            revoked             BOOLEAN NOT NULL DEFAULT false,
            refresh_token_hash  TEXT NOT NULL,
            refresh_expires_at  TIMESTAMPTZ NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions (user_id) WHERE NOT revoked;
        CREATE INDEX IF NOT EXISTS idx_sessions_refresh ON sessions (refresh_token_hash) WHERE NOT revoked;
        CREATE TABLE IF NOT EXISTS oauth_identities (
            id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            user_id             UUID NOT NULL REFERENCES users(id),
            provider            TEXT NOT NULL,
            provider_user_id    TEXT NOT NULL,
            email               TEXT,
            name                TEXT,
            avatar_url          TEXT,
            created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
            UNIQUE (provider, provider_user_id)
        );
        CREATE INDEX IF NOT EXISTS idx_oauth_provider_user
            ON oauth_identities (provider, provider_user_id);
        CREATE INDEX IF NOT EXISTS idx_oauth_user_id ON oauth_identities (user_id);
        CREATE TABLE IF NOT EXISTS magic_links (
            id              BIGSERIAL PRIMARY KEY,
            token_hash      TEXT NOT NULL UNIQUE,
            user_id         UUID NOT NULL REFERENCES users(id),
            expires_at      TIMESTAMPTZ NOT NULL,
            consumed        BOOLEAN NOT NULL DEFAULT false,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
        );
        CREATE INDEX IF NOT EXISTS idx_magic_links_hash
            ON magic_links (token_hash) WHERE NOT consumed;
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Signup
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SignupRequest {
    email: String,
    password: String,
    #[serde(default)]
    name: Option<String>,
}

/// `POST /api/auth/signup` -- Create a new account with email and password.
pub async fn auth_signup(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<SignupRequest>,
) -> Result<Response, ApiError> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::bad_request("Invalid email address"));
    }
    if body.password.len() < 8 {
        return Err(ApiError::bad_request(
            "Password must be at least 8 characters",
        ));
    }
    if body.password.len() > 128 {
        return Err(ApiError::bad_request(
            "Password must be at most 128 characters",
        ));
    }

    let password_hash = PasswordProvider::hash_password(&body.password)
        .map_err(|e| ApiError::internal(format!("Password hashing failed: {e}")))?;

    let user_id = Uuid::new_v4();
    let roles = serde_json::json!(["user"]);

    let insert_result =
        sqlx::query("INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4)")
            .bind(user_id)
            .bind(&email)
            .bind(&password_hash)
            .bind(&roles)
            .execute(&state.pool)
            .await;

    match insert_result {
        Ok(_) => {}
        Err(e) => {
            let err_str = e.to_string();
            if err_str.contains("duplicate key") || err_str.contains("unique constraint") {
                return Err(ApiError::bad_request(
                    "An account with this email already exists",
                ));
            }
            return Err(ApiError::internal(format!("Failed to create user: {e}")));
        }
    }

    let user_triples = vec![
        TripleInput {
            entity_id: user_id,
            attribute: ":db/type".into(),
            value: Value::String("user".into()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/email".into(),
            value: Value::String(email.clone()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/name".into(),
            value: Value::String(body.name.unwrap_or_default()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: user_id,
            attribute: "user/created_at".into(),
            value: Value::String(chrono::Utc::now().to_rfc3339()),
            value_type: 0,
            ttl_seconds: None,
        },
    ];
    let _ = state.triple_store.set_triples(&user_triples).await;

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
        .create_session(user_id, vec!["user".into()], ip, ua, dfp)
        .await
        .map_err(|e| ApiError::internal(format!("Session creation failed: {e}")))?;

    let response = serde_json::json!({
        "user_id": user_id,
        "email": email,
        "access_token": token_pair.access_token,
        "refresh_token": token_pair.refresh_token,
        "expires_in": token_pair.expires_in,
        "token_type": token_pair.token_type,
    });

    Ok(negotiate_response_status(
        &headers,
        StatusCode::CREATED,
        &response,
    ))
}

// ---------------------------------------------------------------------------
// Signin
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SigninRequest {
    email: String,
    password: String,
}

/// `POST /api/auth/signin` -- Authenticate with email and password.
pub async fn auth_signin(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<SigninRequest>,
) -> Result<Response, ApiError> {
    let email = body.email.trim().to_lowercase();
    if email.is_empty() {
        return Err(ApiError::bad_request("Email is required"));
    }
    if body.password.is_empty() {
        return Err(ApiError::bad_request("Password is required"));
    }

    let outcome = PasswordProvider::authenticate(&state.pool, &email, &body.password)
        .await
        .map_err(|e| ApiError::internal(format!("Authentication error: {e}")))?;

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
        AuthOutcome::Failed { reason: _ } => {
            Err(ApiError::unauthenticated("Invalid email or password"))
        }
    }
}

// ---------------------------------------------------------------------------
// Refresh
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RefreshRequest {
    refresh_token: String,
}

/// `POST /api/auth/refresh` -- Rotate a refresh token for a new token pair.
pub async fn auth_refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<RefreshRequest>,
) -> Result<Response, ApiError> {
    if body.refresh_token.is_empty() {
        return Err(ApiError::bad_request("Refresh token is required"));
    }

    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token_pair = state
        .session_manager
        .refresh_session(&body.refresh_token, dfp)
        .await
        .map_err(|e| match &e {
            crate::auth::AuthError::SessionRevoked => {
                ApiError::unauthenticated("Session has been revoked")
            }
            crate::auth::AuthError::DeviceMismatch => ApiError::unauthenticated(
                "Device fingerprint mismatch - session revoked for security",
            ),
            crate::auth::AuthError::TokenInvalid(msg) => {
                ApiError::unauthenticated(format!("Invalid refresh token: {msg}"))
            }
            _ => ApiError::internal(format!("Refresh failed: {e}")),
        })?;

    let response = serde_json::json!({
        "access_token": token_pair.access_token,
        "refresh_token": token_pair.refresh_token,
        "expires_in": token_pair.expires_in,
        "token_type": token_pair.token_type,
    });

    Ok(negotiate_response(&headers, &response))
}

// ---------------------------------------------------------------------------
// Signout
// ---------------------------------------------------------------------------

/// `POST /api/auth/signout` -- Revoke the current session.
pub async fn auth_signout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(&headers)?;

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

    let auth_ctx = state
        .session_manager
        .validate_token(&token, ip, ua, dfp)
        .map_err(|e| ApiError::unauthenticated(format!("Invalid token: {e}")))?;

    state
        .session_manager
        .revoke_session(auth_ctx.session_id)
        .await
        .map_err(|e| ApiError::internal(format!("Failed to revoke session: {e}")))?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Me
// ---------------------------------------------------------------------------

/// `GET /api/auth/me` -- Return the authenticated user's profile.
pub async fn auth_me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let token = extract_bearer_token(&headers)?;

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

    let auth_ctx = state
        .session_manager
        .validate_token(&token, ip, ua, dfp)
        .map_err(|e| ApiError::unauthenticated(format!("Invalid token: {e}")))?;

    let user_row: Option<(
        Uuid,
        String,
        serde_json::Value,
        chrono::DateTime<chrono::Utc>,
    )> = sqlx::query_as(
        "SELECT id, email, roles, created_at FROM users WHERE id = $1 AND deleted_at IS NULL",
    )
    .bind(auth_ctx.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("Database error: {e}")))?;

    let (user_id, email, roles, created_at) =
        user_row.ok_or_else(|| ApiError::not_found("User not found"))?;

    let response = serde_json::json!({
        "user_id": user_id,
        "email": email,
        "roles": roles,
        "session_id": auth_ctx.session_id,
        "created_at": created_at.to_rfc3339()
    });

    Ok(negotiate_response(&headers, &response))
}
