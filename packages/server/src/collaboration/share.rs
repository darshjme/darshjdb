//! Share link generation, resolution, and lifecycle management.
//!
//! Share links provide URL-friendly tokens (8 chars, base62) that grant
//! scoped access to a specific resource. Tokens are derived from a
//! SHA-256 hash of a random UUID, then truncated and encoded for
//! compactness.
//!
//! Each share link tracks:
//! - The target resource (table, view, or record).
//! - The permission level (read-only, comment, or edit).
//! - Optional password protection, expiry, and usage caps.
//!
//! All share state is stored as EAV triples under entity `share:{uuid}`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Newtype wrapper for share link identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ShareId(pub Uuid);

impl ShareId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// The EAV entity key for this share.
    pub fn entity_key(&self) -> String {
        format!("share:{}", self.0)
    }
}

impl Default for ShareId {
    fn default() -> Self {
        Self::new()
    }
}

/// The kind of resource a share link points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    Table,
    View,
    Record,
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Table => write!(f, "table"),
            Self::View => write!(f, "view"),
            Self::Record => write!(f, "record"),
        }
    }
}

impl std::str::FromStr for ResourceType {
    type Err = DarshJError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "table" => Ok(Self::Table),
            "view" => Ok(Self::View),
            "record" => Ok(Self::Record),
            other => Err(DarshJError::InvalidAttribute(format!(
                "unknown resource type: {other}"
            ))),
        }
    }
}

/// Permission level granted by a share link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharePermission {
    ReadOnly,
    Comment,
    Edit,
}

impl std::fmt::Display for SharePermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "read_only"),
            Self::Comment => write!(f, "comment"),
            Self::Edit => write!(f, "edit"),
        }
    }
}

impl std::str::FromStr for SharePermission {
    type Err = DarshJError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "read_only" => Ok(Self::ReadOnly),
            "comment" => Ok(Self::Comment),
            "edit" => Ok(Self::Edit),
            other => Err(DarshJError::InvalidAttribute(format!(
                "unknown share permission: {other}"
            ))),
        }
    }
}

/// Full configuration for a share link.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareConfig {
    pub id: ShareId,
    pub resource_type: ResourceType,
    pub resource_id: Uuid,
    pub permission: SharePermission,
    /// BCrypt or Argon2 hash of an optional access password.
    /// When `Some`, the client must supply the password to access.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    /// When the link expires. `None` means it never expires.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Maximum number of times the link can be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    /// Current use count.
    pub use_count: u32,
    /// Who created this share link.
    pub created_by: Uuid,
    /// When it was created.
    pub created_at: DateTime<Utc>,
    /// The short token for URL embedding.
    pub token: String,
    /// Whether the share has been revoked.
    pub revoked: bool,
}

/// The public-facing share link returned to the creator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareLink {
    pub id: ShareId,
    pub token: String,
    pub resource_type: ResourceType,
    pub resource_id: Uuid,
    pub permission: SharePermission,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: Option<u32>,
    pub use_count: u32,
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Token generation
// ---------------------------------------------------------------------------

/// Base62 alphabet for URL-safe token encoding.
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Generate an 8-character base62 token from a UUID.
///
/// Process: SHA-256(uuid bytes) -> take first 6 bytes -> encode as base62.
/// This gives ~47 bits of entropy which is sufficient for short-lived
/// share tokens. Collision probability is ~1 in 140 trillion.
pub fn generate_token(id: &ShareId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(id.0.as_bytes());
    let hash = hasher.finalize();

    // Take first 6 bytes (48 bits) and encode to base62.
    let mut n: u64 = 0;
    for &b in &hash[..6] {
        n = (n << 8) | b as u64;
    }

    let mut token = String::with_capacity(8);
    for _ in 0..8 {
        token.push(BASE62[(n % 62) as usize] as char);
        n /= 62;
    }

    token
}

/// Derive a deterministic UUID from a share token string for reverse lookups.
///
/// Uses SHA-256 of the token and takes the first 16 bytes as a UUID.
/// This avoids depending on uuid v5 feature while remaining collision-safe.
fn token_to_lookup_uuid(token: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(b"darshjdb-share-token-lookup:");
    hasher.update(token.as_bytes());
    let hash = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    Uuid::from_bytes(bytes)
}

// ---------------------------------------------------------------------------
// EAV operations
// ---------------------------------------------------------------------------

