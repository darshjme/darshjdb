//! Collaborator invitation, role management, and access control.
//!
//! The collaborator system manages named-user access to specific resources
//! (tables, views, records). Each collaborator has a role that determines
//! their capabilities. Roles form a strict hierarchy:
//!
//! ```text
//! Owner > Admin > Editor > Commenter > Viewer
//! ```
//!
//! Only users with `Owner` or `Admin` roles can invite new collaborators,
//! change roles, or remove existing collaborators. An `Admin` cannot
//! modify another `Admin` or the `Owner`.
//!
//! State is persisted as EAV triples under entity `collaborator:{uuid}`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::share::ResourceType;
use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Role hierarchy for collaborators. The discriminant values encode the
/// hierarchy: higher values = more permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaboratorRole {
    Viewer = 0,
    Commenter = 1,
    Editor = 2,
    Admin = 3,
    Owner = 4,
}

impl CollaboratorRole {
    /// Whether this role can manage (invite/remove/change) other collaborators.
    pub fn can_manage(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    /// Whether this role can modify a target role.
    ///
    /// Admins cannot modify other Admins or the Owner.
    /// Only the Owner can modify Admins.
    pub fn can_modify(&self, target: CollaboratorRole) -> bool {
        match self {
            Self::Owner => true,
            Self::Admin => target < Self::Admin,
            _ => false,
        }
    }
}

impl std::fmt::Display for CollaboratorRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Viewer => write!(f, "viewer"),
            Self::Commenter => write!(f, "commenter"),
            Self::Editor => write!(f, "editor"),
            Self::Admin => write!(f, "admin"),
            Self::Owner => write!(f, "owner"),
        }
    }
}

impl std::str::FromStr for CollaboratorRole {
    type Err = DarshJError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "viewer" => Ok(Self::Viewer),
            "commenter" => Ok(Self::Commenter),
            "editor" => Ok(Self::Editor),
            "admin" => Ok(Self::Admin),
            "owner" => Ok(Self::Owner),
            other => Err(DarshJError::InvalidAttribute(format!(
                "unknown collaborator role: {other}"
            ))),
        }
    }
}

/// Invitation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus {
    Pending,
    Accepted,
    Declined,
    Revoked,
}

impl std::fmt::Display for InviteStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Accepted => write!(f, "accepted"),
            Self::Declined => write!(f, "declined"),
            Self::Revoked => write!(f, "revoked"),
        }
    }
}

impl std::str::FromStr for InviteStatus {
    type Err = DarshJError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "revoked" => Ok(Self::Revoked),
            other => Err(DarshJError::InvalidAttribute(format!(
                "unknown invite status: {other}"
            ))),
        }
    }
}

/// A collaborator record linking a user to a resource with a role.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collaborator {
    pub id: Uuid,
    /// The user ID of the collaborator. `None` if the invite is still
    /// pending and the user hasn't signed up yet.
    pub user_id: Option<Uuid>,
    /// Email address the invite was sent to.
    pub email: String,
    pub resource_type: ResourceType,
    pub resource_id: Uuid,
    pub role: CollaboratorRole,
    pub status: InviteStatus,
    pub invited_by: Uuid,
    pub invited_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// EAV operations
// ---------------------------------------------------------------------------

