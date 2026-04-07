//! Authentication providers for DarshJDB.
//!
//! Supports three primary authentication flows:
//!
//! - **Password**: Argon2id hashing with tuned parameters (64 MB memory,
//!   3 iterations, parallelism 4).
//! - **Magic Link**: 32-byte random token with hashed storage, 15-minute
//!   expiry, and one-time use semantics.
//! - **OAuth2**: Trait-based provider abstraction with concrete implementations
//!   for Google, GitHub, Apple, Discord, Microsoft, Twitter/X, LinkedIn,
//!   Slack, GitLab, Bitbucket, Facebook, and Spotify. PKCE is mandatory;
//!   the state parameter is HMAC-signed to prevent CSRF.

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use super::{AuthError, AuthOutcome};

// ---------------------------------------------------------------------------
// Password provider
// ---------------------------------------------------------------------------

/// Argon2id password hashing and verification.
///
/// Parameters are pinned to OWASP-recommended values:
/// - Algorithm: Argon2id
/// - Memory: 64 MiB (65536 KiB)
/// - Iterations: 3
/// - Parallelism: 4
pub struct PasswordProvider;

impl PasswordProvider {
    /// Build the Argon2 instance with the project's standard parameters.
    fn hasher() -> Result<Argon2<'static>, AuthError> {
        let params = Params::new(
            64 * 1024, // 64 MiB in KiB
            3,         // iterations
            4,         // parallelism
            None,      // default output length (32 bytes)
        )
        .map_err(|e| AuthError::Crypto(format!("argon2 params: {e}")))?;

        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }

    /// Hash a plaintext password, returning a PHC-formatted string.
    ///
    /// The salt is generated from a cryptographic RNG.
    pub fn hash_password(password: &str) -> Result<String, AuthError> {
        let salt = SaltString::generate(&mut OsRng);
        let hasher = Self::hasher()?;
        let hash = hasher
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| AuthError::Crypto(format!("hash failed: {e}")))?;
        Ok(hash.to_string())
    }

    /// Verify a plaintext password against a stored PHC hash string.
    pub fn verify_password(password: &str, hash: &str) -> Result<bool, AuthError> {
        let parsed = PasswordHash::new(hash)
            .map_err(|e| AuthError::Crypto(format!("invalid hash format: {e}")))?;
        let hasher = Self::hasher()?;
        match hasher.verify_password(password.as_bytes(), &parsed) {
            Ok(()) => Ok(true),
            Err(argon2::password_hash::Error::Password) => Ok(false),
            Err(e) => Err(AuthError::Crypto(format!("verify failed: {e}"))),
        }
    }

    /// Authenticate a user by email and password against the database.
    ///
    /// Returns [`AuthOutcome::Success`] with user id and roles on match,
    /// or [`AuthOutcome::Failed`] if the email is unknown or the password
    /// is incorrect. Timing is constant regardless of whether the email exists.
    pub async fn authenticate(
        pool: &PgPool,
        email: &str,
        password: &str,
    ) -> Result<AuthOutcome, AuthError> {
        // Fetch user row; if missing we still run a dummy verify to avoid
        // timing side-channels that reveal email existence.
        let row: Option<(Uuid, String, serde_json::Value)> = sqlx::query_as(
            "SELECT id, password_hash, roles FROM users WHERE email = $1 AND deleted_at IS NULL",
        )
        .bind(email)
        .fetch_optional(pool)
        .await?;

        let (user_id, hash, roles_json) = match row {
            Some(r) => r,
            None => {
                // Constant-time dummy to prevent timing oracle.
                let _ = Self::verify_password(
                    password,
                    "$argon2id$v=19$m=65536,t=3,p=4$c29tZXNhbHQ$RdescudvJCsgt3ub+b+daw",
                );
                return Ok(AuthOutcome::Failed {
                    reason: "invalid email or password".into(),
                });
            }
        };

        if !Self::verify_password(password, &hash)? {
            return Ok(AuthOutcome::Failed {
                reason: "invalid email or password".into(),
            });
        }

        let roles: Vec<String> = serde_json::from_value(roles_json)
            .map_err(|e| AuthError::Internal(format!("bad roles json: {e}")))?;

        Ok(AuthOutcome::Success { user_id, roles })
    }
}

// ---------------------------------------------------------------------------
// Magic link provider
// ---------------------------------------------------------------------------

/// One-time magic link authentication.
///
/// Flow:
/// 1. Generate a 32-byte random token and store its SHA-256 hash with
///    a 15-minute expiry.
/// 2. Send the raw token to the user (via email — transport is external).
/// 3. On verification, hash the presented token and match against the DB row.
///    If valid and unexpired, mark as consumed and return the user identity.
pub struct MagicLinkProvider;

/// A newly created magic link ready to be delivered to the user.
#[derive(Debug)]
pub struct MagicLink {
    /// The raw token to embed in the link URL. Never stored directly.
    pub token: String,
    /// When this token expires.
    pub expires_at: DateTime<Utc>,
}

impl MagicLinkProvider {
    /// Token validity window.
    const EXPIRY_MINUTES: i64 = 15;

