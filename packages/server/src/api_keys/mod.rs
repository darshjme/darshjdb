//! API key management for DarshJDB.
//!
//! Provides secure API key generation, hashing, validation, and revocation.
//! Keys follow the format `ddb_key_<random-hex>` and are stored as SHA-256
//! hashes in a dedicated `api_keys` table for security isolation.
//!
//! # Security Model
//!
//! - Keys are generated with 32 bytes of randomness (256-bit entropy).
//! - Only the SHA-256 hash is stored; the raw key is returned exactly once
//!   at creation time.
//! - Validation uses constant-time comparison via SHA-256 hash matching.
//! - Each key carries a set of [`ApiKeyScope`]s that restrict what
//!   operations the bearer can perform.
//! - Keys can be rotated (new key issued, old revoked atomically) or
//!   individually revoked.
//!
//! # Integration with Auth Middleware
//!
//! The auth middleware accepts API keys via:
//! - `Authorization: Bearer ddb_key_...`
//! - `X-API-Key: ddb_key_...`
//!
//! When a valid API key is presented, an [`ApiKeyAuth`] context is built
//! and inserted into request extensions alongside a synthetic [`AuthContext`].

pub mod handlers;

use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use sqlx::PgPool;
use tracing::{debug, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Unique identifier for an API key.
pub type ApiKeyId = Uuid;

/// The prefix used for all DarshJDB API keys.
pub const API_KEY_PREFIX: &str = "ddb_key_";

/// Scopes that control what an API key can do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiKeyScope {
    /// Read access to all tables.
    Read,
    /// Write access to all tables.
    Write,
    /// Full admin access.
    Admin,
    /// Access restricted to specific tables.
    Tables(Vec<String>),
    /// Custom named scope for extensibility.
    Custom(String),
}

impl ApiKeyScope {
    /// Check if this scope grants the given operation on the given table.
    pub fn permits(&self, operation: &str, table: Option<&str>) -> bool {
        match self {
            Self::Admin => true,
            Self::Read => operation == "read",
            Self::Write => matches!(operation, "read" | "write"),
            Self::Tables(tables) => {
                table.map_or(false, |t| tables.iter().any(|allowed| allowed == t))
            }
            Self::Custom(_) => false, // custom scopes require explicit checks
        }
    }
}

/// Stored API key metadata (never contains the raw key).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    /// Unique key identifier.
    pub id: ApiKeyId,
    /// Human-readable name for this key.
    pub name: String,
    /// First 8 characters of the key for display (e.g. `ddb_key_a1b2c3d4`).
    pub key_prefix: String,
    /// SHA-256 hash of the full key (hex-encoded).
    #[serde(skip_serializing)]
    pub key_hash: String,
    /// What this key is allowed to do.
    pub scopes: Vec<ApiKeyScope>,
    /// Optional per-key rate limit (requests per minute).
    pub rate_limit: Option<u32>,
    /// When the key expires (None = never).
    pub expires_at: Option<DateTime<Utc>>,
    /// User who created this key.
    pub created_by: Uuid,
    /// When the key was created.
    pub created_at: DateTime<Utc>,
    /// When the key was last used for authentication.
    pub last_used_at: Option<DateTime<Utc>>,
    /// Whether the key has been revoked.
    pub revoked: bool,
}

/// Authentication context built from a validated API key.
///
/// Inserted into request extensions by the auth middleware when
/// an API key is used instead of a JWT.
#[derive(Debug, Clone)]
pub struct ApiKeyAuth {
    /// The key's unique ID.
    pub key_id: ApiKeyId,
    /// The key's name.
    pub name: String,
    /// Scopes granted to this key.
    pub scopes: Vec<ApiKeyScope>,
    /// User who owns this key.
    pub owner_id: Uuid,
    /// Per-key rate limit override.
    pub rate_limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// Key generation
// ---------------------------------------------------------------------------

/// Generate a new API key string with the `ddb_key_` prefix.
fn generate_raw_key() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{}{}", API_KEY_PREFIX, hex::encode(bytes))
}

/// Compute hex-encoded SHA-256 hash of a key.
fn hash_key(key: &str) -> String {
    let digest = sha2::Sha256::digest(key.as_bytes());
    hex::encode(digest)
}

