// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// Magic link authentication provider: 32-byte tokens, SHA-256 hashed storage,
// 15-minute expiry, single-use semantics with atomic consumption, and
// pluggable email delivery (SMTP via lettre, HTTP via SendGrid, or dev log).

//! Passwordless "magic link" authentication for DarshJDB.
//!
//! # Flow
//!
//! 1. Caller (HTTP handler) resolves an email to a `user_id` and invokes
//!    [`MagicLinkProvider::generate`]. A 32-byte CSPRNG token is created,
//!    its SHA-256 hash is stored in `magic_link_tokens`, and the raw token
//!    plus a sign-in URL are returned.
//! 2. The handler calls [`MagicLinkProvider::send_email`] to deliver the
//!    URL. Transport is selected at runtime from env vars (`SMTP_HOST`,
//!    `SENDGRID_API_KEY`) or logs the link in dev mode.
//! 3. When the user clicks, the callback handler invokes
//!    [`MagicLinkProvider::verify`] with the raw token. The token is
//!    re-hashed, looked up, checked for expiry and prior consumption, and
//!    atomically marked used. On success the owning `user_id` is returned.
//!
//! # Security properties
//!
//! - **Only hashes are stored.** A database breach does not yield usable
//!   tokens (SHA-256 is applied to the base64url-encoded random bytes).
//! - **Single use.** The `UPDATE ... WHERE used_at IS NULL` with
//!   `rows_affected == 1` check enforces atomic consumption even under
//!   concurrent verification attempts.
//! - **Expiry.** Tokens are invalid 15 minutes after issuance.
//! - **IP audit.** Generation IP is recorded for forensic correlation.

use chrono::{DateTime, Utc};
use data_encoding::{BASE64URL_NOPAD, HEXLOWER};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use super::AuthError;

/// One-time magic link authentication provider.
///
/// Stateless; all persistence happens in the `magic_link_tokens` table.
pub struct MagicLinkProvider;

/// A newly generated magic link ready to be delivered to a user.
#[derive(Debug, Clone)]
pub struct GeneratedMagicLink {
    /// The raw (un-hashed) token, safe to embed in the sign-in URL.
    pub token: String,
    /// Fully-qualified sign-in URL (base + `?token=...`).
    pub url: String,
    /// When the underlying DB row expires.
    pub expires_at: DateTime<Utc>,
}

impl MagicLinkProvider {
    /// Token validity window in minutes. Matches the default in the
    /// `magic_link_tokens` migration.
    pub const EXPIRY_MINUTES: i64 = 15;

    /// Compute the SHA-256 hex digest used as the stored `token_hash`.
    ///
    /// This is deterministic; the same token always yields the same hash,
    /// which is the lookup key in [`Self::verify`].
    fn hash_token(token: &str) -> String {
        let digest = Sha256::digest(token.as_bytes());
        HEXLOWER.encode(&digest)
    }

    /// Build the sign-in URL from the configured base and a raw token.
    ///
    /// `DDB_MAGIC_LINK_BASE_URL` defaults to
    /// `http://localhost:8080/auth/verify` which matches the dev-mode
    /// REST handler.
    fn build_url(token: &str) -> String {
        let base = std::env::var("DDB_MAGIC_LINK_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8080/auth/verify".to_string());
        if base.contains('?') {
            format!("{base}&token={token}")
        } else {
            format!("{base}?token={token}")
        }
    }

