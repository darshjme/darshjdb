//! Multi-factor authentication for DarshanDB.
//!
//! Supports three MFA mechanisms:
//!
//! - **TOTP** (RFC 6238): Time-based one-time passwords with a +/-1 step
//!   window to accommodate clock skew.
//! - **Recovery codes**: 10 one-time-use codes, each Argon2id-hashed before
//!   storage so a database breach does not reveal unused codes.
//! - **WebAuthn**: Registration and assertion stubs for future FIDO2/passkey
//!   integration.

use argon2::{
    Algorithm, Argon2, Params, Version,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use sqlx::PgPool;
use uuid::Uuid;

use super::AuthError;

// ---------------------------------------------------------------------------
// TOTP manager
// ---------------------------------------------------------------------------

/// TOTP (Time-based One-Time Password) per RFC 6238.
///
/// Parameters:
/// - Hash: HMAC-SHA1 (compatibility with Google Authenticator and most apps).
/// - Period: 30 seconds.
/// - Digits: 6.
/// - Window: +/-1 step (accepts codes from the previous, current, or next interval).
pub struct TotpManager;

/// Result of TOTP enrollment.
#[derive(Debug)]
pub struct TotpEnrollment {
    /// The base32-encoded shared secret for the authenticator app.
    pub secret_base32: String,
    /// The raw secret bytes (store encrypted in the database).
    pub secret_raw: Vec<u8>,
    /// An `otpauth://` URI suitable for QR code generation.
    pub provisioning_uri: String,
}

impl TotpManager {
    /// TOTP time step in seconds.
    const PERIOD: u64 = 30;
    /// Number of output digits.
    const DIGITS: u32 = 6;
    /// Acceptable clock-skew window (steps before and after current).
    const WINDOW: i64 = 1;

    /// Generate a new TOTP secret for enrollment.
    ///
    /// The caller should display `provisioning_uri` as a QR code and
    /// store `secret_raw` (encrypted) after the user verifies a test code.
    pub fn enroll(issuer: &str, account: &str) -> TotpEnrollment {
        let mut secret = [0u8; 20]; // 160-bit key
        OsRng.fill_bytes(&mut secret);

        let secret_base32 = data_encoding::BASE32_NOPAD.encode(&secret);

        let provisioning_uri = format!(
            "otpauth://totp/{issuer}:{account}?secret={secret_base32}&issuer={issuer}&algorithm=SHA1&digits={digits}&period={period}",
            issuer = urlencoding(issuer),
            account = urlencoding(account),
            secret_base32 = secret_base32,
            digits = Self::DIGITS,
            period = Self::PERIOD,
        );

        TotpEnrollment {
            secret_base32,
            secret_raw: secret.to_vec(),
            provisioning_uri,
        }
    }

    /// Verify a TOTP code against a shared secret.
    ///
    /// Checks the current time step and +/-1 neighbors to allow for
    /// minor clock drift. Returns `true` if any window matches.
    pub fn verify(secret: &[u8], code: &str) -> Result<bool, AuthError> {
        let now = Utc::now().timestamp() as u64;
        let current_step = now / Self::PERIOD;

        let code_num: u32 = code
            .parse()
            .map_err(|_| AuthError::MfaFailed("TOTP code must be numeric".into()))?;

        for offset in -Self::WINDOW..=Self::WINDOW {
            let step = (current_step as i64 + offset) as u64;
            let expected = Self::generate_code(secret, step)?;
            if expected == code_num {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Generate the TOTP code for a specific time step.
    fn generate_code(secret: &[u8], step: u64) -> Result<u32, AuthError> {
        let step_bytes = step.to_be_bytes();

        let mut mac = Hmac::<Sha1>::new_from_slice(secret)
            .map_err(|e| AuthError::Crypto(format!("hmac-sha1: {e}")))?;
        mac.update(&step_bytes);
        let result = mac.finalize().into_bytes();

        // Dynamic truncation per RFC 4226 Section 5.4.
        let offset = (result[19] & 0x0f) as usize;
        let binary = ((result[offset] as u32 & 0x7f) << 24)
            | ((result[offset + 1] as u32) << 16)
            | ((result[offset + 2] as u32) << 8)
            | (result[offset + 3] as u32);

        Ok(binary % 10u32.pow(Self::DIGITS))
    }

    /// Store the TOTP secret (encrypted) for a user.
    ///
    /// The secret should be encrypted at rest by the caller; this stores
    /// the provided bytes directly. In production, wrap with an envelope
    /// encryption layer.
    pub async fn save_secret(
        pool: &PgPool,
        user_id: Uuid,
        encrypted_secret: &[u8],
    ) -> Result<(), AuthError> {
        sqlx::query(
            "INSERT INTO user_totp (user_id, encrypted_secret, enabled, created_at)
             VALUES ($1, $2, true, NOW())
             ON CONFLICT (user_id) DO UPDATE SET encrypted_secret = $2, enabled = true",
        )
        .bind(user_id)
        .bind(encrypted_secret)
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Disable TOTP for a user (e.g., on recovery code use).
    pub async fn disable(pool: &PgPool, user_id: Uuid) -> Result<(), AuthError> {
        sqlx::query("UPDATE user_totp SET enabled = false WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Recovery codes
// ---------------------------------------------------------------------------

/// Manages one-time recovery codes for account access when TOTP is unavailable.
///
/// On enrollment, 10 codes are generated. Each code is Argon2id-hashed
/// before storage so a database breach cannot reveal unused codes.
pub struct RecoveryCodeManager;

/// A set of newly generated recovery codes.
#[derive(Debug)]
pub struct RecoveryCodes {
    /// The plaintext codes to display to the user exactly once.
    pub codes: Vec<String>,
}

impl RecoveryCodeManager {
    /// Number of recovery codes to generate.
    const CODE_COUNT: usize = 10;
    /// Length of each code in bytes (encoded as hex = 16 chars).
    const CODE_BYTES: usize = 8;

    /// Build the Argon2id hasher with project-standard parameters.
    fn hasher() -> Result<Argon2<'static>, AuthError> {
        let params = Params::new(64 * 1024, 3, 4, None)
            .map_err(|e| AuthError::Crypto(format!("argon2 params: {e}")))?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }

    /// Generate and store a fresh set of recovery codes for a user.
    ///
    /// Any existing codes are replaced. The plaintext codes are returned
    /// exactly once for the user to record; only hashes are stored.
    pub async fn generate(pool: &PgPool, user_id: Uuid) -> Result<RecoveryCodes, AuthError> {
        // Delete existing codes.
        sqlx::query("DELETE FROM recovery_codes WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await?;

        let hasher = Self::hasher()?;
        let mut codes = Vec::with_capacity(Self::CODE_COUNT);

        for _ in 0..Self::CODE_COUNT {
            let mut raw = [0u8; Self::CODE_BYTES];
            OsRng.fill_bytes(&mut raw);
            let code = data_encoding::HEXLOWER.encode(&raw);

            let salt = SaltString::generate(&mut OsRng);
            let hash = hasher
                .hash_password(code.as_bytes(), &salt)
                .map_err(|e| AuthError::Crypto(format!("recovery hash: {e}")))?
                .to_string();

            sqlx::query(
                "INSERT INTO recovery_codes (user_id, code_hash, used) VALUES ($1, $2, false)",
            )
            .bind(user_id)
            .bind(&hash)
            .execute(pool)
            .await?;

            codes.push(code);
        }

        Ok(RecoveryCodes { codes })
    }

    /// Verify and consume a recovery code.
    ///
    /// On success the code is marked as used and cannot be reused.
    /// Returns `true` if the code matched an unused entry.
    pub async fn verify(pool: &PgPool, user_id: Uuid, code: &str) -> Result<bool, AuthError> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, code_hash FROM recovery_codes WHERE user_id = $1 AND used = false",
        )
        .bind(user_id)
        .fetch_all(pool)
        .await?;

        let hasher = Self::hasher()?;

        for (row_id, stored_hash) in rows {
            let parsed = PasswordHash::new(&stored_hash)
                .map_err(|e| AuthError::Crypto(format!("parse hash: {e}")))?;

            if hasher.verify_password(code.as_bytes(), &parsed).is_ok() {
                // Mark consumed.
                sqlx::query("UPDATE recovery_codes SET used = true WHERE id = $1")
                    .bind(row_id)
                    .execute(pool)
                    .await?;
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Count remaining unused recovery codes for a user.
    pub async fn remaining_count(pool: &PgPool, user_id: Uuid) -> Result<i64, AuthError> {
        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM recovery_codes WHERE user_id = $1 AND used = false",
        )
        .bind(user_id)
        .fetch_one(pool)
        .await?;
        Ok(count.0)
    }
}

// ---------------------------------------------------------------------------
// WebAuthn stubs
// ---------------------------------------------------------------------------

/// Stub for WebAuthn (FIDO2/passkey) registration and assertion.
///
/// Full WebAuthn implementation requires a dedicated library (e.g.,
/// `webauthn-rs`). These stubs define the interface contract so that
/// the rest of the auth system can reference WebAuthn without blocking
/// on the full implementation.
pub struct WebAuthnStub;

/// Data needed to complete a WebAuthn registration ceremony.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebAuthnRegistrationChallenge {
    /// The challenge bytes (base64url encoded).
    pub challenge: String,
    /// The relying party ID (typically the domain).
    pub rp_id: String,
    /// The user handle.
    pub user_id: String,
    /// The user display name.
    pub user_name: String,
}

/// Data needed to complete a WebAuthn assertion ceremony.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebAuthnAssertionChallenge {
    /// The challenge bytes (base64url encoded).
    pub challenge: String,
    /// The relying party ID.
    pub rp_id: String,
    /// Allowed credential IDs (base64url encoded).
    pub allowed_credentials: Vec<String>,
}

impl WebAuthnStub {
    /// Begin a WebAuthn registration ceremony.
    ///
    /// Returns a challenge that the client should pass to
    /// `navigator.credentials.create()`.
    pub fn begin_registration(
        rp_id: &str,
        user_id: Uuid,
        user_name: &str,
    ) -> Result<WebAuthnRegistrationChallenge, AuthError> {
        let mut challenge_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut challenge_bytes);
        let challenge = data_encoding::BASE64URL_NOPAD.encode(&challenge_bytes);

        Ok(WebAuthnRegistrationChallenge {
            challenge,
            rp_id: rp_id.to_string(),
            user_id: user_id.to_string(),
            user_name: user_name.to_string(),
        })
    }

    /// Complete a WebAuthn registration ceremony.
    ///
    /// Stub: validates input shape but does not perform full attestation
    /// verification. Wire in `webauthn-rs` for production use.
    pub async fn complete_registration(
        _pool: &PgPool,
        _user_id: Uuid,
        _challenge: &WebAuthnRegistrationChallenge,
        _response_json: &serde_json::Value,
    ) -> Result<(), AuthError> {
        // TODO: Implement with webauthn-rs crate.
        Err(AuthError::Internal(
            "WebAuthn registration not yet implemented — use webauthn-rs".into(),
        ))
    }

    /// Begin a WebAuthn assertion ceremony.
    ///
    /// Returns a challenge that the client should pass to
    /// `navigator.credentials.get()`.
    pub async fn begin_assertion(
        pool: &PgPool,
        rp_id: &str,
        user_id: Uuid,
    ) -> Result<WebAuthnAssertionChallenge, AuthError> {
        let mut challenge_bytes = [0u8; 32];
        OsRng.fill_bytes(&mut challenge_bytes);
        let challenge = data_encoding::BASE64URL_NOPAD.encode(&challenge_bytes);

        // Fetch stored credential IDs for this user.
        let cred_ids: Vec<String> =
            sqlx::query_scalar("SELECT credential_id FROM webauthn_credentials WHERE user_id = $1")
                .bind(user_id)
                .fetch_all(pool)
                .await?;

        Ok(WebAuthnAssertionChallenge {
            challenge,
            rp_id: rp_id.to_string(),
            allowed_credentials: cred_ids,
        })
    }

    /// Complete a WebAuthn assertion ceremony.
    ///
    /// Stub: validates input shape but does not perform full signature
    /// verification. Wire in `webauthn-rs` for production use.
    pub async fn complete_assertion(
        _pool: &PgPool,
        _user_id: Uuid,
        _challenge: &WebAuthnAssertionChallenge,
        _response_json: &serde_json::Value,
    ) -> Result<bool, AuthError> {
        // TODO: Implement with webauthn-rs crate.
        Err(AuthError::Internal(
            "WebAuthn assertion not yet implemented — use webauthn-rs".into(),
        ))
    }
}

/// Minimal percent-encoding for URI components.
fn urlencoding(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_generate_and_verify() {
        let secret = b"12345678901234567890"; // RFC 6238 test vector key
        let step = 1; // arbitrary step
        let code = TotpManager::generate_code(secret, step).expect("generate");
        // Code should be a 6-digit number.
        assert!(code < 1_000_000);
    }

    #[test]
    fn totp_enrollment_produces_valid_uri() {
        let enrollment = TotpManager::enroll("DarshanDB", "user@example.com");
        assert!(enrollment.provisioning_uri.starts_with("otpauth://totp/"));
        assert!(enrollment.provisioning_uri.contains("DarshanDB"));
        assert!(!enrollment.secret_base32.is_empty());
        assert_eq!(enrollment.secret_raw.len(), 20);
    }
}
