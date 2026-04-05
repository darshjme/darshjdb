//! JWT issuance, refresh rotation, and session lifecycle for DarshanDB.
//!
//! # Token Architecture
//!
//! - **Access token**: RS256 JWT, 15-minute lifetime, stateless validation.
//! - **Refresh token**: Opaque 32-byte token stored as SHA-256 hash in Postgres,
//!   bound to a device fingerprint and valid for 30 days.
//!
//! # Key Rotation
//!
//! The [`KeyManager`] holds two RSA key pairs: *current* (used for signing)
//! and *previous* (accepted for verification only). On rotation, the current
//! key becomes previous and a new key is generated. This provides a grace
//! window for tokens signed with the old key.
//!
//! # Session Tracking
//!
//! Every login creates a [`SessionRecord`] in Postgres, enabling per-device
//! visibility and remote revocation.

use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sqlx::PgPool;
use uuid::Uuid;

use super::{AuthContext, AuthError};

// ---------------------------------------------------------------------------
// JWT claims
// ---------------------------------------------------------------------------

/// Claims embedded in every access token.
#[derive(Debug, Serialize, Deserialize)]
pub struct AccessClaims {
    /// Subject (user ID).
    pub sub: String,
    /// Session ID.
    pub sid: String,
    /// Roles.
    pub roles: Vec<String>,
    /// Issued at (Unix timestamp).
    pub iat: i64,
    /// Expiration (Unix timestamp).
    pub exp: i64,
    /// Issuer.
    pub iss: String,
}

/// Claims embedded in refresh tokens (for the rare case they are JWTs;
/// normally we use opaque tokens, but the claims struct is here for
/// flexibility).
#[derive(Debug, Serialize, Deserialize)]
pub struct RefreshClaims {
    /// Subject (user ID).
    pub sub: String,
    /// Session ID.
    pub sid: String,
    /// Device fingerprint hash.
    pub dfp: String,
    /// Issued at.
    pub iat: i64,
    /// Expiration.
    pub exp: i64,
}

// ---------------------------------------------------------------------------
// Key manager
// ---------------------------------------------------------------------------

/// Holds RSA key pairs for JWT signing and verification.
///
/// Two keys are maintained to support rotation without invalidating
/// recently-issued tokens.
pub struct KeyManager {
    /// Current signing key.
    current_encoding: EncodingKey,
    /// Current verification key.
    current_decoding: DecodingKey,
    /// Previous verification key (accepted during rotation grace period).
    previous_decoding: Option<DecodingKey>,
    /// Key ID for the current key (embedded in JWT header).
    current_kid: String,
    /// Key ID for the previous key (retained for JWKS endpoint publishing).
    #[allow(dead_code)]
    previous_kid: Option<String>,
}

impl KeyManager {
    /// Create a new key manager from PEM-encoded RSA keys.
    ///
    /// `current_private_pem` is the active signing key.
    /// `current_public_pem` is the corresponding verification key.
    /// `previous_public_pem` is optional — the old key still accepted
    /// for verification.
    pub fn new(
        current_private_pem: &[u8],
        current_public_pem: &[u8],
        current_kid: String,
        previous_public_pem: Option<&[u8]>,
        previous_kid: Option<String>,
    ) -> Result<Self, AuthError> {
        let current_encoding = EncodingKey::from_rsa_pem(current_private_pem)
            .map_err(|e| AuthError::Crypto(format!("encoding key: {e}")))?;
        let current_decoding = DecodingKey::from_rsa_pem(current_public_pem)
            .map_err(|e| AuthError::Crypto(format!("decoding key: {e}")))?;

        let previous_decoding = previous_public_pem
            .map(|pem| {
                DecodingKey::from_rsa_pem(pem)
                    .map_err(|e| AuthError::Crypto(format!("previous key: {e}")))
            })
            .transpose()?;

        Ok(Self {
            current_encoding,
            current_decoding,
            previous_decoding,
            current_kid,
            previous_kid,
        })
    }

    /// Sign an access token with the current key.
    pub fn sign_access_token(&self, claims: &AccessClaims) -> Result<String, AuthError> {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(self.current_kid.clone());
        encode(&header, claims, &self.current_encoding)
            .map_err(|e| AuthError::Crypto(format!("jwt sign: {e}")))
    }

    /// Validate an access token against current or previous key.
    ///
    /// The `kid` header field determines which key to use. If neither
    /// matches, validation fails.
    pub fn validate_access_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_issuer(&["darshandb"]);
        validation.validate_exp = true;

