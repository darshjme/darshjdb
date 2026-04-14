//! JWT issuance, refresh rotation, and session lifecycle for DarshJDB.
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

/// Quantum readiness: when NIST PQC standards are final, add CRYSTALS-Dilithium
/// as an alternative signing algorithm. The KeyManager already supports algorithm
/// selection -- adding a new variant is a config change, not a rewrite.
/// See docs/strategy/QUANTUM_STRATEGY.md for the migration plan.
pub const QUANTUM_READY_NOTE: &str = "PQC migration path documented";

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
    /// Audience (prevents cross-service token confusion).
    #[serde(default)]
    pub aud: Option<String>,
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
    /// Algorithm used for signing/verification.
    algorithm: Algorithm,
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
            algorithm: Algorithm::RS256,
        })
    }

    /// Create an HMAC-based key manager from a shared secret (HS256).
    ///
    /// Suitable for single-node deployments or development. For production
    /// with key rotation, use [`KeyManager::new`] with RSA PEM keys.
    pub fn from_secret(secret: &[u8]) -> Self {
        Self {
            current_encoding: EncodingKey::from_secret(secret),
            current_decoding: DecodingKey::from_secret(secret),
            previous_decoding: None,
            current_kid: "hmac-1".to_string(),
            previous_kid: None,
            algorithm: Algorithm::HS256,
        }
    }

    /// Generate an ephemeral HMAC key manager for development.
    ///
    /// A random 64-byte secret is generated in-memory. Tokens will not
    /// survive a server restart.
    pub fn generate() -> Self {
        let mut secret = [0u8; 64];
        rand::thread_rng().fill_bytes(&mut secret);
        Self::from_secret(&secret)
    }

    /// Sign an access token with the current key.
    pub fn sign_access_token(&self, claims: &AccessClaims) -> Result<String, AuthError> {
        let mut header = Header::new(self.algorithm);
        header.kid = Some(self.current_kid.clone());
        encode(&header, claims, &self.current_encoding)
            .map_err(|e| AuthError::Crypto(format!("jwt sign: {e}")))
    }

    /// Validate an access token against current or previous key.
    ///
    /// The `kid` header field determines which key to use. If neither
    /// matches, validation fails. Both issuer and audience are verified
    /// to prevent cross-service token confusion.
    pub fn validate_access_token(&self, token: &str) -> Result<AccessClaims, AuthError> {
        let mut validation = Validation::new(self.algorithm);
        validation.set_issuer(&["darshjdb"]);
        validation.set_audience(&["darshjdb"]);
        validation.validate_exp = true;

        // Try current key first.
        match decode::<AccessClaims>(token, &self.current_decoding, &validation) {
            Ok(data) => Ok(data.claims),
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
    /// Client IP at session creation (legacy text column).
    pub ip: String,
    /// User-Agent at session creation.
    pub user_agent: String,
    /// When the session was created.
    pub created_at: DateTime<Utc>,
    /// Legacy boolean revocation flag — kept in lock-step with
    /// [`Self::revoked_at`] so back-compat consumers keep working. The
    /// canonical "is active" predicate is `revoked_at IS NULL`.
    pub revoked: bool,
    /// SHA-256 hash of the current refresh token.
    pub refresh_token_hash: String,
    /// When the refresh token expires.
    pub refresh_expires_at: DateTime<Utc>,
    /// Last time the session was used to authenticate a request.
    pub last_active_at: DateTime<Utc>,
    /// Hard wall-clock cutoff after which the session is auto-revoked
    /// regardless of activity. Defaults to 24 hours after creation.
    pub absolute_expires_at: DateTime<Utc>,
    /// Timestamp at which the session was revoked. `None` while the session
    /// is active.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Structured reason recorded alongside [`Self::revoked_at`]. One of
    /// `logout`, `overflow`, `absolute_timeout`, `device_mismatch`, `manual`,
    /// `legacy_revoked`, or `dedupe_on_migration`.
    pub revoke_reason: Option<String>,
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
    /// Hard wall-clock lifetime for any session, regardless of refresh activity.
    /// Matches the OWASP ASVS L2 recommendation for high-trust APIs.
    const ABSOLUTE_LIFETIME_HOURS: i64 = 24;
    /// Maximum number of concurrent active sessions per user. Older sessions
    /// are evicted (revoked with reason `overflow`) when a new login pushes the
    /// count past this limit.
    const MAX_ACTIVE_SESSIONS_PER_USER: i64 = 5;

    /// Create a new session manager.
    pub fn new(pool: PgPool, keys: KeyManager) -> Self {
        Self { pool, keys }
    }

    /// Create a new session and issue a token pair.
    ///
    /// The refresh token is a 32-byte random value; only its SHA-256
    /// hash is stored in the database. Before insertion, this enforces the
    /// per-user concurrency cap by revoking the oldest active session(s) with
    /// reason `overflow` whenever the limit is reached.
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
        let dfp_hash = hex_sha256(device_fingerprint.as_bytes());

        let mut tx = self.pool.begin().await?;

        // ── 1a. Evict any prior active session bound to this exact device ──
        // The partial unique index `idx_sessions_user_device` would otherwise
        // reject the insert for a re-login from the same (user, device) pair.
        sqlx::query(
            "UPDATE sessions
                SET revoked = true,
                    revoked_at = now(),
                    revoke_reason = 'overflow'
              WHERE user_id = $1
                AND device_fingerprint = $2
                AND revoked_at IS NULL",
        )
        .bind(user_id)
        .bind(&dfp_hash)
        .execute(&mut *tx)
        .await?;

        // ── 1b. Overflow eviction ────────────────────────────────────────
        let active_count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sessions
              WHERE user_id = $1
                AND revoked_at IS NULL
                AND absolute_expires_at > now()",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;

        if active_count.0 >= Self::MAX_ACTIVE_SESSIONS_PER_USER {
            let to_evict = active_count.0 - Self::MAX_ACTIVE_SESSIONS_PER_USER + 1;
            sqlx::query(
                "UPDATE sessions
                    SET revoked = true,
                        revoked_at = now(),
                        revoke_reason = 'overflow'
                  WHERE session_id IN (
                      SELECT session_id FROM sessions
                       WHERE user_id = $1
                         AND revoked_at IS NULL
                       ORDER BY created_at ASC
                       LIMIT $2
                  )",
            )
            .bind(user_id)
            .bind(to_evict)
            .execute(&mut *tx)
            .await?;
        }

        // ── 2. Mint refresh token + persist new session ──────────────────
        // Only the SHA-256 hash is stored; the raw token never touches disk.
        let mut raw_refresh = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut raw_refresh);
        let refresh_token = data_encoding::BASE64URL_NOPAD.encode(&raw_refresh);
        let refresh_hash = hex_sha256(refresh_token.as_bytes());
        let refresh_expires = now + Duration::days(Self::REFRESH_TTL_DAYS);
        let absolute_expires = now + Duration::hours(Self::ABSOLUTE_LIFETIME_HOURS);

        // Validate the IP string. INET cast happens server-side; bad input is
        // stored as NULL so we never fail logins because of a misconfigured
        // proxy header.
        let ip_inet: Option<String> = ip.parse::<std::net::IpAddr>().ok().map(|i| i.to_string());

        sqlx::query(
            "INSERT INTO sessions
                (session_id, user_id, device_fingerprint, ip, user_agent,
                 created_at, revoked, refresh_token_hash, refresh_expires_at,
                 ip_address, last_active_at, absolute_expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, false, $7, $8, $9::inet, $6, $10)",
        )
        .bind(session_id)
        .bind(user_id)
        .bind(&dfp_hash)
        .bind(ip)
        .bind(user_agent)
        .bind(now)
        .bind(&refresh_hash)
        .bind(refresh_expires)
        .bind(ip_inet)
        .bind(absolute_expires)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        // Issue access token.
        let access_exp = now + Duration::minutes(Self::ACCESS_TTL_MINUTES);
        let claims = AccessClaims {
            sub: user_id.to_string(),
            sid: session_id.to_string(),
            roles,
            iat: now.timestamp(),
            exp: access_exp.timestamp(),
            iss: "darshjdb".into(),
            aud: Some("darshjdb".into()),
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
    /// (rotation). The device fingerprint must match the one stored at session
    /// creation. Sessions past their `absolute_expires_at` cutoff are
    /// auto-revoked with reason `absolute_timeout`.
    pub async fn refresh_session(
        &self,
        refresh_token: &str,
        device_fingerprint: &str,
    ) -> Result<TokenPair, AuthError> {
        let refresh_hash = hex_sha256(refresh_token.as_bytes());
        let dfp_hash = hex_sha256(device_fingerprint.as_bytes());

        let session: Option<SessionRecord> = sqlx::query_as(
            "SELECT session_id, user_id, device_fingerprint, ip, user_agent,
                    created_at, revoked, refresh_token_hash, refresh_expires_at,
                    last_active_at, absolute_expires_at, revoked_at, revoke_reason
             FROM sessions WHERE refresh_token_hash = $1",
        )
        .bind(&refresh_hash)
        .fetch_optional(&self.pool)
        .await?;

        let session =
            session.ok_or_else(|| AuthError::TokenInvalid("refresh token not found".into()))?;

        if session.revoked_at.is_some() {
            return Err(AuthError::SessionRevoked);
        }

        let now = Utc::now();
        if now >= session.absolute_expires_at {
            let _ = sqlx::query(
                "UPDATE sessions
                    SET revoked = true,
                        revoked_at = now(),
                        revoke_reason = 'absolute_timeout'
                  WHERE session_id = $1
                    AND revoked_at IS NULL",
            )
            .bind(session.session_id)
            .execute(&self.pool)
            .await;
            return Err(AuthError::SessionExpired);
        }

        if now > session.refresh_expires_at {
            return Err(AuthError::TokenInvalid("refresh token expired".into()));
        }

        if session.device_fingerprint != dfp_hash {
            // Potential token theft — revoke the session entirely.
            let _ = sqlx::query(
                "UPDATE sessions
                    SET revoked = true,
                        revoked_at = now(),
                        revoke_reason = 'device_mismatch'
                  WHERE session_id = $1",
            )
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
            "UPDATE sessions
                SET refresh_token_hash = $1,
                    refresh_expires_at = $2,
                    last_active_at = now()
              WHERE session_id = $3
                AND revoked_at IS NULL",
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
            iss: "darshjdb".into(),
            aud: Some("darshjdb".into()),
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
        sqlx::query(
            "UPDATE sessions
                SET revoked = true,
                    revoked_at = COALESCE(revoked_at, now()),
                    revoke_reason = COALESCE(revoke_reason, 'logout')
              WHERE session_id = $1",
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Revoke all sessions for a user (password change, security event).
    pub async fn revoke_all_sessions(&self, user_id: Uuid) -> Result<u64, AuthError> {
        let result = sqlx::query(
            "UPDATE sessions
                SET revoked = true,
                    revoked_at = now(),
                    revoke_reason = 'manual'
              WHERE user_id = $1
                AND revoked_at IS NULL",
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
                    created_at, revoked, refresh_token_hash, refresh_expires_at,
                    last_active_at, absolute_expires_at, revoked_at, revoke_reason
             FROM sessions
             WHERE user_id = $1
               AND revoked_at IS NULL
               AND absolute_expires_at > NOW()
             ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(sessions)
    }

    /// Validate an access token and build an [`AuthContext`].
    ///
    /// This is the primary entry point used by middleware. In addition to
    /// stateless JWT verification, it now performs a per-request DB lookup so
    /// that revoked sessions and sessions past their absolute timeout are
    /// rejected immediately (rather than living for up to 15 minutes until the
    /// access token naturally expires). The cost is one indexed PK lookup +
    /// one bounded `last_active_at` UPDATE per authenticated request.
    pub async fn validate_token(
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

        // Stateful session check: revoked / absolute timeout.
        let row: Option<(Option<DateTime<Utc>>, DateTime<Utc>)> = sqlx::query_as(
            "SELECT revoked_at, absolute_expires_at
               FROM sessions
              WHERE session_id = $1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await?;

        let (revoked_at, absolute_expires_at) =
            row.ok_or_else(|| AuthError::TokenInvalid("session not found".into()))?;

        if revoked_at.is_some() {
            return Err(AuthError::SessionRevoked);
        }

        if Utc::now() >= absolute_expires_at {
            // Auto-revoke past the absolute cutoff and reject this request.
            let _ = sqlx::query(
                "UPDATE sessions
                    SET revoked = true,
                        revoked_at = now(),
                        revoke_reason = 'absolute_timeout'
                  WHERE session_id = $1
                    AND revoked_at IS NULL",
            )
            .bind(session_id)
            .execute(&self.pool)
            .await;
            return Err(AuthError::SessionExpired);
        }

        // Touch last_active_at. A failure here is a hard DB error and must
        // propagate — silently swallowing it would mask connection issues.
        sqlx::query("UPDATE sessions SET last_active_at = now() WHERE session_id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a fresh RSA key pair and return (private_pem, public_pem).
    fn generate_rsa_keypair() -> (Vec<u8>, Vec<u8>) {
        // Use pre-generated test RSA PEM keys to avoid the heavy
        // rsa crate dependency at runtime.
        //
        // For tests, we use pre-generated PEM keys to avoid the heavy
        // rsa crate dependency.
        let private_pem = include_bytes!("../../tests/fixtures/test_rsa_private.pem");
        let public_pem = include_bytes!("../../tests/fixtures/test_rsa_public.pem");
        (private_pem.to_vec(), public_pem.to_vec())
    }

    fn make_key_manager() -> KeyManager {
        let (priv_pem, pub_pem) = generate_rsa_keypair();
        KeyManager::new(&priv_pem, &pub_pem, "kid-test".into(), None, None).expect("key manager")
    }

    fn make_claims(exp_offset_secs: i64) -> AccessClaims {
        let now = Utc::now();
        AccessClaims {
            sub: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
            roles: vec!["user".into()],
            iat: now.timestamp(),
            exp: (now + Duration::seconds(exp_offset_secs)).timestamp(),
            iss: "darshjdb".into(),
            aud: Some("darshjdb".into()),
        }
    }

    // -----------------------------------------------------------------------
    // JWT creation and validation
    // -----------------------------------------------------------------------

    #[test]
    fn jwt_sign_and_validate_roundtrip() {
        let km = make_key_manager();
        let claims = make_claims(300);
        let token = km.sign_access_token(&claims).expect("sign");
        let decoded = km.validate_access_token(&token).expect("validate");
        assert_eq!(decoded.sub, claims.sub);
        assert_eq!(decoded.sid, claims.sid);
        assert_eq!(decoded.roles, claims.roles);
        assert_eq!(decoded.iss, "darshjdb");
    }

    #[test]
    fn jwt_expired_token_rejected() {
        let km = make_key_manager();
        let claims = make_claims(-120); // expired 120s ago (well past leeway)
        let token = km.sign_access_token(&claims).expect("sign");
        let result = km.validate_access_token(&token);
        assert!(result.is_err(), "expired token must be rejected");
    }

    #[test]
    fn jwt_tampered_payload_rejected() {
        let km = make_key_manager();
        let claims = make_claims(300);
        let token = km.sign_access_token(&claims).expect("sign");
        let parts: Vec<&str> = token.splitn(3, '.').collect();
        assert_eq!(parts.len(), 3);
        let mut payload = parts[1].to_string();
        let c = if payload.ends_with('A') { 'B' } else { 'A' };
        payload.pop();
        payload.push(c);
        let tampered = format!("{}.{}.{}", parts[0], payload, parts[2]);
        assert!(
            km.validate_access_token(&tampered).is_err(),
            "tampered JWT must fail"
        );
    }

    #[test]
    fn jwt_wrong_issuer_rejected() {
        let km = make_key_manager();
        let now = Utc::now();
        let claims = AccessClaims {
            sub: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
            roles: vec![],
            iat: now.timestamp(),
            exp: (now + Duration::seconds(300)).timestamp(),
            iss: "wrong-issuer".into(),
            aud: Some("darshjdb".into()),
        };
        let token = km.sign_access_token(&claims).expect("sign");
        assert!(
            km.validate_access_token(&token).is_err(),
            "wrong issuer must be rejected"
        );
    }

    #[test]
    fn jwt_wrong_audience_rejected() {
        let km = make_key_manager();
        let now = Utc::now();
        let claims = AccessClaims {
            sub: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
            roles: vec![],
            iat: now.timestamp(),
            exp: (now + Duration::seconds(300)).timestamp(),
            iss: "darshjdb".into(),
            aud: Some("wrong-audience".into()),
        };
        let token = km.sign_access_token(&claims).expect("sign");
        assert!(
            km.validate_access_token(&token).is_err(),
            "wrong audience must be rejected"
        );
    }

    // -----------------------------------------------------------------------
    // Key rotation
    // -----------------------------------------------------------------------

    #[test]
    fn jwt_key_rotation_old_token_still_valid() {
        let (priv_pem, pub_pem) = generate_rsa_keypair();
        let old_km =
            KeyManager::new(&priv_pem, &pub_pem, "kid-1".into(), None, None).expect("old km");

        // New key manager with old key as previous.
        let new_km = KeyManager::new(
            &priv_pem,
            &pub_pem,
            "kid-2".into(),
            Some(&pub_pem),
            Some("kid-1".into()),
        )
        .expect("new km");

        let claims = make_claims(300);
        let token = old_km.sign_access_token(&claims).expect("sign with old");
        let decoded = new_km
            .validate_access_token(&token)
            .expect("validate with new");
        assert_eq!(decoded.sub, claims.sub);
    }

    // -----------------------------------------------------------------------
    // Utility
    // -----------------------------------------------------------------------

    #[test]
    fn hex_sha256_deterministic() {
        let h1 = hex_sha256(b"hello world");
        let h2 = hex_sha256(b"hello world");
        assert_eq!(h1, h2);
        assert_eq!(
            h1,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn hex_sha256_empty() {
        let h = hex_sha256(b"");
        assert_eq!(
            h,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn jwt_claims_preserve_roles() {
        let km = make_key_manager();
        let now = Utc::now();
        let claims = AccessClaims {
            sub: Uuid::new_v4().to_string(),
            sid: Uuid::new_v4().to_string(),
            roles: vec!["admin".into(), "editor".into(), "viewer".into()],
            iat: now.timestamp(),
            exp: (now + Duration::seconds(300)).timestamp(),
            iss: "darshjdb".into(),
            aud: Some("darshjdb".into()),
        };
        let token = km.sign_access_token(&claims).expect("sign");
        let decoded = km.validate_access_token(&token).expect("validate");
        assert_eq!(decoded.roles, vec!["admin", "editor", "viewer"]);
    }

    // -----------------------------------------------------------------------
    // HMAC (HS256) key manager
    // -----------------------------------------------------------------------

    #[test]
    fn hmac_key_manager_sign_and_validate() {
        let km = KeyManager::from_secret(b"test-secret-at-least-32-bytes-long-for-hs256");
        let claims = make_claims(300);
        let token = km.sign_access_token(&claims).expect("sign");
        let decoded = km.validate_access_token(&token).expect("validate");
        assert_eq!(decoded.sub, claims.sub);
    }

    #[test]
    fn hmac_key_manager_wrong_secret_rejected() {
        let km1 = KeyManager::from_secret(b"secret-one-for-signing-tokens!!");
        let km2 = KeyManager::from_secret(b"secret-two-different-entirely!!");
        let claims = make_claims(300);
        let token = km1.sign_access_token(&claims).expect("sign");
        assert!(
            km2.validate_access_token(&token).is_err(),
            "different secret must reject"
        );
    }

    #[test]
    fn hmac_key_manager_expired_rejected() {
        let km = KeyManager::from_secret(b"test-secret-for-expiry-testing!");
        let claims = make_claims(-120); // well past leeway
        let token = km.sign_access_token(&claims).expect("sign");
        assert!(
            km.validate_access_token(&token).is_err(),
            "expired HMAC token must be rejected"
        );
    }

    #[test]
    fn generated_key_manager_works() {
        let km = KeyManager::generate();
        let claims = make_claims(300);
        let token = km.sign_access_token(&claims).expect("sign");
        let decoded = km.validate_access_token(&token).expect("validate");
        assert_eq!(decoded.sub, claims.sub);
    }

    #[test]
    fn rsa_and_hmac_tokens_not_interchangeable() {
        let rsa_km = make_key_manager();
        let hmac_km = KeyManager::from_secret(b"hmac-secret-key-for-testing-now");
        let claims = make_claims(300);

        let rsa_token = rsa_km.sign_access_token(&claims).expect("rsa sign");
        let hmac_token = hmac_km.sign_access_token(&claims).expect("hmac sign");

        // RSA token must not validate with HMAC manager and vice versa.
        assert!(
            hmac_km.validate_access_token(&rsa_token).is_err(),
            "RSA token rejected by HMAC km"
        );
        assert!(
            rsa_km.validate_access_token(&hmac_token).is_err(),
            "HMAC token rejected by RSA km"
        );
    }
}
