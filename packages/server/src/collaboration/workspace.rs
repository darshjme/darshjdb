//! Workspace (team) management for DarshJDB.
//!
//! A workspace is a container that groups tables and views under a shared
//! permission boundary. All resources within a workspace inherit the
//! workspace-level member roles unless overridden at the resource level
//! via the collaborator system.
//!
//! Cross-workspace sharing is achieved through share links, which are
//! orthogonal to workspace membership.
//!
//! State is persisted as EAV triples under entities `workspace:{uuid}`
//! and `workspace_member:{uuid}`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Role within a workspace. Mirrors [`super::collaborator::CollaboratorRole`]
/// but scoped to workspace-level access. Resources inside the workspace
/// inherit these permissions unless explicitly overridden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRole {
    Viewer = 0,
    Commenter = 1,
    Editor = 2,
    Admin = 3,
    Owner = 4,
}

impl WorkspaceRole {
    pub fn can_manage(&self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }

    pub fn can_modify(&self, target: WorkspaceRole) -> bool {
        match self {
            Self::Owner => true,
            Self::Admin => target < Self::Admin,
            _ => false,
        }
    }
}

impl std::fmt::Display for WorkspaceRole {
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

impl std::str::FromStr for WorkspaceRole {
    type Err = DarshJError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "viewer" => Ok(Self::Viewer),
            "commenter" => Ok(Self::Commenter),
            "editor" => Ok(Self::Editor),
            "admin" => Ok(Self::Admin),
            "owner" => Ok(Self::Owner),
            other => Err(DarshJError::InvalidAttribute(format!(
                "unknown workspace role: {other}"
            ))),
        }
    }
}

/// A workspace / team container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Uuid,
    pub name: String,
    /// URL-friendly slug, unique within the system.
    pub slug: String,
    pub owner_id: Uuid,
    pub created_at: DateTime<Utc>,
    /// Arbitrary settings stored as JSON (e.g., default permissions,
    /// branding, feature flags).
    pub settings: Value,
}

/// A workspace membership record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceMember {
    pub id: Uuid,
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub role: WorkspaceRole,
    pub joined_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Slug generation
// ---------------------------------------------------------------------------

/// Generate a URL-safe slug from a workspace name.
///
/// Lowercases, replaces spaces/non-alphanumeric with hyphens, trims
/// leading/trailing hyphens, and collapses consecutive hyphens.
pub fn slugify(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse consecutive hyphens and trim.
    let mut result = String::with_capacity(slug.len());
    let mut prev_hyphen = true; // treat start as hyphen to trim leading
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push('-');
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    // Trim trailing hyphen.
    if result.ends_with('-') {
        result.pop();
    }

    result
}

// ---------------------------------------------------------------------------
// EAV operations
// ---------------------------------------------------------------------------