/// Invite a collaborator to a resource.
///
/// Creates a pending collaborator record. The caller is responsible for
/// verifying that the inviting user has `Owner` or `Admin` role on the
/// resource before calling this function.
pub async fn invite_collaborator(
    store: &PgTripleStore,
    email: &str,
    resource_type: ResourceType,
    resource_id: Uuid,
    role: CollaboratorRole,
    invited_by: Uuid,
) -> Result<Collaborator> {
    if role == CollaboratorRole::Owner {
        return Err(DarshJError::InvalidAttribute(
            "cannot invite as owner; transfer ownership instead".into(),
        ));
    }

    let id = Uuid::new_v4();
    let now = Utc::now();

    let triples = vec![
        TripleInput {
            entity_id: id,
            attribute: "collaborator/email".into(),
            value: json!(email),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "collaborator/resource_type".into(),
            value: json!(resource_type.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "collaborator/resource_id".into(),
            value: json!(resource_id.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "collaborator/role".into(),
            value: json!(role.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "collaborator/status".into(),
            value: json!("pending"),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "collaborator/invited_by".into(),
            value: json!(invited_by.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "collaborator/invited_at".into(),
            value: json!(now.to_rfc3339()),
            value_type: 3,
            ttl_seconds: None,
        },
    ];

    store.set_triples(&triples).await?;

    Ok(Collaborator {
        id,
        user_id: None,
        email: email.to_string(),
        resource_type,
        resource_id,
        role,
        status: InviteStatus::Pending,
        invited_by,
        invited_at: now,
        accepted_at: None,
    })
}

/// Accept a pending invite, binding it to a user ID.
pub async fn accept_invite(
    store: &PgTripleStore,
    collaborator_id: Uuid,
    user_id: Uuid,
) -> Result<()> {
    let collab = load_collaborator(store, collaborator_id).await?;
    let collab = collab.ok_or_else(|| {
        DarshJError::InvalidAttribute("collaborator not found".into())
    })?;

    if collab.status != InviteStatus::Pending {
        return Err(DarshJError::InvalidAttribute(format!(
            "invite is not pending, current status: {}",
            collab.status
        )));
    }

    let now = Utc::now();

    // Set user_id.
    store
        .set_triples(&[TripleInput {
            entity_id: collaborator_id,
            attribute: "collaborator/user_id".into(),
            value: json!(user_id.to_string()),
            value_type: 3,
            ttl_seconds: None,
        }])
        .await?;

    // Update status.
    store
        .retract(collaborator_id, "collaborator/status")
        .await?;
    store
        .set_triples(&[TripleInput {
            entity_id: collaborator_id,
            attribute: "collaborator/status".into(),
            value: json!("accepted"),
            value_type: 3,
            ttl_seconds: None,
        }])
        .await?;

    // Set accepted_at.
    store
        .set_triples(&[TripleInput {
            entity_id: collaborator_id,
            attribute: "collaborator/accepted_at".into(),
            value: json!(now.to_rfc3339()),
            value_type: 3,
            ttl_seconds: None,
        }])
        .await?;

    Ok(())
}

/// Update a collaborator's role.
///
/// The caller must verify that the acting user has sufficient permissions
/// (Owner or Admin with hierarchy rules) before calling this.
pub async fn update_role(
    store: &PgTripleStore,
    collaborator_id: Uuid,
    new_role: CollaboratorRole,
) -> Result<()> {
    if new_role == CollaboratorRole::Owner {
        return Err(DarshJError::InvalidAttribute(
            "cannot promote to owner; use ownership transfer".into(),
        ));
    }

    store
        .retract(collaborator_id, "collaborator/role")
        .await?;
    store
        .set_triples(&[TripleInput {
            entity_id: collaborator_id,
            attribute: "collaborator/role".into(),
            value: json!(new_role.to_string()),
            value_type: 3,
            ttl_seconds: None,
        }])
        .await?;

    Ok(())
}

/// Remove a collaborator (revoke their access).
pub async fn remove_collaborator(
    store: &PgTripleStore,
    collaborator_id: Uuid,
) -> Result<()> {
    store
        .retract(collaborator_id, "collaborator/status")
        .await?;
    store
        .set_triples(&[TripleInput {
            entity_id: collaborator_id,
            attribute: "collaborator/status".into(),
            value: json!("revoked"),
            value_type: 3,
            ttl_seconds: None,
        }])
        .await?;

    Ok(())
}

/// List all active collaborators for a resource.
pub async fn list_collaborators(
    store: &PgTripleStore,
    resource_type: ResourceType,
    resource_id: Uuid,
) -> Result<Vec<Collaborator>> {
    // Find all collaborator entities for this resource.
    let triples = store
        .query_by_attribute(
            "collaborator/resource_id",
            Some(&json!(resource_id.to_string())),
        )
        .await?;

    let mut collaborators = Vec::new();
    for triple in &triples {
        if let Some(collab) = load_collaborator(store, triple.entity_id).await? {
            // Filter by resource_type and exclude revoked.
            if collab.resource_type == resource_type && collab.status != InviteStatus::Revoked {
                collaborators.push(collab);
            }
        }
    }

    Ok(collaborators)
}

/// Get a user's role for a specific resource, if any.
pub async fn get_user_role(
    store: &PgTripleStore,
    user_id: Uuid,
    resource_type: ResourceType,
    resource_id: Uuid,
) -> Result<Option<CollaboratorRole>> {
    let collabs = list_collaborators(store, resource_type, resource_id).await?;
    Ok(collabs
        .iter()
        .find(|c| c.user_id == Some(user_id) && c.status == InviteStatus::Accepted)
        .map(|c| c.role))
}

/// Load a single collaborator by its entity ID.
pub async fn load_collaborator(
    store: &PgTripleStore,
    collaborator_id: Uuid,
) -> Result<Option<Collaborator>> {
    let triples = store.get_entity(collaborator_id).await?;
    if triples.is_empty() {
        return Ok(None);
    }

    let get_str = |attr: &str| -> Option<String> {
        triples
            .iter()
            .find(|t| t.attribute == attr)
            .and_then(|t| t.value.as_str().map(String::from))
    };

    let email = match get_str("collaborator/email") {
        Some(e) => e,
        None => return Ok(None),
    };

    let resource_type: ResourceType = match get_str("collaborator/resource_type") {
        Some(s) => s.parse()?,
        None => return Ok(None),
    };

    let resource_id = match get_str("collaborator/resource_id")
        .and_then(|s| Uuid::parse_str(&s).ok())
    {
        Some(id) => id,
        None => return Ok(None),
    };

    let role: CollaboratorRole = match get_str("collaborator/role") {
        Some(s) => s.parse()?,
        None => return Ok(None),
    };

    let status: InviteStatus = match get_str("collaborator/status") {
        Some(s) => s.parse()?,
        None => return Ok(None),
    };

    let invited_by = match get_str("collaborator/invited_by")
        .and_then(|s| Uuid::parse_str(&s).ok())
    {
        Some(id) => id,
        None => return Ok(None),
    };

    let invited_at = get_str("collaborator/invited_at")
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let user_id = get_str("collaborator/user_id")
        .and_then(|s| Uuid::parse_str(&s).ok());

    let accepted_at = get_str("collaborator/accepted_at")
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc));

    Ok(Some(Collaborator {
        id: collaborator_id,
        user_id,
        email,
        resource_type,
        resource_id,
        role,
        status,
        invited_by,
        invited_at,
        accepted_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_hierarchy_ordering() {
        assert!(CollaboratorRole::Viewer < CollaboratorRole::Commenter);
        assert!(CollaboratorRole::Commenter < CollaboratorRole::Editor);
        assert!(CollaboratorRole::Editor < CollaboratorRole::Admin);
        assert!(CollaboratorRole::Admin < CollaboratorRole::Owner);
    }

    #[test]
    fn owner_can_manage() {
        assert!(CollaboratorRole::Owner.can_manage());
    }

    #[test]
    fn admin_can_manage() {
        assert!(CollaboratorRole::Admin.can_manage());
    }

    #[test]
    fn editor_cannot_manage() {
        assert!(!CollaboratorRole::Editor.can_manage());
    }

    #[test]
    fn viewer_cannot_manage() {
        assert!(!CollaboratorRole::Viewer.can_manage());
    }

    #[test]
    fn owner_can_modify_admin() {
        assert!(CollaboratorRole::Owner.can_modify(CollaboratorRole::Admin));
    }

    #[test]
    fn admin_cannot_modify_admin() {
        assert!(!CollaboratorRole::Admin.can_modify(CollaboratorRole::Admin));
    }

    #[test]
    fn admin_can_modify_editor() {
        assert!(CollaboratorRole::Admin.can_modify(CollaboratorRole::Editor));
    }

    #[test]
    fn admin_cannot_modify_owner() {
        assert!(!CollaboratorRole::Admin.can_modify(CollaboratorRole::Owner));
    }

    #[test]
    fn editor_cannot_modify_viewer() {
        assert!(!CollaboratorRole::Editor.can_modify(CollaboratorRole::Viewer));
    }

    #[test]
    fn role_roundtrip() {
        for role in [
            CollaboratorRole::Viewer,
            CollaboratorRole::Commenter,
            CollaboratorRole::Editor,
            CollaboratorRole::Admin,
            CollaboratorRole::Owner,
        ] {
            let s = role.to_string();
            let parsed: CollaboratorRole = s.parse().unwrap();
            assert_eq!(parsed, role);
        }
    }

    #[test]
    fn invalid_role_parse() {
        let result: std::result::Result<CollaboratorRole, _> = "superadmin".parse();
        assert!(result.is_err());
    }

    #[test]
    fn invite_status_roundtrip() {
        for status in [
            InviteStatus::Pending,
            InviteStatus::Accepted,
            InviteStatus::Declined,
            InviteStatus::Revoked,
        ] {
            let s = status.to_string();
            let parsed: InviteStatus = s.parse().unwrap();
            assert_eq!(parsed, status);
        }
    }
}
