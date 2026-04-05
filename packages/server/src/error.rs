//! Unified error types for DarshJDB server.
//!
//! All fallible operations across the triple store and query engine
//! surface errors through [`DarshJError`], which maps cleanly to
//! both internal handling (via `thiserror`) and HTTP responses.

use thiserror::Error;

/// Top-level error type for all DarshJDB server operations.
#[derive(Debug, Error)]
pub enum DarshJError {
    /// A database query or connection failed.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// The caller supplied an invalid or malformed query.
    #[error("invalid query: {0}")]
    InvalidQuery(String),

    /// The requested entity does not exist.
    #[error("entity not found: {0}")]
    EntityNotFound(uuid::Uuid),

    /// An attribute name violates naming rules or does not exist in schema.
    #[error("invalid attribute: {0}")]
    InvalidAttribute(String),

    /// A value could not be coerced to the expected type.
    #[error("type mismatch: expected {expected}, got {actual}")]
    TypeMismatch {
        /// The type that was expected.
        expected: String,
        /// The type that was actually provided.
        actual: String,
    },

    /// Schema migration would result in data loss or conflict.
    #[error("schema conflict: {0}")]
    SchemaConflict(String),

    /// JSON serialization or deserialization failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// An internal invariant was violated.
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, DarshJError>;
