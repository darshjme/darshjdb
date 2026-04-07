//! Formula engine: expression parsing, evaluation, and dependency-driven
//! cascading recalculation for computed fields.
//!
//! Inspired by Teable's calculation engine, this module provides:
//!
//! - [`parser`] — Tokenizer + recursive descent parser that converts formula
//!   strings (e.g. `IF({Status} = "Done", 1, 0)`) into an AST.
//! - [`evaluator`] — Walks the AST to produce a [`serde_json::Value`] given
//!   a record context (field name → value map).
//! - [`graph`] — Topological dependency graph that determines recalculation
//!   order and detects circular references.
//! - [`recalculate`] — Batch recalculation engine that propagates changes
//!   through the dependency graph within a single Postgres transaction.

pub mod evaluator;
pub mod graph;
pub mod parser;
pub mod recalculate;
