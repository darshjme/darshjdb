//! DarshanDB server library crate.
//!
//! Provides the core data layer: a Postgres-backed triple store,
//! the DarshanQL query engine with plan caching, and reactive
//! dependency tracking for live query invalidation.
//!
//! # Modules
//!
//! - [`error`] — Unified error types (`DarshanError`, `Result`).
//! - [`triple_store`] — Triple storage, schema inference, migrations.
//! - [`query`] — DarshanQL parsing, planning, execution, and caching.

pub mod api;
pub mod audit;
pub mod auth;
pub mod cache;
pub mod connectors;
pub mod embeddings;
pub mod error;
pub mod functions;
pub mod query;
pub mod rules;
pub mod storage;
pub mod sync;
pub mod triple_store;
