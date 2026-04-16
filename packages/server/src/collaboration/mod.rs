//! Collaboration and sharing system for DarshJDB.
//!
//! Provides three layers of multi-user access:
//!
//! - **Share links**: Public or password-protected URLs granting temporary
//!   access to a table, view, or record. Tokens are short base62 strings
//!   derived from SHA-256 for URL-friendliness.
//!
//! - **Collaborators**: Named users invited by email with a specific role
//!   (Owner, Admin, Editor, Commenter, Viewer). Role hierarchy enforces
//!   who can invite, remove, or change permissions.
//!
//! - **Workspaces**: Team containers that group tables and views under a
//!   shared permission boundary. Members inherit workspace-level roles
//!   unless overridden at the resource level.
//!
//! All state is persisted as EAV triples (`share:{uuid}`, `collaborator:{uuid}`,
//! `workspace:{uuid}`) through the existing triple store, keeping the
//! collaboration layer schema-free and auditable.

pub mod collaborator;
pub mod handlers;
pub mod share;
pub mod workspace;

pub use collaborator::{Collaborator, CollaboratorRole, InviteStatus};
pub use share::{ResourceType, ShareConfig, ShareId, ShareLink, SharePermission};
pub use workspace::{Workspace, WorkspaceMember, WorkspaceRole};