/// Create a new workspace.
///
/// The creating user is automatically added as the `Owner`.
pub async fn create_workspace(
    store: &PgTripleStore,
    name: &str,
    owner_id: Uuid,
    settings: Option<Value>,
) -> Result<Workspace> {
    if name.trim().is_empty() {
        return Err(DarshJError::InvalidAttribute(
            "workspace name must not be empty".into(),
        ));
    }

    let id = Uuid::new_v4();
    let slug = slugify(name);
    let now = Utc::now();
    let settings = settings.unwrap_or_else(|| json!({}));

    let triples = vec![
        TripleInput {
            entity_id: id,
            attribute: "workspace/name".into(),
            value: json!(name),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace/slug".into(),
            value: json!(slug),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace/owner_id".into(),
            value: json!(owner_id.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace/created_at".into(),
            value: json!(now.to_rfc3339()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace/settings".into(),
            value: settings.clone(),
            value_type: 5, // json object
            ttl_seconds: None,
        },
    ];

    store.set_triples(&triples).await?;

    // Add the owner as a workspace member.
    add_member(store, id, owner_id, WorkspaceRole::Owner).await?;

    Ok(Workspace {
        id,
        name: name.to_string(),
        slug,
        owner_id,
        created_at: now,
        settings,
    })
}

/// Load a workspace by ID.
pub async fn get_workspace(store: &PgTripleStore, workspace_id: Uuid) -> Result<Option<Workspace>> {
    let triples = store.get_entity(workspace_id).await?;
    if triples.is_empty() {
        return Ok(None);
    }

    let get_str = |attr: &str| -> Option<String> {
        triples
            .iter()
            .find(|t| t.attribute == attr)
            .and_then(|t| t.value.as_str().map(String::from))
    };

    let name = match get_str("workspace/name") {
        Some(n) => n,
        None => return Ok(None),
    };

    let slug = get_str("workspace/slug").unwrap_or_else(|| slugify(&name));

    let owner_id = match get_str("workspace/owner_id").and_then(|s| Uuid::parse_str(&s).ok()) {
        Some(id) => id,
        None => return Ok(None),
    };

    let created_at = get_str("workspace/created_at")
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    let settings = triples
        .iter()
        .find(|t| t.attribute == "workspace/settings")
        .map(|t| t.value.clone())
        .unwrap_or_else(|| json!({}));

    Ok(Some(Workspace {
        id: workspace_id,
        name,
        slug,
        owner_id,
        created_at,
        settings,
    }))
}

/// Update a workspace's name and/or settings.
pub async fn update_workspace(
    store: &PgTripleStore,
    workspace_id: Uuid,
    name: Option<&str>,
    settings: Option<Value>,
) -> Result<()> {
    if let Some(new_name) = name {
        if new_name.trim().is_empty() {
            return Err(DarshJError::InvalidAttribute(
                "workspace name must not be empty".into(),
            ));
        }

        store.retract(workspace_id, "workspace/name").await?;
        store
            .set_triples(&[TripleInput {
                entity_id: workspace_id,
                attribute: "workspace/name".into(),
                value: json!(new_name),
                value_type: 3,
                ttl_seconds: None,
            }])
            .await?;

        let new_slug = slugify(new_name);
        store.retract(workspace_id, "workspace/slug").await?;
        store
            .set_triples(&[TripleInput {
                entity_id: workspace_id,
                attribute: "workspace/slug".into(),
                value: json!(new_slug),
                value_type: 3,
                ttl_seconds: None,
            }])
            .await?;
    }

    if let Some(new_settings) = settings {
        store.retract(workspace_id, "workspace/settings").await?;
        store
            .set_triples(&[TripleInput {
                entity_id: workspace_id,
                attribute: "workspace/settings".into(),
                value: new_settings,
                value_type: 5,
                ttl_seconds: None,
            }])
            .await?;
    }

    Ok(())
}

/// Add a member to a workspace.
pub async fn add_member(
    store: &PgTripleStore,
    workspace_id: Uuid,
    user_id: Uuid,
    role: WorkspaceRole,
) -> Result<WorkspaceMember> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    let triples = vec![
        TripleInput {
            entity_id: id,
            attribute: "workspace_member/workspace_id".into(),
            value: json!(workspace_id.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace_member/user_id".into(),
            value: json!(user_id.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace_member/role".into(),
            value: json!(role.to_string()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace_member/joined_at".into(),
            value: json!(now.to_rfc3339()),
            value_type: 3,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: id,
            attribute: "workspace_member/active".into(),
            value: json!(true),
            value_type: 4,
            ttl_seconds: None,
        },
    ];

    store.set_triples(&triples).await?;

    Ok(WorkspaceMember {
        id,
        user_id,
        workspace_id,
        role,
        joined_at: now,
    })
}

/// Remove a member from a workspace (soft-delete via active flag).
pub async fn remove_member(store: &PgTripleStore, member_id: Uuid) -> Result<()> {
    store.retract(member_id, "workspace_member/active").await?;
    store
        .set_triples(&[TripleInput {
            entity_id: member_id,
            attribute: "workspace_member/active".into(),
            value: json!(false),
            value_type: 4,
            ttl_seconds: None,
        }])
        .await?;
    Ok(())
}

/// Update a workspace member's role.
pub async fn update_member_role(
    store: &PgTripleStore,
    member_id: Uuid,
    new_role: WorkspaceRole,
) -> Result<()> {
    store.retract(member_id, "workspace_member/role").await?;
    store
        .set_triples(&[TripleInput {
            entity_id: member_id,
            attribute: "workspace_member/role".into(),
            value: json!(new_role.to_string()),
            value_type: 3,
            ttl_seconds: None,
        }])
        .await?;
    Ok(())
}

/// List all active members of a workspace.
pub async fn list_members(
    store: &PgTripleStore,
    workspace_id: Uuid,
) -> Result<Vec<WorkspaceMember>> {
    let triples = store
        .query_by_attribute(
            "workspace_member/workspace_id",
            Some(&json!(workspace_id.to_string())),
        )
        .await?;

    let mut members = Vec::new();
    for triple in &triples {
        if let Some(member) = load_member(store, triple.entity_id).await? {
            members.push(member);
        }
    }

    Ok(members)
}

/// List all workspaces a user belongs to.
pub async fn list_user_workspaces(store: &PgTripleStore, user_id: Uuid) -> Result<Vec<Workspace>> {
    let triples = store
        .query_by_attribute(
            "workspace_member/user_id",
            Some(&json!(user_id.to_string())),
        )
        .await?;

    let mut workspaces = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    for triple in &triples {
        if let Some(member) = load_member(store, triple.entity_id).await?
            && seen_ids.insert(member.workspace_id)
            && let Some(ws) = get_workspace(store, member.workspace_id).await?
        {
            workspaces.push(ws);
        }
    }

    Ok(workspaces)
}

/// Get a user's role within a workspace.
pub async fn get_member_role(
    store: &PgTripleStore,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceRole>> {
    let members = list_members(store, workspace_id).await?;
    Ok(members
        .iter()
        .find(|m| m.user_id == user_id)
        .map(|m| m.role))
}

/// Load a single workspace member by entity ID.
fn load_member(
    store: &PgTripleStore,
    member_id: Uuid,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Option<WorkspaceMember>>> + Send + '_>>
{
    Box::pin(async move {
        let triples = store.get_entity(member_id).await?;
        if triples.is_empty() {
            return Ok(None);
        }

        let get_str = |attr: &str| -> Option<String> {
            triples
                .iter()
                .find(|t| t.attribute == attr)
                .and_then(|t| t.value.as_str().map(String::from))
        };

        // Check if active.
        let active = triples
            .iter()
            .find(|t| t.attribute == "workspace_member/active")
            .and_then(|t| t.value.as_bool())
            .unwrap_or(true);

        if !active {
            return Ok(None);
        }

        let workspace_id =
            match get_str("workspace_member/workspace_id").and_then(|s| Uuid::parse_str(&s).ok()) {
                Some(id) => id,
                None => return Ok(None),
            };

        let user_id =
            match get_str("workspace_member/user_id").and_then(|s| Uuid::parse_str(&s).ok()) {
                Some(id) => id,
                None => return Ok(None),
            };

        let role: WorkspaceRole = match get_str("workspace_member/role") {
            Some(s) => s.parse()?,
            None => return Ok(None),
        };

        let joined_at = get_str("workspace_member/joined_at")
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        Ok(Some(WorkspaceMember {
            id: member_id,
            user_id,
            workspace_id,
            role,
            joined_at,
        }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("My Workspace"), "my-workspace");
    }

    #[test]
    fn slugify_special_chars() {
        assert_eq!(slugify("Hello World! @#$%"), "hello-world");
    }

    #[test]
    fn slugify_consecutive_spaces() {
        assert_eq!(slugify("a   b   c"), "a-b-c");
    }

    #[test]
    fn slugify_leading_trailing() {
        assert_eq!(slugify("  spaces  "), "spaces");
    }

    #[test]
    fn workspace_role_hierarchy() {
        assert!(WorkspaceRole::Viewer < WorkspaceRole::Commenter);
        assert!(WorkspaceRole::Commenter < WorkspaceRole::Editor);
        assert!(WorkspaceRole::Editor < WorkspaceRole::Admin);
        assert!(WorkspaceRole::Admin < WorkspaceRole::Owner);
    }

    #[test]
    fn workspace_role_can_manage() {
        assert!(WorkspaceRole::Owner.can_manage());
        assert!(WorkspaceRole::Admin.can_manage());
        assert!(!WorkspaceRole::Editor.can_manage());
        assert!(!WorkspaceRole::Commenter.can_manage());
        assert!(!WorkspaceRole::Viewer.can_manage());
    }

    #[test]
    fn workspace_role_can_modify() {
        assert!(WorkspaceRole::Owner.can_modify(WorkspaceRole::Admin));
        assert!(WorkspaceRole::Owner.can_modify(WorkspaceRole::Viewer));
        assert!(WorkspaceRole::Admin.can_modify(WorkspaceRole::Editor));
        assert!(!WorkspaceRole::Admin.can_modify(WorkspaceRole::Admin));
        assert!(!WorkspaceRole::Admin.can_modify(WorkspaceRole::Owner));
        assert!(!WorkspaceRole::Editor.can_modify(WorkspaceRole::Viewer));
    }

    #[test]
    fn workspace_role_roundtrip() {
        for role in [
            WorkspaceRole::Viewer,
            WorkspaceRole::Commenter,
            WorkspaceRole::Editor,
            WorkspaceRole::Admin,
            WorkspaceRole::Owner,
        ] {
            let s = role.to_string();
            let parsed: WorkspaceRole = s.parse().unwrap();
            assert_eq!(parsed, role);
        }
    }
}