/// Create a new share link, persisting it as EAV triples.
///
/// Returns the `ShareLink` with the generated token for the caller to
/// distribute.
pub async fn create_share(
    store: &PgTripleStore,
    resource_type: ResourceType,
    resource_id: Uuid,
    permission: SharePermission,
    password_hash: Option<String>,
    expires_at: Option<DateTime<Utc>>,
    max_uses: Option<u32>,
    created_by: Uuid,
) -> Result<ShareLink> {
    let id = ShareId::new();
    let token = generate_token(&id);
    let now = Utc::now();
    let entity_id = id.0;

    let mut triples = vec![
        TripleInput {
            entity_id,
            attribute: "share/token".into(),
            value: json!(token),
            value_type: 3, // string
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "share/resource_type".into(),
            value: json!(resource_type.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "share/resource_id".into(),
            value: json!(resource_id.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "share/permission".into(),
            value: json!(permission.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "share/created_by".into(),
            value: json!(created_by.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "share/created_at".into(),
            value: json!(now.to_rfc3339()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "share/use_count".into(),
            value: json!(0),
            value_type: 1, // integer
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "share/revoked".into(),
            value: json!(false),
            value_type: 4, // boolean
            ttl_seconds: None,
        },
    ];

    if let Some(ref pw) = password_hash {
        triples.push(TripleInput {
            entity_id,
            attribute: "share/password_hash".into(),
            value: json!(pw),
            value_type: 3,
            ttl_seconds: None,
        });
    }

    if let Some(exp) = expires_at {
        triples.push(TripleInput {
            entity_id,
            attribute: "share/expires_at".into(),
            value: json!(exp.to_rfc3339()),
            value_type: 3,
            ttl_seconds: None,
        });
    }

    if let Some(max) = max_uses {
        triples.push(TripleInput {
            entity_id,
            attribute: "share/max_uses".into(),
            value: json!(max),
            value_type: 1,
            ttl_seconds: None,
        });
    }

    // Also store a reverse lookup: token -> share_id.
    let token_entity = token_to_lookup_uuid(&token);
    triples.push(TripleInput {
        entity_id: token_entity,
        attribute: "share_token_lookup/share_id".into(),
        value: json!(id.0.to_string()),
        value_type: 3,
        ttl_seconds: None,
    });

    store.set_triples(&triples).await?;

    Ok(ShareLink {
        id,
        token,
        resource_type,
        resource_id,
        permission,
        expires_at,
        max_uses,
        use_count: 0,
        created_at: now,
    })
}

/// Resolve a share token to its configuration.
///
/// Returns `None` if the token is unknown, expired, revoked, or has
/// exceeded its maximum use count.
pub async fn resolve_share(
    store: &PgTripleStore,
    token: &str,
) -> Result<Option<ShareConfig>> {
    // Look up the share_id via the reverse lookup entity.
    let token_entity = token_to_lookup_uuid(token);
    let lookup_triples = store.get_entity(token_entity).await?;

    let share_id_str = lookup_triples
        .iter()
        .find(|t| t.attribute == "share_token_lookup/share_id")
        .and_then(|t| t.value.as_str().map(String::from));

    let share_id = match share_id_str {
        Some(s) => match Uuid::parse_str(&s) {
            Ok(id) => id,
            Err(_) => return Ok(None),
        },
        None => return Ok(None),
    };

    resolve_share_by_id(store, ShareId(share_id)).await
}

/// Resolve a share by its ID, performing all validity checks.
pub async fn resolve_share_by_id(
    store: &PgTripleStore,
    share_id: ShareId,
) -> Result<Option<ShareConfig>> {
    let triples = store.get_entity(share_id.0).await?;
    if triples.is_empty() {
        return Ok(None);
    }

    // Helper to extract a string attribute.
    let get_str = |attr: &str| -> Option<String> {
        triples
            .iter()
            .find(|t| t.attribute == attr)
            .and_then(|t| t.value.as_str().map(String::from))
    };

    let get_bool = |attr: &str| -> bool {
        triples
            .iter()
            .find(|t| t.attribute == attr)
            .and_then(|t| t.value.as_bool())
            .unwrap_or(false)
    };

    let get_u32 = |attr: &str| -> Option<u32> {
        triples
            .iter()
            .find(|t| t.attribute == attr)
            .and_then(|t| t.value.as_u64().map(|v| v as u32))
    };

    let token = match get_str("share/token") {
        Some(t) => t,
        None => return Ok(None),
    };

    let revoked = get_bool("share/revoked");
    if revoked {
        return Ok(None);
    }

    let resource_type: ResourceType = match get_str("share/resource_type") {
        Some(s) => s.parse()?,
        None => return Ok(None),
    };

    let resource_id = match get_str("share/resource_id").and_then(|s| Uuid::parse_str(&s).ok()) {
        Some(id) => id,
        None => return Ok(None),
    };

    let permission: SharePermission = match get_str("share/permission") {
        Some(s) => s.parse()?,
        None => return Ok(None),
    };

    let created_by = match get_str("share/created_by").and_then(|s| Uuid::parse_str(&s).ok()) {
        Some(id) => id,
        None => return Ok(None),
    };

    let created_at = get_str("share/created_at")
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let expires_at = get_str("share/expires_at")
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    // Check expiry.
    if let Some(exp) = expires_at {
        if Utc::now() > exp {
            return Ok(None);
        }
    }

    let max_uses = get_u32("share/max_uses");
    let use_count = get_u32("share/use_count").unwrap_or(0);

    // Check max uses.
    if let Some(max) = max_uses {
        if use_count >= max {
            return Ok(None);
        }
    }

    let password_hash = get_str("share/password_hash");

    Ok(Some(ShareConfig {
        id: share_id,
        resource_type,
        resource_id,
        permission,
        password_hash,
        expires_at,
        max_uses,
        use_count,
        created_by,
        created_at,
        token,
        revoked: false,
    }))
}

/// Increment the use count for a share link.
pub async fn increment_use_count(
    store: &PgTripleStore,
    share_id: ShareId,
    current_count: u32,
) -> Result<()> {
    // Retract old count and set new one.
    store.retract(share_id.0, "share/use_count").await?;
    store
        .set_triples(&[TripleInput {
            entity_id: share_id.0,
            attribute: "share/use_count".into(),
            value: json!(current_count + 1),
            value_type: 1,
            ttl_seconds: None,
        }])
        .await?;
    Ok(())
}

/// Revoke a share link. After revocation it can no longer be resolved.
pub async fn revoke_share(store: &PgTripleStore, share_id: ShareId) -> Result<()> {
    store.retract(share_id.0, "share/revoked").await?;
    store
        .set_triples(&[TripleInput {
            entity_id: share_id.0,
            attribute: "share/revoked".into(),
            value: json!(true),
            value_type: 4,
            ttl_seconds: None,
        }])
        .await?;
    Ok(())
}

/// List all share links created by a specific user.
pub async fn list_shares_by_creator(
    store: &PgTripleStore,
    user_id: Uuid,
) -> Result<Vec<ShareLink>> {
    let triples = store
        .query_by_attribute("share/created_by", Some(&json!(user_id.to_string())))
        .await?;

    let mut links = Vec::new();
    for triple in &triples {
        if let Some(config) = resolve_share_by_id(store, ShareId(triple.entity_id)).await? {
            links.push(ShareLink {
                id: config.id,
                token: config.token,
                resource_type: config.resource_type,
                resource_id: config.resource_id,
                permission: config.permission,
                expires_at: config.expires_at,
                max_uses: config.max_uses,
                use_count: config.use_count,
                created_at: config.created_at,
            });
        }
    }

    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_8_chars_base62() {
        let id = ShareId::new();
        let token = generate_token(&id);
        assert_eq!(token.len(), 8);
        assert!(token.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn token_deterministic_for_same_id() {
        let id = ShareId(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap());
        let t1 = generate_token(&id);
        let t2 = generate_token(&id);
        assert_eq!(t1, t2);
    }

    #[test]
    fn different_ids_produce_different_tokens() {
        let t1 = generate_token(&ShareId::new());
        let t2 = generate_token(&ShareId::new());
        assert_ne!(t1, t2);
    }

    #[test]
    fn resource_type_roundtrip() {
        for rt in [ResourceType::Table, ResourceType::View, ResourceType::Record] {
            let s = rt.to_string();
            let parsed: ResourceType = s.parse().unwrap();
            assert_eq!(parsed, rt);
        }
    }

    #[test]
    fn share_permission_ordering() {
        assert!(SharePermission::ReadOnly < SharePermission::Comment);
        assert!(SharePermission::Comment < SharePermission::Edit);
    }

    #[test]
    fn share_id_entity_key_format() {
        let id = ShareId(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap());
        assert_eq!(id.entity_key(), "share:550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn invalid_resource_type() {
        let result: std::result::Result<ResourceType, _> = "invalid".parse();
        assert!(result.is_err());
    }

    #[test]
    fn invalid_share_permission() {
        let result: std::result::Result<SharePermission, _> = "invalid".parse();
        assert!(result.is_err());
    }
}
