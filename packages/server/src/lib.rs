#![recursion_limit = "512"]
//! DarshJDB server library crate.
//!
//! Provides the core data layer: a Postgres-backed triple store,
//! the DarshJQL query engine with plan caching, and reactive
//! dependency tracking for live query invalidation.
//!
//! # Modules
//!
//! - [`error`] — Unified error types (`DarshJError`, `Result`).
//! - [`triple_store`] — Triple storage, schema inference, migrations.
//! - [`query`] — DarshJQL parsing, planning, execution, and caching.

pub mod activity;
pub mod aggregation;
pub mod api;
pub mod api_keys;
pub mod audit;
pub mod auth;
pub mod automations;
pub mod cache;
pub mod collaboration;
pub mod connectors;
pub mod embeddings;
pub mod error;
pub mod events;
pub mod fields;
pub mod formulas;
pub mod functions;
pub mod graph;
pub mod history;
pub mod import_export;
pub mod plugins;
pub mod query;
pub mod relations;
pub mod rules;
pub mod schema;
pub mod storage;
pub mod sync;
pub mod tables;
pub mod triple_store;
pub mod views;
pub mod webhooks;