    /// Generate a new magic link for `user_id`.
    ///
    /// Returns the raw token and the full sign-in URL. Only the SHA-256
    /// hash is persisted. `ip` is recorded for audit purposes and may be
    /// any string parseable as an IP (use an empty string to skip).
    pub async fn generate(
        pool: &PgPool,
        email: &str,
        user_id: Uuid,
        ip: &str,
    ) -> Result<GeneratedMagicLink, AuthError> {
        // 32 random bytes → base64url → SHA-256 hex.
        let mut raw = [0u8; 32];
        OsRng.fill_bytes(&mut raw);
        let token = BASE64URL_NOPAD.encode(&raw);
        let token_hash = Self::hash_token(&token);

        let expires_at = Utc::now() + chrono::Duration::minutes(Self::EXPIRY_MINUTES);
        let url = Self::build_url(&token);

        // Validate the IP string before handing it to Postgres so the
        // server-side `::inet` cast cannot fail. Invalid/empty → NULL.
        let ip_text: Option<String> = if ip.is_empty() {
            None
        } else {
            ip.parse::<std::net::IpAddr>().ok().map(|a| a.to_string())
        };

        sqlx::query(
            "INSERT INTO magic_link_tokens \
             (user_id, token_hash, email, expires_at, ip_address) \
             VALUES ($1, $2, $3, $4, $5::inet)",
        )
        .bind(user_id)
        .bind(&token_hash)
        .bind(email)
        .bind(expires_at)
        .bind(ip_text)
        .execute(pool)
        .await?;

        Ok(GeneratedMagicLink {
            token,
            url,
            expires_at,
        })
    }

    /// Deliver `link` to `to` by whatever transport is configured.
    ///
    /// Resolution order:
    /// 1. `SMTP_HOST` set → use `lettre` SMTP transport. Requires
    ///    `SMTP_USERNAME`, `SMTP_PASSWORD`, and `SMTP_FROM`.
    /// 2. `SENDGRID_API_KEY` set → POST to the SendGrid v3 mail API,
    ///    using `SENDGRID_FROM` as the sender.
    /// 3. Neither set → `tracing::warn!` the link (dev mode).
    pub async fn send_email(to: &str, link: &str) -> Result<(), AuthError> {
        if let Ok(host) = std::env::var("SMTP_HOST") {
            Self::send_via_smtp(&host, to, link).await
        } else if let Ok(key) = std::env::var("SENDGRID_API_KEY") {
            Self::send_via_sendgrid(&key, to, link).await
        } else {
            tracing::warn!(
                target: "ddb::auth::magic_link",
                %to,
                %link,
                "DEV MODE: magic link (no SMTP_HOST or SENDGRID_API_KEY set)"
            );
            Ok(())
        }
    }

    /// SMTP delivery via `lettre`. Uses STARTTLS on the configured port
    /// (default 587).
    async fn send_via_smtp(host: &str, to: &str, link: &str) -> Result<(), AuthError> {
        use lettre::message::header::ContentType;
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

        let user = std::env::var("SMTP_USERNAME")
            .map_err(|_| AuthError::Internal("SMTP_USERNAME not set".into()))?;
        let pass = std::env::var("SMTP_PASSWORD")
            .map_err(|_| AuthError::Internal("SMTP_PASSWORD not set".into()))?;
        let from = std::env::var("SMTP_FROM")
            .map_err(|_| AuthError::Internal("SMTP_FROM not set".into()))?;
        let port: u16 = std::env::var("SMTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(587);

        let body = format!(
            "Click the link below to sign in to DarshJDB. This link expires \
             in {} minutes and can only be used once.\n\n{}\n",
            Self::EXPIRY_MINUTES,
            link
        );

        let email = Message::builder()
            .from(
                from.parse()
                    .map_err(|e| AuthError::Internal(format!("bad SMTP_FROM: {e}")))?,
            )
            .to(to
                .parse()
                .map_err(|e| AuthError::Internal(format!("bad recipient: {e}")))?)
            .subject("Your DarshJDB sign-in link")
            .header(ContentType::TEXT_PLAIN)
            .body(body)
            .map_err(|e| AuthError::Internal(format!("build email: {e}")))?;

        let creds = Credentials::new(user, pass);
        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
                .map_err(|e| AuthError::Internal(format!("smtp relay: {e}")))?
                .port(port)
                .credentials(creds)
                .build();

        mailer
            .send(email)
            .await
            .map_err(|e| AuthError::Internal(format!("smtp send: {e}")))?;
        Ok(())
    }

    /// SendGrid v3 mail API delivery via `reqwest`.
    async fn send_via_sendgrid(key: &str, to: &str, link: &str) -> Result<(), AuthError> {
        let from = std::env::var("SENDGRID_FROM")
            .map_err(|_| AuthError::Internal("SENDGRID_FROM not set".into()))?;

        let payload = serde_json::json!({
            "personalizations": [{ "to": [{ "email": to }] }],
            "from": { "email": from },
            "subject": "Your DarshJDB sign-in link",
            "content": [{
                "type": "text/plain",
                "value": format!(
                    "Click the link below to sign in to DarshJDB. This link \
                     expires in {} minutes and can only be used once.\n\n{}\n",
                    Self::EXPIRY_MINUTES,
                    link
                ),
            }]
        });

        let resp = reqwest::Client::new()
            .post("https://api.sendgrid.com/v3/mail/send")
            .bearer_auth(key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| AuthError::Internal(format!("sendgrid request: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AuthError::Internal(format!(
                "sendgrid error {status}: {body}"
            )));
        }
        Ok(())
    }