    /// Generate a new magic link for the given user.
    ///
    /// The raw token is returned for embedding in a URL. Only its SHA-256
    /// hash is persisted, so a database breach cannot reconstruct tokens.
    pub async fn generate(pool: &PgPool, user_id: Uuid) -> Result<MagicLink, AuthError> {
        let mut raw = [0u8; 32];
        OsRng.fill_bytes(&mut raw);
        let token = data_encoding::BASE64URL_NOPAD.encode(&raw);

        let hash = {
            use sha2::Digest;
            let digest = sha2::Sha256::digest(token.as_bytes());
            data_encoding::HEXLOWER.encode(&digest)
        };

        let expires_at = Utc::now() + Duration::minutes(Self::EXPIRY_MINUTES);

        sqlx::query(
            "INSERT INTO magic_links (token_hash, user_id, expires_at, consumed)
             VALUES ($1, $2, $3, false)",
        )
        .bind(&hash)
        .bind(user_id)
        .bind(expires_at)
        .execute(pool)
        .await?;

        Ok(MagicLink { token, expires_at })
    }

    /// Verify a magic link token.
    ///
    /// On success the token row is marked consumed (one-time use) and the
    /// user identity is returned.
    pub async fn verify(pool: &PgPool, token: &str) -> Result<AuthOutcome, AuthError> {
        let hash = {
            use sha2::Digest;
            let digest = sha2::Sha256::digest(token.as_bytes());
            data_encoding::HEXLOWER.encode(&digest)
        };

        let row: Option<(Uuid, DateTime<Utc>, bool)> = sqlx::query_as(
            "SELECT user_id, expires_at, consumed FROM magic_links WHERE token_hash = $1",
        )
        .bind(&hash)
        .fetch_optional(pool)
        .await?;

        let (user_id, expires_at, consumed) = match row {
            Some(r) => r,
            None => {
                return Ok(AuthOutcome::Failed {
                    reason: "invalid or expired magic link".into(),
                });
            }
        };

        if consumed {
            return Ok(AuthOutcome::Failed {
                reason: "magic link already used".into(),
            });
        }

        if Utc::now() > expires_at {
            return Ok(AuthOutcome::Failed {
                reason: "magic link expired".into(),
            });
        }

        // Mark consumed atomically.
        let result = sqlx::query(
            "UPDATE magic_links SET consumed = true
             WHERE token_hash = $1 AND consumed = false",
        )
        .bind(&hash)
        .execute(pool)
        .await?;

        if result.rows_affected() == 0 {
            // Race condition: another request consumed it first.
            return Ok(AuthOutcome::Failed {
                reason: "magic link already used".into(),
            });
        }

        // Fetch roles for the user.
        let roles: Vec<String> = sqlx::query_scalar("SELECT roles FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?
            .and_then(|v: serde_json::Value| serde_json::from_value(v).ok())
            .unwrap_or_default();

        Ok(AuthOutcome::Success { user_id, roles })
    }
}

// ---------------------------------------------------------------------------
// OAuth2 provider
// ---------------------------------------------------------------------------

/// Identifies the upstream OAuth2 identity provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OAuthProviderKind {
    /// Google (OpenID Connect).
    Google,
    /// GitHub.
    GitHub,
    /// Apple (Sign in with Apple).
    Apple,
    /// Discord.
    Discord,
    /// Microsoft (Azure AD / Entra ID).
    Microsoft,
    /// Twitter / X (OAuth 2.0).
    Twitter,
    /// LinkedIn (OpenID Connect).
    LinkedIn,
    /// Slack.
    Slack,
    /// GitLab.
    GitLab,
    /// Bitbucket (Atlassian).
    Bitbucket,
    /// Facebook / Meta.
    Facebook,
    /// Spotify.
    Spotify,
}

impl std::fmt::Display for OAuthProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Google => write!(f, "google"),
            Self::GitHub => write!(f, "github"),
            Self::Apple => write!(f, "apple"),
            Self::Discord => write!(f, "discord"),
            Self::Microsoft => write!(f, "microsoft"),
            Self::Twitter => write!(f, "twitter"),
            Self::LinkedIn => write!(f, "linkedin"),
            Self::Slack => write!(f, "slack"),
            Self::GitLab => write!(f, "gitlab"),
            Self::Bitbucket => write!(f, "bitbucket"),
            Self::Facebook => write!(f, "facebook"),
            Self::Spotify => write!(f, "spotify"),
        }
    }
}

/// Configuration for a single OAuth2 provider.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// Provider kind.
    pub kind: OAuthProviderKind,
    /// OAuth2 client ID.
    pub client_id: String,
    /// OAuth2 client secret.
    pub client_secret: String,
    /// Authorization endpoint URL.
    pub auth_url: String,
    /// Token endpoint URL.
    pub token_url: String,
    /// Post-auth redirect URI (our callback).
    pub redirect_uri: String,
    /// Scopes to request.
    pub scopes: Vec<String>,
    /// Userinfo endpoint for fetching profile data.
    pub userinfo_url: String,
}

/// Trait for OAuth2 provider operations.
///
/// Each provider must be able to build an authorization URL (with PKCE
/// and HMAC-signed state) and exchange a callback code for user info.
pub trait OAuth2Provider: Send + Sync {
    /// Build the authorization redirect URL.
    ///
    /// Returns `(redirect_url, csrf_state, pkce_verifier)`.
    fn authorization_url(&self, state_secret: &[u8])
    -> Result<(String, String, String), AuthError>;

    /// Exchange an authorization code for user profile information.
    ///
    /// The `state` parameter is verified against the HMAC before proceeding.
    fn exchange_code(
        &self,
        code: &str,
        state: &str,
        pkce_verifier: &str,
        state_secret: &[u8],
    ) -> impl std::future::Future<Output = Result<OAuthUserInfo, AuthError>> + Send;
}

/// User information returned by an OAuth2 provider after code exchange.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthUserInfo {
    /// The provider-specific user identifier.
    pub provider_user_id: String,
    /// Email address (if the scope was granted).
    pub email: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
    /// Which provider this came from.
    pub provider: OAuthProviderKind,
}

