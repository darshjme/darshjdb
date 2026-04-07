//! DarshQL — a SurrealQL-inspired query language for DarshJDB.
//!
//! Provides a SQL-like syntax with graph traversal, record links,
//! RELATE statements, LIVE SELECT subscriptions, type casting, and
//! computed fields. Designed as a human-friendly interface to the
//! underlying triple store.
//!
//! # Supported Statements
//!
//! - `SELECT` — query with graph traversal, type casts, computed fields
//! - `CREATE` — create records with record links
//! - `UPDATE` — modify records with WHERE filters
//! - `DELETE` — soft-delete records
//! - `INSERT` — batch insert rows
//! - `RELATE` — create graph edges between records
//! - `LIVE SELECT` — register live query subscriptions
//! - `DEFINE TABLE` — define table schema
//! - `DEFINE FIELD` — define field constraints
//! - `INFO FOR` — introspect schema
//!
//! # Examples
//!
//! ```text
//! SELECT * FROM users WHERE age > 18 ORDER BY name ASC LIMIT 10
//! SELECT ->friends->friends FROM user:darsh
//! CREATE user:darsh SET name = "Darsh", company = company:knowai
//! RELATE user:darsh->works_at->company:knowai SET since = "2024"
//! LIVE SELECT * FROM users
//! SELECT <int>age, <string>id FROM users
//! SELECT *, count(->posts) AS post_count FROM users
//! ```

pub mod ast;
pub mod executor;
pub mod parser;

pub use ast::*;
pub use executor::{ExecResult, execute};
pub use parser::Parser;