    /// Verify a raw magic link token and return the owning `user_id`.
    ///
    /// Errors:
    /// - [`AuthError::TokenInvalid`] — unknown token or expired.
    /// - [`AuthError::TokenAlreadyUsed`] — previously consumed (also the
    ///   outcome of a losing race between concurrent verifications).
    pub async fn verify(pool: &PgPool, raw_token: &str) -> Result<Uuid, AuthError> {
        if raw_token.is_empty() {
            return Err(AuthError::TokenInvalid("empty token".into()));
        }
        let token_hash = Self::hash_token(raw_token);

        // Fetch the row. Missing row = unknown or expired+cleaned token.
        let row: Option<(Uuid, DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT user_id, expires_at, used_at \
             FROM magic_link_tokens WHERE token_hash = $1",
        )
        .bind(&token_hash)
        .fetch_optional(pool)
        .await?;

        let (user_id, expires_at, used_at) = row
            .ok_or_else(|| AuthError::TokenInvalid("unknown magic link token".into()))?;

        if used_at.is_some() {
            return Err(AuthError::TokenAlreadyUsed);
        }
        if Utc::now() > expires_at {
            return Err(AuthError::TokenInvalid("magic link token expired".into()));
        }

        // Atomically mark used. The partial index on (token_hash) WHERE
        // used_at IS NULL makes this a single-row O(1) update.
        let result = sqlx::query(
            "UPDATE magic_link_tokens SET used_at = now() \
             WHERE token_hash = $1 AND used_at IS NULL",
        )
        .bind(&token_hash)
        .execute(pool)
        .await?;

        if result.rows_affected() == 0 {
            // Another concurrent verify consumed it in the window between
            // our SELECT and UPDATE.
            return Err(AuthError::TokenAlreadyUsed);
        }

        Ok(user_id)
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------
//
// Pure-logic tests run unconditionally. DB-backed tests require a
// `DATABASE_URL` pointing at a Postgres instance with the `users` table and
// the `magic_link_tokens` migration already applied; they return early
// otherwise (same pattern as `tests/integration.rs`).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_64_hex_chars() {
        let h1 = MagicLinkProvider::hash_token("abc-xyz-123");
        let h2 = MagicLinkProvider::hash_token("abc-xyz-123");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn different_tokens_hash_differently() {
        let h1 = MagicLinkProvider::hash_token("token-a");
        let h2 = MagicLinkProvider::hash_token("token-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn expiry_constant_is_15_minutes() {
        assert_eq!(MagicLinkProvider::EXPIRY_MINUTES, 15);
    }

    /// Single test for both URL-building cases so we don't race against
    /// other tests on the `DDB_MAGIC_LINK_BASE_URL` process env.
    #[test]
    fn build_url_cases() {
        // SAFETY: mutating the process env is only unsafe in the presence
        // of other threads reading it concurrently; by merging the two
        // cases into one `#[test]` we serialize access ourselves.
        unsafe {
            std::env::set_var(
                "DDB_MAGIC_LINK_BASE_URL",
                "https://db.darshj.me/auth/verify",
            );
        }
        let url = MagicLinkProvider::build_url("tok123");
        assert_eq!(url, "https://db.darshj.me/auth/verify?token=tok123");

        unsafe {
            std::env::set_var(
                "DDB_MAGIC_LINK_BASE_URL",
                "https://db.darshj.me/auth/verify?redirect=/home",
            );
        }
        let url = MagicLinkProvider::build_url("tok456");
        assert_eq!(
            url,
            "https://db.darshj.me/auth/verify?redirect=/home&token=tok456"
        );

        unsafe {
            std::env::remove_var("DDB_MAGIC_LINK_BASE_URL");
        }
    }

    // -----------------------------------------------------------------------
    // DB-backed tests (skipped when DATABASE_URL is not set).
    // -----------------------------------------------------------------------

    async fn setup() -> Option<(PgPool, Uuid, String)> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url).await.ok()?;

        // Best-effort ensure the target tables exist.
        crate::api::rest::ensure_auth_schema(&pool).await.ok()?;

        // Seed a user row.
        let user_id = Uuid::new_v4();
        let email = format!("mlink-{user_id}@darshjdb.test");
        let hash = crate::auth::PasswordProvider::hash_password("unused").ok()?;
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, roles) \
             VALUES ($1, $2, $3, $4::jsonb)",
        )
        .bind(user_id)
        .bind(&email)
        .bind(&hash)
        .bind(serde_json::json!(["user"]))
        .execute(&pool)
        .await
        .ok()?;

        Some((pool, user_id, email))
    }

    async fn teardown(pool: &PgPool, user_id: Uuid) {
        let _ = sqlx::query("DELETE FROM magic_link_tokens WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
    }

    #[tokio::test]
    async fn magic_link_generate_verify_happy_path() {
        let Some((pool, user_id, email)) = setup().await else {
            return;
        };

        let link = MagicLinkProvider::generate(&pool, &email, user_id, "127.0.0.1")
            .await
            .expect("generate");
        assert!(!link.token.is_empty());
        assert!(link.url.contains(&link.token));
        assert!(link.expires_at > Utc::now());

        let verified = MagicLinkProvider::verify(&pool, &link.token)
            .await
            .expect("verify");
        assert_eq!(verified, user_id);

        teardown(&pool, user_id).await;
    }

    #[tokio::test]
    async fn magic_link_reused_token_errors() {
        let Some((pool, user_id, email)) = setup().await else {
            return;
        };

        let link = MagicLinkProvider::generate(&pool, &email, user_id, "")
            .await
            .expect("generate");
        let _ = MagicLinkProvider::verify(&pool, &link.token)
            .await
            .expect("first verify");

        let err = MagicLinkProvider::verify(&pool, &link.token)
            .await
            .expect_err("second verify must fail");
        assert!(matches!(err, AuthError::TokenAlreadyUsed));

        teardown(&pool, user_id).await;
    }

    #[tokio::test]
    async fn magic_link_invalid_token_errors() {
        let Some((pool, user_id, _email)) = setup().await else {
            return;
        };

        let err = MagicLinkProvider::verify(&pool, "totally-bogus-token-not-in-db")
            .await
            .expect_err("unknown token must fail");
        assert!(matches!(err, AuthError::TokenInvalid(_)));

        teardown(&pool, user_id).await;
    }

    #[tokio::test]
    async fn magic_link_expired_token_errors() {
        let Some((pool, user_id, email)) = setup().await else {
            return;
        };

        let link = MagicLinkProvider::generate(&pool, &email, user_id, "")
            .await
            .expect("generate");

        // Force the row to be expired in the past.
        sqlx::query(
            "UPDATE magic_link_tokens SET expires_at = now() - INTERVAL '1 minute' \
             WHERE token_hash = $1",
        )
        .bind(MagicLinkProvider::hash_token(&link.token))
        .execute(&pool)
        .await
        .expect("force expiry");

        let err = MagicLinkProvider::verify(&pool, &link.token)
            .await
            .expect_err("expired token must fail");
        assert!(
            matches!(err, AuthError::TokenInvalid(ref m) if m.contains("expired")),
            "got: {err:?}"
        );

        teardown(&pool, user_id).await;
    }
}