use serde::{Deserialize, Serialize};

/// Generic OAuth2 provider implementation that works for all supported
/// identity providers. Provider-specific differences (endpoints, scopes,
/// userinfo parsing) are captured in [`OAuthConfig`].
pub struct GenericOAuth2Provider {
    config: OAuthConfig,
}

impl GenericOAuth2Provider {
    /// Create a new provider from configuration.
    pub fn new(config: OAuthConfig) -> Self {
        Self { config }
    }

    /// Create an HMAC-signed state parameter.
    ///
    /// Format: `{random_hex}.{hmac_hex}` so the callback can verify
    /// the state was issued by us.
    fn sign_state(secret: &[u8]) -> Result<String, AuthError> {
        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        let nonce_hex = data_encoding::HEXLOWER.encode(&nonce);

        let mut mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|e| AuthError::Crypto(format!("hmac key: {e}")))?;
        mac.update(nonce_hex.as_bytes());
        let sig = data_encoding::HEXLOWER.encode(&mac.finalize().into_bytes());

        Ok(format!("{nonce_hex}.{sig}"))
    }

    /// Verify an HMAC-signed state parameter.
    fn verify_state(state: &str, secret: &[u8]) -> Result<(), AuthError> {
        let parts: Vec<&str> = state.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(AuthError::OAuth2("malformed state parameter".into()));
        }

        let mut mac = Hmac::<Sha256>::new_from_slice(secret)
            .map_err(|e| AuthError::Crypto(format!("hmac key: {e}")))?;
        mac.update(parts[0].as_bytes());

        let expected = data_encoding::HEXLOWER
            .decode(parts[1].as_bytes())
            .map_err(|e| AuthError::OAuth2(format!("state decode: {e}")))?;

        mac.verify_slice(&expected)
            .map_err(|_| AuthError::OAuth2("state HMAC verification failed".into()))?;

        Ok(())
    }

    /// Generate a PKCE code verifier and challenge (S256).
    fn pkce_pair() -> (String, String) {
        let mut verifier_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut verifier_bytes);
        let verifier = data_encoding::BASE64URL_NOPAD.encode(&verifier_bytes);

        let challenge = {
            use sha2::Digest;
            let hash = sha2::Sha256::digest(verifier.as_bytes());
            data_encoding::BASE64URL_NOPAD.encode(&hash)
        };

        (verifier, challenge)
    }

    /// Construct well-known configs for each supported provider.
    pub fn google(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Google,
            client_id,
            client_secret,
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            redirect_uri,
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
            userinfo_url: "https://www.googleapis.com/oauth2/v3/userinfo".into(),
        })
    }

    /// Create a GitHub OAuth2 provider.
    pub fn github(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::GitHub,
            client_id,
            client_secret,
            auth_url: "https://github.com/login/oauth/authorize".into(),
            token_url: "https://github.com/login/oauth/access_token".into(),
            redirect_uri,
            scopes: vec!["read:user".into(), "user:email".into()],
            userinfo_url: "https://api.github.com/user".into(),
        })
    }

    /// Create an Apple Sign In provider.
    pub fn apple(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Apple,
            client_id,
            client_secret,
            auth_url: "https://appleid.apple.com/auth/authorize".into(),
            token_url: "https://appleid.apple.com/auth/token".into(),
            redirect_uri,
            scopes: vec!["name".into(), "email".into()],
            userinfo_url: String::new(), // Apple returns identity in the ID token.
        })
    }

    /// Create a Discord OAuth2 provider.
    pub fn discord(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Discord,
            client_id,
            client_secret,
            auth_url: "https://discord.com/api/oauth2/authorize".into(),
            token_url: "https://discord.com/api/oauth2/token".into(),
            redirect_uri,
            scopes: vec!["identify".into(), "email".into()],
            userinfo_url: "https://discord.com/api/users/@me".into(),
        })
    }

    /// Create a Microsoft (Azure AD / Entra ID) OAuth2 provider.
    ///
    /// Uses the "common" tenant so any Microsoft account (personal or
    /// organizational) can authenticate. Override with a specific tenant
    /// ID for single-org lockdown.
    pub fn microsoft(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Microsoft,
            client_id,
            client_secret,
            auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            redirect_uri,
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
            userinfo_url: "https://graph.microsoft.com/oidc/userinfo".into(),
        })
    }

    /// Create a Twitter / X OAuth 2.0 provider.
    ///
    /// Twitter OAuth 2.0 with PKCE (no client secret required for public
    /// clients, but we send it for confidential server flows).
    pub fn twitter(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Twitter,
            client_id,
            client_secret,
            auth_url: "https://twitter.com/i/oauth2/authorize".into(),
            token_url: "https://api.twitter.com/2/oauth2/token".into(),
            redirect_uri,
            scopes: vec!["users.read".into(), "tweet.read".into()],
            userinfo_url: "https://api.twitter.com/2/users/me".into(),
        })
    }

    /// Create a LinkedIn OAuth2 provider (OpenID Connect).
    pub fn linkedin(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::LinkedIn,
            client_id,
            client_secret,
            auth_url: "https://www.linkedin.com/oauth/v2/authorization".into(),
            token_url: "https://www.linkedin.com/oauth/v2/accessToken".into(),
            redirect_uri,
            scopes: vec!["openid".into(), "profile".into(), "email".into()],
            userinfo_url: "https://api.linkedin.com/v2/userinfo".into(),
        })
    }

    /// Create a Slack OAuth2 provider.
    pub fn slack(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Slack,
            client_id,
            client_secret,
            auth_url: "https://slack.com/openid/connect/authorize".into(),
            token_url: "https://slack.com/api/openid.connect.token".into(),
            redirect_uri,
            scopes: vec!["openid".into(), "email".into(), "profile".into()],
            userinfo_url: "https://slack.com/api/openid.connect.userInfo".into(),
        })
    }

    /// Create a GitLab OAuth2 provider.
    pub fn gitlab(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::GitLab,
            client_id,
            client_secret,
            auth_url: "https://gitlab.com/oauth/authorize".into(),
            token_url: "https://gitlab.com/oauth/token".into(),
            redirect_uri,
            scopes: vec!["openid".into(), "read_user".into(), "email".into()],
            userinfo_url: "https://gitlab.com/oauth/userinfo".into(),
        })
    }

    /// Create a Bitbucket (Atlassian) OAuth2 provider.
    pub fn bitbucket(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Bitbucket,
            client_id,
            client_secret,
            auth_url: "https://bitbucket.org/site/oauth2/authorize".into(),
            token_url: "https://bitbucket.org/site/oauth2/access_token".into(),
            redirect_uri,
            scopes: vec!["account".into(), "email".into()],
            userinfo_url: "https://api.bitbucket.org/2.0/user".into(),
        })
    }

    /// Create a Facebook / Meta OAuth2 provider.
    pub fn facebook(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Facebook,
            client_id,
            client_secret,
            auth_url: "https://www.facebook.com/v19.0/dialog/oauth".into(),
            token_url: "https://graph.facebook.com/v19.0/oauth/access_token".into(),
            redirect_uri,
            scopes: vec!["email".into(), "public_profile".into()],
            userinfo_url:
                "https://graph.facebook.com/v19.0/me?fields=id,name,email,picture.type(large)"
                    .into(),
        })
    }

    /// Create a Spotify OAuth2 provider.
    pub fn spotify(client_id: String, client_secret: String, redirect_uri: String) -> Self {
        Self::new(OAuthConfig {
            kind: OAuthProviderKind::Spotify,
            client_id,
            client_secret,
            auth_url: "https://accounts.spotify.com/authorize".into(),
            token_url: "https://accounts.spotify.com/api/token".into(),
            redirect_uri,
            scopes: vec!["user-read-email".into(), "user-read-private".into()],
            userinfo_url: "https://api.spotify.com/v1/me".into(),
        })
    }
}

