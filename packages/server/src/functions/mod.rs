//! Server-side function execution runtime for DarshJDB.
//!
//! This module provides the infrastructure for running user-defined functions
//! (queries, mutations, actions, scheduled jobs, and HTTP endpoints) in an
//! isolated, resource-limited environment.
//!
//! # Architecture
//!
//! ```text
//! Client Request
//!       │
//!       ▼
//!   Registry ──lookup──▶ FunctionDef
//!       │                     │
//!       ▼                     ▼
//!   Validator ──check──▶ ArgSchema
//!       │
//!       ▼
//!   Runtime ──spawn──▶ Isolate (subprocess)
//!       │                     │
//!       ▼                     ▼
//!   Scheduler            ctx.db / ctx.auth / ctx.scheduler
//! ```
//!
//! - **Registry**: Discovers and indexes `.ts`/`.js` function files with hot reload.
//! - **Validator**: Validates function arguments against declared schemas.
//! - **Runtime**: Executes functions in resource-limited isolates via a pluggable backend.
//! - **Scheduler**: Runs cron-scheduled functions with distributed locking and retry.

pub mod registry;
pub mod runtime;
pub mod scheduler;
pub mod validator;

pub use registry::{FunctionDef, FunctionKind, FunctionRegistry};
pub use runtime::{FunctionRuntime, ResourceLimits, RuntimeBackend};
pub use scheduler::{ScheduledJob, Scheduler};
pub use validator::{ArgSchema, validate_args};
