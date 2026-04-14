#![recursion_limit = "512"]
#![allow(clippy::type_complexity)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::new_without_default)]
#![allow(clippy::module_inception)]
#![allow(clippy::manual_strip)]
#![allow(clippy::redundant_guards)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::float_cmp)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::match_like_matches_macro)]
#![allow(clippy::manual_clamp)]
#![allow(clippy::explicit_counter_loop)]
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
pub mod agent_memory;
pub mod aggregation;
pub mod anchor;
pub mod api;
pub mod api_keys;
pub mod audit;
pub mod auth;
pub mod automations;
pub mod cache;
pub mod collaboration;
pub mod connectors;
pub mod embeddings;
#[cfg(feature = "embedded-db")]
pub mod embedded_pg;
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