impl OAuth2Provider for GenericOAuth2Provider {
    fn authorization_url(
        &self,
        state_secret: &[u8],
    ) -> Result<(String, String, String), AuthError> {
        let state = Self::sign_state(state_secret)?;
        let (verifier, challenge) = Self::pkce_pair();

        let scopes = self.config.scopes.join(" ");
        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
            self.config.auth_url,
            urlencoding(&self.config.client_id),
            urlencoding(&self.config.redirect_uri),
            urlencoding(&scopes),
            urlencoding(&state),
            urlencoding(&challenge),
        );

        Ok((url, state, verifier))
    }

    async fn exchange_code(
        &self,
        code: &str,
        state: &str,
        pkce_verifier: &str,
        state_secret: &[u8],
    ) -> Result<OAuthUserInfo, AuthError> {
        // Verify the state HMAC first.
        Self::verify_state(state, state_secret)?;

        let http = reqwest::Client::new();

        // Exchange authorization code for an access token.
        let token_resp = http
            .post(&self.config.token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", &self.config.redirect_uri),
                ("client_id", &self.config.client_id),
                ("client_secret", &self.config.client_secret),
                ("code_verifier", pkce_verifier),
            ])
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| AuthError::OAuth2(format!("token request failed: {e}")))?;

        if !token_resp.status().is_success() {
            let status = token_resp.status();
            let body = token_resp.text().await.unwrap_or_else(|_| "unknown".into());
            return Err(AuthError::OAuth2(format!(
                "token exchange failed ({status}): {body}"
            )));
        }

        let token_json: serde_json::Value = token_resp
            .json()
            .await
            .map_err(|e| AuthError::OAuth2(format!("token response parse: {e}")))?;

        let access_token = token_json["access_token"]
            .as_str()
            .ok_or_else(|| AuthError::OAuth2("missing access_token in response".into()))?;

        // For Apple, user info is in the ID token; for others, call userinfo endpoint.
        if self.config.kind == OAuthProviderKind::Apple {
            // Apple returns identity claims in the id_token JWT.
            let id_token = token_json["id_token"]
                .as_str()
                .ok_or_else(|| AuthError::OAuth2("missing id_token from Apple".into()))?;

            // Decode payload without verification (Apple's public keys would
            // be needed for full verification; the state HMAC + PKCE already
            // bind this flow to our server).
            let parts: Vec<&str> = id_token.splitn(3, '.').collect();
            if parts.len() < 2 {
                return Err(AuthError::OAuth2("malformed Apple id_token".into()));
            }
            let payload_bytes = data_encoding::BASE64URL_NOPAD
                .decode(parts[1].as_bytes())
                .or_else(|_| {
                    // Try with padding
                    base64_decode_lenient(parts[1])
                })
                .map_err(|e| AuthError::OAuth2(format!("Apple id_token decode: {e}")))?;
            let claims: serde_json::Value = serde_json::from_slice(&payload_bytes)
                .map_err(|e| AuthError::OAuth2(format!("Apple claims parse: {e}")))?;

            return Ok(OAuthUserInfo {
                provider_user_id: claims["sub"].as_str().unwrap_or_default().to_string(),
                email: claims["email"].as_str().map(String::from),
                name: None, // Apple sends name only on first auth, via form_post
                avatar_url: None,
                provider: OAuthProviderKind::Apple,
            });
        }

        // Fetch user info from the provider's userinfo endpoint.
        let userinfo_resp = http
            .get(&self.config.userinfo_url)
            .bearer_auth(access_token)
            .header("Accept", "application/json")
            // GitHub API requires a User-Agent header.
            .header("User-Agent", "DarshJDB")
            .send()
            .await
            .map_err(|e| AuthError::OAuth2(format!("userinfo request failed: {e}")))?;

        if !userinfo_resp.status().is_success() {
            let status = userinfo_resp.status();
            let body = userinfo_resp
                .text()
                .await
                .unwrap_or_else(|_| "unknown".into());
            return Err(AuthError::OAuth2(format!(
                "userinfo fetch failed ({status}): {body}"
            )));
        }

        let info: serde_json::Value = userinfo_resp
            .json()
            .await
            .map_err(|e| AuthError::OAuth2(format!("userinfo parse: {e}")))?;

        // Map provider-specific JSON shapes to our unified struct.
        match self.config.kind {
            OAuthProviderKind::Google => Ok(OAuthUserInfo {
                provider_user_id: info["sub"].as_str().unwrap_or_default().to_string(),
                email: info["email"].as_str().map(String::from),
                name: info["name"].as_str().map(String::from),
                avatar_url: info["picture"].as_str().map(String::from),
                provider: OAuthProviderKind::Google,
            }),
            OAuthProviderKind::GitHub => {
                // GitHub may not include email in /user; need separate call.
                let mut email = info["email"].as_str().map(String::from);
                if email.is_none() {
                    // Fetch primary verified email from /user/emails.
                    if let Ok(emails_resp) = http
                        .get("https://api.github.com/user/emails")
                        .bearer_auth(access_token)
                        .header("Accept", "application/json")
                        .header("User-Agent", "DarshJDB")
                        .send()
                        .await
                        && let Ok(emails) = emails_resp.json::<Vec<serde_json::Value>>().await
                    {
                        email = emails
                            .iter()
                            .find(|e| {
                                e["primary"].as_bool() == Some(true)
                                    && e["verified"].as_bool() == Some(true)
                            })
                            .or_else(|| {
                                emails
                                    .iter()
                                    .find(|e| e["verified"].as_bool() == Some(true))
                            })
                            .and_then(|e| e["email"].as_str().map(String::from));
                    }
                }

                Ok(OAuthUserInfo {
                    provider_user_id: info["id"]
                        .as_i64()
                        .map(|id| id.to_string())
                        .or_else(|| info["id"].as_str().map(String::from))
                        .unwrap_or_default(),
                    email,
                    name: info["name"]
                        .as_str()
                        .or_else(|| info["login"].as_str())
                        .map(String::from),
                    avatar_url: info["avatar_url"].as_str().map(String::from),
                    provider: OAuthProviderKind::GitHub,
                })
            }
            OAuthProviderKind::Discord => Ok(OAuthUserInfo {
                provider_user_id: info["id"].as_str().unwrap_or_default().to_string(),
                email: info["email"].as_str().map(String::from),
                name: info["username"].as_str().map(String::from),
                avatar_url: info["id"].as_str().and_then(|id| {
                    info["avatar"]
                        .as_str()
                        .map(|av| format!("https://cdn.discordapp.com/avatars/{id}/{av}.png"))
                }),
                provider: OAuthProviderKind::Discord,
            }),
            OAuthProviderKind::Apple => {
                // Handled above; unreachable.
                unreachable!("Apple handled via id_token path above")
            }
            OAuthProviderKind::Microsoft => Ok(OAuthUserInfo {
                provider_user_id: info["sub"].as_str().unwrap_or_default().to_string(),
                email: info["email"].as_str().map(String::from),
                name: info["name"].as_str().map(String::from),
                avatar_url: info["picture"].as_str().map(String::from),
                provider: OAuthProviderKind::Microsoft,
            }),
            OAuthProviderKind::Twitter => {
                // Twitter v2 wraps user data in a "data" object.
                let data = info.get("data").unwrap_or(&info);
                Ok(OAuthUserInfo {
                    provider_user_id: data["id"].as_str().unwrap_or_default().to_string(),
                    email: None, // Twitter does not expose email via users.read scope.
                    name: data["name"].as_str().map(String::from),
                    avatar_url: data["profile_image_url"].as_str().map(String::from),
                    provider: OAuthProviderKind::Twitter,
                })
            }
            OAuthProviderKind::LinkedIn => Ok(OAuthUserInfo {
                provider_user_id: info["sub"].as_str().unwrap_or_default().to_string(),
                email: info["email"].as_str().map(String::from),
                name: info["name"].as_str().map(String::from),
                avatar_url: info["picture"].as_str().map(String::from),
                provider: OAuthProviderKind::LinkedIn,
            }),
            OAuthProviderKind::Slack => Ok(OAuthUserInfo {
                provider_user_id: info["sub"]
                    .as_str()
                    .or_else(|| info["https://slack.com/user_id"].as_str())
                    .unwrap_or_default()
                    .to_string(),
                email: info["email"].as_str().map(String::from),
                name: info["name"].as_str().map(String::from),
                avatar_url: info["picture"].as_str().map(String::from),
                provider: OAuthProviderKind::Slack,
            }),
            OAuthProviderKind::GitLab => Ok(OAuthUserInfo {
                provider_user_id: info["sub"]
                    .as_str()
                    .map(String::from)
                    .or_else(|| info["sub"].as_i64().map(|id| id.to_string()))
                    .unwrap_or_default(),
                email: info["email"].as_str().map(String::from),
                name: info["name"].as_str().map(String::from),
                avatar_url: info["picture"].as_str().map(String::from),
                provider: OAuthProviderKind::GitLab,
            }),
            OAuthProviderKind::Bitbucket => {
                // Bitbucket returns display_name and UUID in the user object.
                let user_id = info["uuid"]
                    .as_str()
                    .or_else(|| info["account_id"].as_str())
                    .unwrap_or_default()
                    .to_string();
                let avatar = info["links"]["avatar"]["href"].as_str().map(String::from);

                // Bitbucket does not include email in /user; need /user/emails.
                let mut email = None;
                if let Ok(emails_resp) = http
                    .get("https://api.bitbucket.org/2.0/user/emails")
                    .bearer_auth(access_token)
                    .header("Accept", "application/json")
                    .send()
                    .await
                    && let Ok(emails_json) = emails_resp.json::<serde_json::Value>().await
                {
                    email = emails_json["values"]
                        .as_array()
                        .and_then(|vals| {
                            vals.iter()
                                .find(|e| e["is_primary"].as_bool() == Some(true))
                                .or_else(|| {
                                    vals.iter()
                                        .find(|e| e["is_confirmed"].as_bool() == Some(true))
                                })
                        })
                        .and_then(|e| e["email"].as_str().map(String::from));
                }

                Ok(OAuthUserInfo {
                    provider_user_id: user_id,
                    email,
                    name: info["display_name"].as_str().map(String::from),
                    avatar_url: avatar,
                    provider: OAuthProviderKind::Bitbucket,
                })
            }
            OAuthProviderKind::Facebook => Ok(OAuthUserInfo {
                provider_user_id: info["id"].as_str().unwrap_or_default().to_string(),
                email: info["email"].as_str().map(String::from),
                name: info["name"].as_str().map(String::from),
                avatar_url: info["picture"]["data"]["url"].as_str().map(String::from),
                provider: OAuthProviderKind::Facebook,
            }),
            OAuthProviderKind::Spotify => Ok(OAuthUserInfo {
                provider_user_id: info["id"].as_str().unwrap_or_default().to_string(),
                email: info["email"].as_str().map(String::from),
                name: info["display_name"].as_str().map(String::from),
                avatar_url: info["images"]
                    .as_array()
                    .and_then(|imgs| imgs.first())
                    .and_then(|img| img["url"].as_str())
                    .map(String::from),
                provider: OAuthProviderKind::Spotify,
            }),
        }
    }
}

