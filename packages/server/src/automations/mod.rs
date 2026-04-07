//! Automation engine for DarshJDB.
//!
//! Event-driven workflow automation inspired by Teable's automation system
//! and DAF's DAG execution model. Triggers fire on data changes, actions
//! execute in response, and workflows chain multiple actions into
//! sequentially (or parallelly) executed pipelines.
//!
//! # Architecture
//!
//! ```text
//! Mutation ─► EventBus ─► TriggerEvaluator ─► WorkflowEngine ─► ActionExecutor
//!                │                                                    │
//!                └── EventLog                          WorkflowRun ◄──┘
//! ```
//!
//! Automations are stored as EAV triples: `automation:{uuid}` entities
//! with attributes for trigger config, workflow steps, and metadata.
//!
//! # Modules
//!
//! - [`trigger`]   — Trigger definitions and evaluation
//! - [`action`]    — Action definitions and executors
//! - [`workflow`]  — Workflow DAG execution engine
//! - [`event_bus`] — Internal event broadcast system
//! - [`handlers`]  — REST API endpoints for CRUD and manual invocation

pub mod action;
pub mod event_bus;
pub mod handlers;
pub mod trigger;
pub mod workflow;

pub use action::{ActionConfig, ActionContext, ActionExecutor, ActionId, ActionKind, ActionResult};
pub use event_bus::{DdbEvent, EventBus, EventSubscriber};
pub use handlers::automation_routes;
pub use trigger::{CronExpr, TriggerConfig, TriggerEvaluator, TriggerId, TriggerKind};
pub use workflow::{
    ErrorStrategy, Workflow, WorkflowEngine, WorkflowId, WorkflowRun, WorkflowStep,
};
