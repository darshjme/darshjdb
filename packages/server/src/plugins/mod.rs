//! Plugin/Extension SDK for DarshJDB.
//!
//! Provides a system for extending DarshJDB with custom field types,
//! custom views, custom automation actions, and API extensions. Plugins
//! are registered at runtime and participate in the request lifecycle
//! through a typed hook system.
//!
//! # Architecture
//!
//! - [`Plugin`] trait — core interface every plugin implements.
//! - [`PluginRegistry`] — thread-safe registry of installed plugins.
//! - [`HookRegistry`] — ordered hook dispatch for lifecycle events.
//! - Loaders — native (trait objects), script (.ts/.js), WASM (future).
//! - [`handlers`] — REST endpoints for plugin CRUD and configuration.
//! - [`builtin`] — reference plugins (Slack, validation, audit log).

pub mod builtin;
pub mod handlers;
pub mod hooks;
pub mod loader;
pub mod plugin;
pub mod registry;

pub use hooks::{Hook, HookContext, HookHandler, HookRegistry, HookResult};
pub use plugin::{Plugin, PluginCapability, PluginId, PluginManifest, PluginState};
pub use registry::PluginRegistry;