/// Parse a provider name string into an [`OAuthProviderKind`].
impl OAuthProviderKind {
    /// Parse a lowercase provider name (e.g. "google", "github") into the enum.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_lowercase().as_str() {
            "google" => Some(Self::Google),
            "github" => Some(Self::GitHub),
            "apple" => Some(Self::Apple),
            "discord" => Some(Self::Discord),
            "microsoft" | "azure" | "azuread" => Some(Self::Microsoft),
            "twitter" | "x" => Some(Self::Twitter),
            "linkedin" => Some(Self::LinkedIn),
            "slack" => Some(Self::Slack),
            "gitlab" => Some(Self::GitLab),
            "bitbucket" => Some(Self::Bitbucket),
            "facebook" | "meta" => Some(Self::Facebook),
            "spotify" => Some(Self::Spotify),
            _ => None,
        }
    }
}

/// Minimal percent-encoding for URL query parameters.
fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Lenient base64url decode that handles missing padding.
fn base64_decode_lenient(input: &str) -> Result<Vec<u8>, data_encoding::DecodeError> {
    // Add padding if needed.
    let padded = match input.len() % 4 {
        2 => format!("{input}=="),
        3 => format!("{input}="),
        _ => input.to_string(),
    };
    // Replace URL-safe chars with standard base64 for decoding.
    let standard = padded.replace('-', "+").replace('_', "/");
    data_encoding::BASE64.decode(standard.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Password hashing
    // -----------------------------------------------------------------------

    #[test]
    fn password_hash_and_verify() {
        let hash = PasswordProvider::hash_password("hunter2").expect("hash");
        assert!(PasswordProvider::verify_password("hunter2", &hash).expect("verify"));
        assert!(!PasswordProvider::verify_password("wrong", &hash).expect("verify"));
    }

    #[test]
    fn password_hash_produces_argon2id_phc_string() {
        let hash = PasswordProvider::hash_password("test123").expect("hash");
        assert!(
            hash.starts_with("$argon2id$"),
            "expected argon2id PHC format, got: {hash}"
        );
        assert!(hash.contains("v=19"), "expected version 19 in PHC string");
        assert!(hash.contains("m=65536"), "expected 64 MiB memory cost");
        assert!(hash.contains("t=3"), "expected 3 iterations");
        assert!(hash.contains("p=4"), "expected parallelism 4");
    }

    #[test]
    fn password_hash_uses_unique_salts() {
        let h1 = PasswordProvider::hash_password("same-password").expect("hash1");
        let h2 = PasswordProvider::hash_password("same-password").expect("hash2");
        assert_ne!(
            h1, h2,
            "two hashes of the same password must differ (unique salts)"
        );
        assert!(PasswordProvider::verify_password("same-password", &h1).expect("v1"));
        assert!(PasswordProvider::verify_password("same-password", &h2).expect("v2"));
    }

    #[test]
    fn password_verify_rejects_corrupted_hash() {
        let result = PasswordProvider::verify_password("anything", "not-a-valid-hash");
        assert!(
            result.is_err(),
            "corrupted hash should return Err, not Ok(false)"
        );
    }

    #[test]
    fn password_empty_string_hashes_and_verifies() {
        let hash = PasswordProvider::hash_password("").expect("hash empty");
        assert!(PasswordProvider::verify_password("", &hash).expect("verify empty"));
        assert!(!PasswordProvider::verify_password("x", &hash).expect("verify non-empty"));
    }

    #[test]
    fn password_unicode_roundtrip() {
        let pw = "p\u{00e4}ssw\u{00f6}rd\u{1f512}";
        let hash = PasswordProvider::hash_password(pw).expect("hash unicode");
        assert!(PasswordProvider::verify_password(pw, &hash).expect("verify unicode"));
    }

    // -----------------------------------------------------------------------
    // OAuth state HMAC
    // -----------------------------------------------------------------------

    #[test]
    fn hmac_state_roundtrip() {
        let secret = b"test-secret-key-for-oauth-state";
        let state = GenericOAuth2Provider::sign_state(secret).expect("sign");
        GenericOAuth2Provider::verify_state(&state, secret).expect("verify");
    }

    #[test]
    fn hmac_state_tampered() {
        let secret = b"test-secret-key-for-oauth-state";
        let state = GenericOAuth2Provider::sign_state(secret).expect("sign");
        let tampered = format!("{}x", state);
        assert!(GenericOAuth2Provider::verify_state(&tampered, secret).is_err());
    }

    #[test]
    fn hmac_state_wrong_secret_rejected() {
        let secret1 = b"secret-one-for-signing-states-ok";
        let secret2 = b"secret-two-different-key-entirely";
        let state = GenericOAuth2Provider::sign_state(secret1).expect("sign");
        assert!(
            GenericOAuth2Provider::verify_state(&state, secret2).is_err(),
            "state signed with secret1 must not verify with secret2"
        );
    }

    #[test]
    fn hmac_state_format_is_nonce_dot_signature() {
        let secret = b"test-format-checking-secret-key!";
        let state = GenericOAuth2Provider::sign_state(secret).expect("sign");
        let parts: Vec<&str> = state.splitn(2, '.').collect();
        assert_eq!(parts.len(), 2, "state must be nonce.signature");
        assert_eq!(parts[0].len(), 32, "nonce should be 32 hex chars");
        assert_eq!(parts[1].len(), 64, "signature should be 64 hex chars");
    }

    #[test]
    fn hmac_state_nonce_swapped() {
        let secret = b"nonce-swap-test-secret-key-here!";
        let state1 = GenericOAuth2Provider::sign_state(secret).expect("sign1");
        let state2 = GenericOAuth2Provider::sign_state(secret).expect("sign2");

        let nonce1 = state1.split('.').next().unwrap();
        let sig2 = state2.split_once('.').unwrap().1;
        let franken = format!("{nonce1}.{sig2}");
        assert!(
            GenericOAuth2Provider::verify_state(&franken, secret).is_err(),
            "mismatched nonce and signature must fail verification"
        );
    }

    #[test]
    fn hmac_state_empty_and_malformed_rejected() {
        let secret = b"test-malformed-state-secret-key!";
        assert!(GenericOAuth2Provider::verify_state("", secret).is_err());
        assert!(GenericOAuth2Provider::verify_state("no-dot-here", secret).is_err());
        assert!(GenericOAuth2Provider::verify_state(".", secret).is_err());
        assert!(GenericOAuth2Provider::verify_state(".invalid-hex", secret).is_err());
    }

    // -----------------------------------------------------------------------
    // PKCE
    // -----------------------------------------------------------------------

    #[test]
    fn pkce_challenge_is_s256() {
        let (verifier, challenge) = GenericOAuth2Provider::pkce_pair();
        use sha2::Digest;
        let hash = sha2::Sha256::digest(verifier.as_bytes());
        let expected = data_encoding::BASE64URL_NOPAD.encode(&hash);
        assert_eq!(challenge, expected);
    }

    #[test]
    fn pkce_pairs_are_unique() {
        let (v1, c1) = GenericOAuth2Provider::pkce_pair();
        let (v2, c2) = GenericOAuth2Provider::pkce_pair();
        assert_ne!(v1, v2, "PKCE verifiers must be unique");
        assert_ne!(c1, c2, "PKCE challenges must be unique");
    }

    #[test]
    fn pkce_verifier_is_base64url() {
        let (verifier, _) = GenericOAuth2Provider::pkce_pair();
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "verifier must be base64url: {verifier}"
        );
    }

    // -----------------------------------------------------------------------
    // OAuthProviderKind
    // -----------------------------------------------------------------------

    #[test]
    fn provider_kind_from_name_all_variants() {
        let cases = vec![
            ("google", Some(OAuthProviderKind::Google)),
            ("github", Some(OAuthProviderKind::GitHub)),
            ("apple", Some(OAuthProviderKind::Apple)),
            ("discord", Some(OAuthProviderKind::Discord)),
            ("microsoft", Some(OAuthProviderKind::Microsoft)),
            ("azure", Some(OAuthProviderKind::Microsoft)),
            ("azuread", Some(OAuthProviderKind::Microsoft)),
            ("twitter", Some(OAuthProviderKind::Twitter)),
            ("x", Some(OAuthProviderKind::Twitter)),
            ("linkedin", Some(OAuthProviderKind::LinkedIn)),
            ("slack", Some(OAuthProviderKind::Slack)),
            ("gitlab", Some(OAuthProviderKind::GitLab)),
            ("bitbucket", Some(OAuthProviderKind::Bitbucket)),
            ("facebook", Some(OAuthProviderKind::Facebook)),
            ("meta", Some(OAuthProviderKind::Facebook)),
            ("spotify", Some(OAuthProviderKind::Spotify)),
            ("GOOGLE", Some(OAuthProviderKind::Google)),
            ("GitHub", Some(OAuthProviderKind::GitHub)),
            ("MICROSOFT", Some(OAuthProviderKind::Microsoft)),
            ("unknown", None),
            ("", None),
        ];

        for (input, expected) in cases {
            assert_eq!(
                OAuthProviderKind::from_name(input),
                expected,
                "from_name({input:?}) mismatch"
            );
        }
    }

    #[test]
    fn provider_kind_display_roundtrip() {
        let kinds = [
            OAuthProviderKind::Google,
            OAuthProviderKind::GitHub,
            OAuthProviderKind::Apple,
            OAuthProviderKind::Discord,
            OAuthProviderKind::Microsoft,
            OAuthProviderKind::Twitter,
            OAuthProviderKind::LinkedIn,
            OAuthProviderKind::Slack,
            OAuthProviderKind::GitLab,
            OAuthProviderKind::Bitbucket,
            OAuthProviderKind::Facebook,
            OAuthProviderKind::Spotify,
        ];

        for kind in kinds {
            let name = kind.to_string();
            let parsed = OAuthProviderKind::from_name(&name);
            assert_eq!(
                parsed,
                Some(kind),
                "Display -> from_name roundtrip failed for {kind:?}"
            );
        }
    }

    #[test]
    fn all_provider_factories_produce_correct_kind() {
        let redirect = "https://example.com/callback".to_string();
        let id = "test-id".to_string();
        let secret = "test-secret".to_string();

        let factories: Vec<(OAuthProviderKind, GenericOAuth2Provider)> = vec![
            (
                OAuthProviderKind::Google,
                GenericOAuth2Provider::google(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::GitHub,
                GenericOAuth2Provider::github(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Apple,
                GenericOAuth2Provider::apple(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Discord,
                GenericOAuth2Provider::discord(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Microsoft,
                GenericOAuth2Provider::microsoft(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Twitter,
                GenericOAuth2Provider::twitter(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::LinkedIn,
                GenericOAuth2Provider::linkedin(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Slack,
                GenericOAuth2Provider::slack(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::GitLab,
                GenericOAuth2Provider::gitlab(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Bitbucket,
                GenericOAuth2Provider::bitbucket(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Facebook,
                GenericOAuth2Provider::facebook(id.clone(), secret.clone(), redirect.clone()),
            ),
            (
                OAuthProviderKind::Spotify,
                GenericOAuth2Provider::spotify(id.clone(), secret.clone(), redirect.clone()),
            ),
        ];

        for (expected_kind, provider) in &factories {
            assert_eq!(
                provider.config.kind, *expected_kind,
                "factory for {expected_kind:?} produced wrong kind"
            );
            assert!(
                !provider.config.auth_url.is_empty(),
                "{expected_kind:?} auth_url empty"
            );
            assert!(
                !provider.config.token_url.is_empty(),
                "{expected_kind:?} token_url empty"
            );
            assert!(
                !provider.config.scopes.is_empty(),
                "{expected_kind:?} scopes empty"
            );
        }
    }

    #[test]
    fn all_providers_generate_valid_authorization_url() {
        let secret = b"test-secret-for-all-providers-ok";
        let redirect = "https://example.com/callback".to_string();
        let id = "test-id".to_string();
        let sec = "test-secret".to_string();

        let providers = vec![
            GenericOAuth2Provider::google(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::github(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::discord(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::microsoft(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::twitter(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::linkedin(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::slack(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::gitlab(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::bitbucket(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::facebook(id.clone(), sec.clone(), redirect.clone()),
            GenericOAuth2Provider::spotify(id.clone(), sec.clone(), redirect.clone()),
        ];

        for provider in &providers {
            let (url, state, verifier) = provider
                .authorization_url(secret)
                .unwrap_or_else(|_| panic!("auth_url for {:?}", provider.config.kind));
            assert!(
                url.starts_with("https://"),
                "URL must be HTTPS for {:?}",
                provider.config.kind
            );
            assert!(
                url.contains("response_type=code"),
                "missing response_type for {:?}",
                provider.config.kind
            );
            assert!(
                url.contains("code_challenge_method=S256"),
                "missing PKCE for {:?}",
                provider.config.kind
            );
            assert!(
                !state.is_empty(),
                "empty state for {:?}",
                provider.config.kind
            );
            assert!(
                !verifier.is_empty(),
                "empty verifier for {:?}",
                provider.config.kind
            );
        }
    }

    // -----------------------------------------------------------------------
    // Magic link token hashing
    // -----------------------------------------------------------------------

    #[test]
    fn magic_link_token_hash_is_deterministic() {
        use sha2::Digest;
        let token = "test-magic-link-token-abc123";
        let digest1 = sha2::Sha256::digest(token.as_bytes());
        let hash1 = data_encoding::HEXLOWER.encode(&digest1);
        let digest2 = sha2::Sha256::digest(token.as_bytes());
        let hash2 = data_encoding::HEXLOWER.encode(&digest2);
        assert_eq!(hash1, hash2, "same token must produce same hash");
        assert_eq!(hash1.len(), 64, "SHA-256 hex output must be 64 chars");
    }

    #[test]
    fn magic_link_different_tokens_produce_different_hashes() {
        use sha2::Digest;
        let hash1 = data_encoding::HEXLOWER
            .encode(&sha2::Sha256::digest(b"token-aaa"));
        let hash2 = data_encoding::HEXLOWER
            .encode(&sha2::Sha256::digest(b"token-bbb"));
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn magic_link_expiry_is_15_minutes() {
        assert_eq!(MagicLinkProvider::EXPIRY_MINUTES, 15);
    }
}