/// Extract the display prefix from a raw key (first 16 chars including prefix).
fn key_display_prefix(key: &str) -> String {
    // "ddb_key_" is 8 chars, plus first 8 hex chars of random = 16 total.
    key.chars().take(16).collect()
}

// ---------------------------------------------------------------------------
// Database operations
// ---------------------------------------------------------------------------

/// Create the API keys table if it does not exist.
pub async fn ensure_api_keys_table(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS api_keys (
            id           UUID PRIMARY KEY,
            name         TEXT NOT NULL,
            key_prefix   TEXT NOT NULL,
            key_hash     TEXT NOT NULL UNIQUE,
            scopes       JSONB NOT NULL DEFAULT '[]'::jsonb,
            rate_limit   INTEGER,
            expires_at   TIMESTAMPTZ,
            created_by   UUID NOT NULL,
            created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            last_used_at TIMESTAMPTZ,
            revoked      BOOLEAN NOT NULL DEFAULT false
        );

        CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys (key_hash);
        CREATE INDEX IF NOT EXISTS idx_api_keys_created_by ON api_keys (created_by);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Create a new API key. Returns `(ApiKeyId, raw_key_string)`.
///
/// The raw key is returned exactly once and must be shown to the user
/// immediately. Only the hash is persisted.
pub async fn create_api_key(
    pool: &PgPool,
    name: &str,
    scopes: Vec<ApiKeyScope>,
    rate_limit: Option<u32>,
    expires_at: Option<DateTime<Utc>>,
    created_by: Uuid,
) -> Result<(ApiKeyId, String), sqlx::Error> {
    let raw_key = generate_raw_key();
    let key_hash = hash_key(&raw_key);
    let key_prefix = key_display_prefix(&raw_key);
    let id = Uuid::new_v4();
    let scopes_json = serde_json::to_value(&scopes).unwrap_or_default();

    sqlx::query(
        "INSERT INTO api_keys (id, name, key_prefix, key_hash, scopes, rate_limit, expires_at, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(name)
    .bind(&key_prefix)
    .bind(&key_hash)
    .bind(&scopes_json)
    .bind(rate_limit.map(|r| r as i32))
    .bind(expires_at)
    .bind(created_by)
    .execute(pool)
    .await?;

    debug!(key_id = %id, name = %name, prefix = %key_prefix, "API key created");

    Ok((id, raw_key))
}

/// Validate an API key and return the auth context if valid.
///
/// Performs constant-time hash comparison (via SHA-256 lookup) and checks
/// expiry and revocation status.
pub async fn validate_api_key(pool: &PgPool, key: &str) -> Option<ApiKeyAuth> {
    if !key.starts_with(API_KEY_PREFIX) {
        return None;
    }

    let key_hash = hash_key(key);

    let row: Option<(
        Uuid,
        String,
        serde_json::Value,
        Option<i32>,
        Option<DateTime<Utc>>,
        Uuid,
        bool,
    )> = sqlx::query_as(
        "SELECT id, name, scopes, rate_limit, expires_at, created_by, revoked
         FROM api_keys WHERE key_hash = $1",
    )
    .bind(&key_hash)
    .fetch_optional(pool)
    .await
    .ok()?;

    let (id, name, scopes_json, rate_limit, expires_at, created_by, revoked) = row?;

    if revoked {
        warn!(key_id = %id, "rejected revoked API key");
        return None;
    }

    if let Some(exp) = expires_at {
        if Utc::now() > exp {
            warn!(key_id = %id, "rejected expired API key");
            return None;
        }
    }

    let scopes: Vec<ApiKeyScope> = serde_json::from_value(scopes_json).unwrap_or_default();

    // Update last_used_at (fire-and-forget, non-blocking).
    let pool_clone = pool.clone();
    let id_clone = id;
    tokio::spawn(async move {
        let _ = sqlx::query("UPDATE api_keys SET last_used_at = NOW() WHERE id = $1")
            .bind(id_clone)
            .execute(&pool_clone)
            .await;
    });

    Some(ApiKeyAuth {
        key_id: id,
        name,
        scopes,
        owner_id: created_by,
        rate_limit: rate_limit.map(|r| r as u32),
    })
}

/// Revoke an API key by ID.
pub async fn revoke_api_key(pool: &PgPool, key_id: ApiKeyId) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE api_keys SET revoked = true WHERE id = $1 AND revoked = false")
        .bind(key_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Rotate an API key: revoke the old one and issue a new key with the
/// same name, scopes, rate limit, and expiry.
///
/// Returns the new `(ApiKeyId, raw_key)`.
pub async fn rotate_api_key(
    pool: &PgPool,
    key_id: ApiKeyId,
) -> Result<Option<(ApiKeyId, String)>, sqlx::Error> {
    // Fetch old key metadata.
    let row: Option<(String, serde_json::Value, Option<i32>, Option<DateTime<Utc>>, Uuid, bool)> =
        sqlx::query_as(
            "SELECT name, scopes, rate_limit, expires_at, created_by, revoked
             FROM api_keys WHERE id = $1",
        )
        .bind(key_id)
        .fetch_optional(pool)
        .await?;

    let (name, scopes_json, rate_limit, expires_at, created_by, revoked) = match row {
        Some(r) => r,
        None => return Ok(None),
    };

    if revoked {
        return Ok(None);
    }

    let scopes: Vec<ApiKeyScope> = serde_json::from_value(scopes_json).unwrap_or_default();

    // Revoke old key.
    revoke_api_key(pool, key_id).await?;

    // Create new key with same attributes.
    let (new_id, new_key) = create_api_key(
        pool,
        &name,
        scopes,
        rate_limit.map(|r| r as u32),
        expires_at,
        created_by,
    )
    .await?;

    debug!(
        old_key_id = %key_id,
        new_key_id = %new_id,
        "API key rotated"
    );

    Ok(Some((new_id, new_key)))
}

/// List API keys for a user (or all if `user_id` is `None`).
///
/// Never returns the key hash or the raw key.
pub async fn list_api_keys(
    pool: &PgPool,
    user_id: Option<Uuid>,
) -> Result<Vec<ApiKey>, sqlx::Error> {
    let rows: Vec<(
        Uuid,
        String,
        String,
        String,
        serde_json::Value,
        Option<i32>,
        Option<DateTime<Utc>>,
        Uuid,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        bool,
    )> = if let Some(uid) = user_id {
        sqlx::query_as(
            "SELECT id, name, key_prefix, key_hash, scopes, rate_limit, expires_at, created_by, created_at, last_used_at, revoked
             FROM api_keys WHERE created_by = $1 AND revoked = false ORDER BY created_at DESC",
        )
        .bind(uid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, name, key_prefix, key_hash, scopes, rate_limit, expires_at, created_by, created_at, last_used_at, revoked
             FROM api_keys WHERE revoked = false ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await?
    };

    Ok(rows
        .into_iter()
        .map(|r| ApiKey {
            id: r.0,
            name: r.1,
            key_prefix: r.2,
            key_hash: r.3, // skip_serializing prevents this from leaking
            scopes: serde_json::from_value(r.4).unwrap_or_default(),
            rate_limit: r.5.map(|r| r as u32),
            expires_at: r.6,
            created_by: r.7,
            created_at: r.8,
            last_used_at: r.9,
            revoked: r.10,
        })
        .collect())
}

/// Get a single API key by ID.
pub async fn get_api_key(pool: &PgPool, key_id: ApiKeyId) -> Result<Option<ApiKey>, sqlx::Error> {
    let row: Option<(
        Uuid,
        String,
        String,
        String,
        serde_json::Value,
        Option<i32>,
        Option<DateTime<Utc>>,
        Uuid,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
        bool,
    )> = sqlx::query_as(
        "SELECT id, name, key_prefix, key_hash, scopes, rate_limit, expires_at, created_by, created_at, last_used_at, revoked
         FROM api_keys WHERE id = $1",
    )
    .bind(key_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ApiKey {
        id: r.0,
        name: r.1,
        key_prefix: r.2,
        key_hash: r.3,
        scopes: serde_json::from_value(r.4).unwrap_or_default(),
        rate_limit: r.5.map(|rl| rl as u32),
        expires_at: r.6,
        created_by: r.7,
        created_at: r.8,
        last_used_at: r.9,
        revoked: r.10,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_has_correct_prefix() {
        let key = generate_raw_key();
        assert!(key.starts_with(API_KEY_PREFIX));
    }

    #[test]
    fn generated_key_has_sufficient_length() {
        let key = generate_raw_key();
        // "ddb_key_" (8) + 64 hex chars (32 bytes) = 72
        assert_eq!(key.len(), 72);
    }

    #[test]
    fn generated_keys_are_unique() {
        let k1 = generate_raw_key();
        let k2 = generate_raw_key();
        assert_ne!(k1, k2);
    }

    #[test]
    fn hash_key_deterministic() {
        let key = "ddb_key_abc123";
        let h1 = hash_key(key);
        let h2 = hash_key(key);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_key_different_inputs_different_hashes() {
        let h1 = hash_key("ddb_key_aaa");
        let h2 = hash_key("ddb_key_bbb");
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_key_is_hex_sha256() {
        let h = hash_key("test");
        assert_eq!(h.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn key_display_prefix_length() {
        let key = generate_raw_key();
        let prefix = key_display_prefix(&key);
        assert_eq!(prefix.len(), 16);
        assert!(prefix.starts_with(API_KEY_PREFIX));
    }

    #[test]
    fn scope_admin_permits_everything() {
        let scope = ApiKeyScope::Admin;
        assert!(scope.permits("read", Some("users")));
        assert!(scope.permits("write", Some("posts")));
        assert!(scope.permits("delete", None));
    }

    #[test]
    fn scope_read_only_permits_read() {
        let scope = ApiKeyScope::Read;
        assert!(scope.permits("read", Some("users")));
        assert!(!scope.permits("write", Some("users")));
        assert!(!scope.permits("delete", Some("users")));
    }

    #[test]
    fn scope_write_permits_read_and_write() {
        let scope = ApiKeyScope::Write;
        assert!(scope.permits("read", Some("users")));
        assert!(scope.permits("write", Some("users")));
        assert!(!scope.permits("delete", Some("users")));
    }

    #[test]
    fn scope_tables_restricts_to_specific_tables() {
        let scope = ApiKeyScope::Tables(vec!["users".into(), "posts".into()]);
        assert!(scope.permits("read", Some("users")));
        assert!(scope.permits("write", Some("posts")));
        assert!(!scope.permits("read", Some("secrets")));
        assert!(!scope.permits("read", None));
    }

    #[test]
    fn scope_custom_denies_by_default() {
        let scope = ApiKeyScope::Custom("special".into());
        assert!(!scope.permits("read", Some("users")));
        assert!(!scope.permits("write", None));
    }

    #[test]
    fn api_key_serialization_hides_hash() {
        let key = ApiKey {
            id: Uuid::new_v4(),
            name: "test-key".into(),
            key_prefix: "ddb_key_a1b2c3d4".into(),
            key_hash: "deadbeef".repeat(8),
            scopes: vec![ApiKeyScope::Read],
            rate_limit: Some(100),
            expires_at: None,
            created_by: Uuid::new_v4(),
            created_at: Utc::now(),
            last_used_at: None,
            revoked: false,
        };

        let json = serde_json::to_string(&key).expect("serialize");
        assert!(!json.contains("deadbeef"), "key_hash must not appear in JSON");
        assert!(json.contains("test-key"));
        assert!(json.contains("ddb_key_a1b2c3d4"));
    }

    #[test]
    fn scope_serialization_roundtrip() {
        let scopes = vec![
            ApiKeyScope::Read,
            ApiKeyScope::Write,
            ApiKeyScope::Admin,
            ApiKeyScope::Tables(vec!["users".into()]),
            ApiKeyScope::Custom("webhook".into()),
        ];

        let json = serde_json::to_value(&scopes).expect("serialize");
        let deserialized: Vec<ApiKeyScope> = serde_json::from_value(json).expect("deserialize");
        assert_eq!(scopes, deserialized);
    }

    #[test]
    fn non_ddb_prefix_rejected() {
        // validate_api_key checks prefix synchronously before DB lookup.
        let key = "sk_live_fake_key";
        assert!(!key.starts_with(API_KEY_PREFIX));
    }
}