        // Try current key first.
        match decode::<AccessClaims>(token, &self.current_decoding, &validation) {
            Ok(data) => return Ok(data.claims),
            Err(e) => {
                // If we have a previous key, try that.
                if let Some(ref prev) = self.previous_decoding {
                    match decode::<AccessClaims>(token, prev, &validation) {
                        Ok(data) => return Ok(data.claims),
                        Err(_) => {
                            return Err(AuthError::TokenInvalid(format!(
                                "neither current nor previous key: {e}"
                            )));
                        }
                    }
                }
                Err(AuthError::TokenInvalid(e.to_string()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Session record
// ---------------------------------------------------------------------------

/// A persistent session row in Postgres.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SessionRecord {
    /// Unique session identifier.
    pub session_id: Uuid,
    /// The user this session belongs to.
    pub user_id: Uuid,
    /// SHA-256 of the device fingerprint.
    pub device_fingerprint: String,
    /// Client IP at session creation.
    pub ip: String,
    /// User-Agent at session creation.
    pub user_agent: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// Whether the session has been revoked.
    pub revoked: bool,
    /// SHA-256 hash of the current refresh token.
    pub refresh_token_hash: String,
    /// When the refresh token expires.
    pub refresh_expires_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Token pair
// ---------------------------------------------------------------------------

/// An access + refresh token pair returned on successful authentication.
#[derive(Debug, Serialize, Deserialize)]
pub struct TokenPair {
    /// The short-lived access token (JWT).
    pub access_token: String,
    /// The long-lived refresh token (opaque, base64url).
    pub refresh_token: String,
    /// Access token expiry in seconds.
    pub expires_in: i64,
    /// Token type (always "Bearer").
    pub token_type: String,
}

// ---------------------------------------------------------------------------
// Session manager
// ---------------------------------------------------------------------------

/// Manages the full session lifecycle: creation, refresh, revocation.
pub struct SessionManager {
    pool: PgPool,
    keys: KeyManager,
}

impl SessionManager {
    /// Access token lifetime.
    const ACCESS_TTL_MINUTES: i64 = 15;
    /// Refresh token lifetime.
    const REFRESH_TTL_DAYS: i64 = 30;

    /// Create a new session manager.
    pub fn new(pool: PgPool, keys: KeyManager) -> Self {
        Self { pool, keys }
    }

    /// Create a new session and issue a token pair.
    ///
    /// The refresh token is a 32-byte random value; only its SHA-256
    /// hash is stored in the database.
    pub async fn create_session(
        &self,
        user_id: Uuid,
        roles: Vec<String>,
        ip: &str,
        user_agent: &str,
        device_fingerprint: &str,
    ) -> Result<TokenPair, AuthError> {
        let session_id = Uuid::new_v4();
        let now = Utc::now();

        // Generate opaque refresh token.
        let mut raw_refresh = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw_refresh);
        let refresh_token = data_encoding::BASE64URL_NOPAD.encode(&raw_refresh);
        let refresh_hash = hex_sha256(refresh_token.as_bytes());
        let refresh_expires = now + Duration::days(Self::REFRESH_TTL_DAYS);

        let dfp_hash = hex_sha256(device_fingerprint.as_bytes());

        // Persist session.
        sqlx::query(
            "INSERT INTO sessions
                (session_id, user_id, device_fingerprint, ip, user_agent,
                 created_at, revoked, refresh_token_hash, refresh_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, false, $7, $8)",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(&dfp_hash)
        .bind(ip)
        .bind(user_agent)
        .bind(now)
        .bind(&refresh_hash)
        .bind(refresh_expires)
        .execute(&self.pool)
        .await?;

        // Issue access token.
        let access_exp = now + Duration::minutes(Self::ACCESS_TTL_MINUTES);
        let claims = AccessClaims {
            sub: user_id.to_string(),
            sid: session_id.to_string(),
            roles,
            iat: now.timestamp(),
            exp: access_exp.timestamp(),
            iss: "darshandb".into(),
        };

        let access_token = self.keys.sign_access_token(&claims)?;

        Ok(TokenPair {
            access_token,
            refresh_token,
            expires_in: Self::ACCESS_TTL_MINUTES * 60,
            token_type: "Bearer".into(),
        })
    }

    /// Refresh an existing session, issuing a new token pair.
    ///
    /// The old refresh token is consumed and a new one is generated
    /// (rotation). The device fingerprint must match the one stored
    /// at session creation.
    pub async fn refresh_session(
        &self,
        refresh_token: &str,
        device_fingerprint: &str,
    ) -> Result<TokenPair, AuthError> {
        let refresh_hash = hex_sha256(refresh_token.as_bytes());
        let dfp_hash = hex_sha256(device_fingerprint.as_bytes());

        let session: Option<SessionRecord> = sqlx::query_as(
            "SELECT session_id, user_id, device_fingerprint, ip, user_agent,
                    created_at, revoked, refresh_token_hash, refresh_expires_at
             FROM sessions WHERE refresh_token_hash = $1",
        )
        .bind(&refresh_hash)
        .fetch_optional(&self.pool)
        .await?;

        let session =
            session.ok_or_else(|| AuthError::TokenInvalid("refresh token not found".into()))?;

        if session.revoked {
            return Err(AuthError::SessionRevoked);
        }

        if Utc::now() > session.refresh_expires_at {
            return Err(AuthError::TokenInvalid("refresh token expired".into()));
        }

        if session.device_fingerprint != dfp_hash {
            // Potential token theft — revoke the session entirely.
            let _ = sqlx::query("UPDATE sessions SET revoked = true WHERE session_id = $1")
                .bind(session.session_id)
                .execute(&self.pool)
                .await;
            return Err(AuthError::DeviceMismatch);
        }

        // Rotate the refresh token.
        let mut new_raw = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut new_raw);
        let new_refresh = data_encoding::BASE64URL_NOPAD.encode(&new_raw);
        let new_hash = hex_sha256(new_refresh.as_bytes());
        let new_expires = Utc::now() + Duration::days(Self::REFRESH_TTL_DAYS);

        sqlx::query(
            "UPDATE sessions SET refresh_token_hash = $1, refresh_expires_at = $2
             WHERE session_id = $3 AND revoked = false",
        )
        .bind(&new_hash)
        .bind(new_expires)
        .bind(session.session_id)
        .execute(&self.pool)
        .await?;

        // Fetch current roles.
        let roles: Vec<String> = sqlx::query_scalar("SELECT roles FROM users WHERE id = $1")
            .bind(session.user_id)
            .fetch_optional(&self.pool)
            .await?
            .and_then(|v: serde_json::Value| serde_json::from_value(v).ok())
            .unwrap_or_default();

        // Issue new access token.
        let now = Utc::now();
        let access_exp = now + Duration::minutes(Self::ACCESS_TTL_MINUTES);
        let claims = AccessClaims {
            sub: session.user_id.to_string(),
            sid: session.session_id.to_string(),
            roles,
            iat: now.timestamp(),
            exp: access_exp.timestamp(),
            iss: "darshandb".into(),
        };

        let access_token = self.keys.sign_access_token(&claims)?;

        Ok(TokenPair {
            access_token,
            refresh_token: new_refresh,
            expires_in: Self::ACCESS_TTL_MINUTES * 60,
            token_type: "Bearer".into(),
        })
    }

    /// Revoke a specific session (logout).
    pub async fn revoke_session(&self, session_id: Uuid) -> Result<(), AuthError> {
        sqlx::query("UPDATE sessions SET revoked = true WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Revoke all sessions for a user (password change, security event).
    pub async fn revoke_all_sessions(&self, user_id: Uuid) -> Result<u64, AuthError> {
        let result = sqlx::query(
            "UPDATE sessions SET revoked = true WHERE user_id = $1 AND revoked = false",
        )
        .bind(user_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// List active (non-revoked, non-expired) sessions for a user.
    pub async fn list_sessions(&self, user_id: Uuid) -> Result<Vec<SessionRecord>, AuthError> {
        let sessions: Vec<SessionRecord> = sqlx::query_as(
            "SELECT session_id, user_id, device_fingerprint, ip, user_agent,
                    created_at, revoked, refresh_token_hash, refresh_expires_at
             FROM sessions
             WHERE user_id = $1 AND revoked = false AND refresh_expires_at > NOW()
             ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions)
    }

    /// Validate an access token and build an [`AuthContext`].
    ///
    /// This is the primary entry point used by middleware.
    pub fn validate_token(
        &self,
        token: &str,
        ip: &str,
        user_agent: &str,
        device_fingerprint: &str,
    ) -> Result<AuthContext, AuthError> {
        let claims = self.keys.validate_access_token(token)?;

        let user_id = Uuid::parse_str(&claims.sub)
            .map_err(|e| AuthError::TokenInvalid(format!("bad user id: {e}")))?;
        let session_id = Uuid::parse_str(&claims.sid)
            .map_err(|e| AuthError::TokenInvalid(format!("bad session id: {e}")))?;

        Ok(AuthContext {
            user_id,
            session_id,
            roles: claims.roles,
            ip: ip.to_string(),
            user_agent: user_agent.to_string(),
            device_fingerprint: device_fingerprint.to_string(),
        })
    }
}

/// Compute hex-encoded SHA-256 digest.
fn hex_sha256(input: &[u8]) -> String {
    let digest = sha2::Sha256::digest(input);
    data_encoding::HEXLOWER.encode(&digest)
}
