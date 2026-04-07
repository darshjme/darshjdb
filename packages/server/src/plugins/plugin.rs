//! Core plugin types and traits.
//!
//! Every DarshJDB plugin implements the [`Plugin`] trait, which provides
//! lifecycle hooks (initialize, shutdown, health check) and exposes a
//! [`PluginManifest`] describing its identity, version, and capabilities.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Plugin identity
// ---------------------------------------------------------------------------

/// A plugin's unique identifier (UUID v4).
pub type PluginId = Uuid;

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

/// Declares a single capability a plugin provides.
///
/// The inner `String` is the capability-specific name — e.g. a custom
/// field type name, a custom view identifier, or an API extension path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum PluginCapability {
    /// Registers a new field type (e.g. `"color_picker"`, `"rating"`).
    CustomField(String),
    /// Registers a custom view (e.g. `"kanban"`, `"timeline"`).
    CustomView(String),
    /// Registers a custom automation action (e.g. `"send_email"`).
    CustomAction(String),
    /// Mounts additional API routes under `/api/ext/{name}`.
    ApiExtension(String),
    /// Registers an incoming webhook endpoint.
    Webhook(String),
    /// Injects middleware into the request pipeline.
    Middleware(String),
}

impl PluginCapability {
    /// Returns the discriminant as a string for capability-based queries.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::CustomField(_) => "custom_field",
            Self::CustomView(_) => "custom_view",
            Self::CustomAction(_) => "custom_action",
            Self::ApiExtension(_) => "api_extension",
            Self::Webhook(_) => "webhook",
            Self::Middleware(_) => "middleware",
        }
    }

    /// Returns the inner capability name.
    pub fn name(&self) -> &str {
        match self {
            Self::CustomField(n)
            | Self::CustomView(n)
            | Self::CustomAction(n)
            | Self::ApiExtension(n)
            | Self::Webhook(n)
            | Self::Middleware(n) => n,
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin state
// ---------------------------------------------------------------------------

/// Lifecycle state of a plugin instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum PluginState {
    /// Plugin is installed but not yet active.
    Installed,
    /// Plugin is running and handling events.
    Active,
    /// Plugin has been explicitly disabled by an admin.
    Disabled,
    /// Plugin encountered an error and is not operational.
    Error(String),
}

impl fmt::Display for PluginState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installed => write!(f, "installed"),
            Self::Active => write!(f, "active"),
            Self::Disabled => write!(f, "disabled"),
            Self::Error(msg) => write!(f, "error: {msg}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin manifest
// ---------------------------------------------------------------------------

/// Declarative metadata describing a plugin.
///
/// The manifest is loaded from a YAML or JSON file in the plugin
/// directory, or constructed programmatically for native plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique identifier (UUID v4).
    pub id: PluginId,
    /// Human-readable plugin name.
    pub name: String,
    /// Semantic version string (e.g. `"1.2.0"`).
    pub version: String,
    /// Plugin author name or organization.
    pub author: String,
    /// Short description of what the plugin does.
    pub description: String,
    /// Optional homepage / docs URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    /// Capabilities this plugin provides.
    pub capabilities: Vec<PluginCapability>,
    /// JSON Schema describing the plugin's configuration surface.
    /// Clients use this to render a settings form.
    #[serde(default = "default_config_schema")]
    pub config_schema: Value,
    /// Entry point for loading the plugin:
    /// - Native: Rust struct name (informational).
    /// - Script: path to `.ts`/`.js` file relative to plugin dir.
    /// - WASM: path to `.wasm` file.
    #[serde(default)]
    pub entry_point: String,
}

fn default_config_schema() -> Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

// ---------------------------------------------------------------------------
// Plugin trait
// ---------------------------------------------------------------------------

/// The core interface every DarshJDB plugin must implement.
///
/// Methods return boxed futures for dyn-compatibility (async fn in
/// trait is not object-safe). Plugins must be `Send + Sync` because
/// the registry and hook system are shared across tokio tasks.
pub trait Plugin: Send + Sync {
    /// Return the plugin's manifest.
    fn manifest(&self) -> &PluginManifest;

    /// Initialize the plugin with the given configuration.
    ///
    /// Called once when the plugin is activated. The `config` value
    /// conforms to the plugin's `config_schema`.
    fn initialize(
        &mut self,
        config: Value,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    /// Gracefully shut down the plugin.
    ///
    /// Called when the plugin is deactivated or the server is stopping.
    fn shutdown(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;

    /// Report current health / lifecycle state.
    fn health(&self) -> PluginState;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_capability_kind() {
        assert_eq!(
            PluginCapability::CustomField("rating".into()).kind(),
            "custom_field"
        );
        assert_eq!(
            PluginCapability::ApiExtension("analytics".into()).kind(),
            "api_extension"
        );
        assert_eq!(
            PluginCapability::Webhook("stripe".into()).kind(),
            "webhook"
        );
    }

    #[test]
    fn plugin_capability_name() {
        let cap = PluginCapability::CustomView("kanban".into());
        assert_eq!(cap.name(), "kanban");
    }

    #[test]
    fn plugin_state_display() {
        assert_eq!(PluginState::Active.to_string(), "active");
        assert_eq!(PluginState::Installed.to_string(), "installed");
        assert_eq!(PluginState::Disabled.to_string(), "disabled");
        assert_eq!(
            PluginState::Error("crash".into()).to_string(),
            "error: crash"
        );
    }

    #[test]
    fn manifest_serde_roundtrip() {
        let manifest = PluginManifest {
            id: Uuid::new_v4(),
            name: "test-plugin".into(),
            version: "0.1.0".into(),
            author: "DarshJ".into(),
            description: "A test plugin".into(),
            homepage: Some("https://darshj.me".into()),
            capabilities: vec![
                PluginCapability::CustomField("color".into()),
                PluginCapability::CustomAction("notify".into()),
            ],
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "api_key": { "type": "string" }
                }
            }),
            entry_point: "main.ts".into(),
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let deserialized: PluginManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, manifest.id);
        assert_eq!(deserialized.name, "test-plugin");
        assert_eq!(deserialized.capabilities.len(), 2);
        assert_eq!(deserialized.homepage.as_deref(), Some("https://darshj.me"));
    }

    #[test]
    fn manifest_default_config_schema() {
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "minimal",
            "version": "1.0.0",
            "author": "test",
            "description": "minimal plugin",
            "capabilities": []
        }"#;

        let manifest: PluginManifest = serde_json::from_str(json).unwrap();
        assert_eq!(manifest.config_schema["type"], "object");
        assert_eq!(manifest.entry_point, "");
    }

    #[test]
    fn plugin_capability_serde_tagged() {
        let cap = PluginCapability::CustomField("rating".into());
        let json = serde_json::to_value(&cap).unwrap();
        assert_eq!(json["kind"], "custom_field");
        assert_eq!(json["name"], "rating");

        let back: PluginCapability = serde_json::from_value(json).unwrap();
        assert_eq!(back, cap);
    }
}
