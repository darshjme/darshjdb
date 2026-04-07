//! Plugin registry — thread-safe storage and lifecycle management.
//!
//! The [`PluginRegistry`] holds all installed plugins (manifests and
//! trait objects), manages activation/deactivation, and provides
//! capability-based lookup for the hook system and API layer.

use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;
use tracing::{error, info, warn};

use super::hooks::HookRegistry;
use super::plugin::{Plugin, PluginId, PluginManifest, PluginState};

// ---------------------------------------------------------------------------
// Plugin entry (manifest + live instance)
// ---------------------------------------------------------------------------

/// An installed plugin: its manifest, current state, configuration,
/// and optional live instance.
pub struct PluginEntry {
    /// The plugin's declared manifest.
    pub manifest: PluginManifest,
    /// Current lifecycle state.
    pub state: PluginState,
    /// Active configuration (set via `/api/plugins/{id}/configure`).
    pub config: Value,
    /// The live plugin instance (`None` when installed but not active).
    pub instance: Option<Box<dyn Plugin>>,
}

// ---------------------------------------------------------------------------
// Plugin registry
// ---------------------------------------------------------------------------

/// Thread-safe registry of all installed plugins.
///
/// Uses [`DashMap`] for lock-free concurrent reads with fine-grained
/// write locking per entry — matching the pattern used by the function
/// registry and other DarshJDB subsystems.
pub struct PluginRegistry {
    /// Installed plugins keyed by plugin ID.
    plugins: DashMap<PluginId, PluginEntry>,
    /// Shared hook registry for cross-cutting lifecycle hooks.
    hook_registry: Arc<HookRegistry>,
}

impl PluginRegistry {
    /// Create a new empty plugin registry with the given hook registry.
    pub fn new(hook_registry: Arc<HookRegistry>) -> Self {
        Self {
            plugins: DashMap::new(),
            hook_registry,
        }
    }

    /// Return a reference to the underlying hook registry.
    pub fn hooks(&self) -> &Arc<HookRegistry> {
        &self.hook_registry
    }

    // -----------------------------------------------------------------------
    // CRUD
    // -----------------------------------------------------------------------

    /// Register (install) a plugin by its manifest.
    ///
    /// The plugin starts in [`PluginState::Installed`]. Call [`activate`]
    /// to start it.
    pub fn register(&self, manifest: PluginManifest) {
        let id = manifest.id;
        let name = manifest.name.clone();

        self.plugins.insert(
            id,
            PluginEntry {
                manifest,
                state: PluginState::Installed,
                config: serde_json::json!({}),
                instance: None,
            },
        );

        info!(plugin_id = %id, name = %name, "plugin registered");
    }

    /// Register a plugin with a live trait object instance.
    ///
    /// The manifest is extracted from the instance. The plugin starts
    /// in [`PluginState::Installed`].
    pub fn register_with_instance(&self, plugin: Box<dyn Plugin>) {
        let manifest = plugin.manifest().clone();
        let id = manifest.id;
        let name = manifest.name.clone();

        self.plugins.insert(
            id,
            PluginEntry {
                manifest,
                state: PluginState::Installed,
                config: serde_json::json!({}),
                instance: Some(plugin),
            },
        );

        info!(plugin_id = %id, name = %name, "plugin registered with instance");
    }

    /// Unregister (uninstall) a plugin.
    ///
    /// If the plugin is active, it is shut down first. Returns `true`
    /// if the plugin was found and removed.
    pub async fn unregister(&self, plugin_id: PluginId) -> bool {
        // Shut down if active.
        if let Some(mut entry) = self.plugins.get_mut(&plugin_id) {
            if matches!(entry.state, PluginState::Active) {
                if let Some(ref mut instance) = entry.instance {
                    instance.shutdown().await;
                }
                entry.state = PluginState::Disabled;
            }
        }

        // Remove hook handlers.
        self.hook_registry.unregister_plugin(plugin_id).await;

        let removed = self.plugins.remove(&plugin_id).is_some();
        if removed {
            info!(plugin_id = %plugin_id, "plugin unregistered");
        } else {
            warn!(plugin_id = %plugin_id, "attempted to unregister unknown plugin");
        }
        removed
    }

    /// Get a plugin's manifest by ID.
    pub fn get(&self, plugin_id: PluginId) -> Option<PluginManifest> {
        self.plugins.get(&plugin_id).map(|e| e.manifest.clone())
    }

    /// Get a plugin's current state.
    pub fn get_state(&self, plugin_id: PluginId) -> Option<PluginState> {
        self.plugins.get(&plugin_id).map(|e| e.state.clone())
    }

    /// Get a plugin's current configuration.
    pub fn get_config(&self, plugin_id: PluginId) -> Option<Value> {
        self.plugins.get(&plugin_id).map(|e| e.config.clone())
    }

    /// List all installed plugin manifests.
    pub fn list(&self) -> Vec<PluginManifest> {
        self.plugins.iter().map(|e| e.manifest.clone()).collect()
    }

    /// List manifests of plugins that provide a specific capability kind.
    ///
    /// `cap_kind` is one of: `"custom_field"`, `"custom_view"`,
    /// `"custom_action"`, `"api_extension"`, `"webhook"`, `"middleware"`.
    pub fn list_by_capability(&self, cap_kind: &str) -> Vec<PluginManifest> {
        self.plugins
            .iter()
            .filter(|e| e.manifest.capabilities.iter().any(|c| c.kind() == cap_kind))
            .map(|e| e.manifest.clone())
            .collect()
    }

    /// Return the total number of installed plugins.
    pub fn count(&self) -> usize {
        self.plugins.len()
    }

    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    /// Activate a plugin, calling its `initialize` method with the
    /// stored configuration.
    pub async fn activate(&self, plugin_id: PluginId) -> Result<(), String> {
        let config = {
            let entry = self
                .plugins
                .get(&plugin_id)
                .ok_or_else(|| format!("plugin {plugin_id} not found"))?;

            if matches!(entry.state, PluginState::Active) {
                return Ok(()); // Already active.
            }

            entry.config.clone()
        };

        // Take the instance out, initialize it, put it back.
        let mut entry = self
            .plugins
            .get_mut(&plugin_id)
            .ok_or_else(|| format!("plugin {plugin_id} not found"))?;

        if let Some(ref mut instance) = entry.instance {
            match instance.initialize(config).await {
                Ok(()) => {
                    entry.state = PluginState::Active;
                    let name = entry.manifest.name.clone();
                    info!(plugin_id = %plugin_id, name = %name, "plugin activated");
                    Ok(())
                }
                Err(e) => {
                    entry.state = PluginState::Error(e.clone());
                    error!(plugin_id = %plugin_id, error = %e, "plugin activation failed");
                    Err(e)
                }
            }
        } else {
            // Manifest-only registration (script/WASM plugins) — mark
            // as active without a Rust instance.
            entry.state = PluginState::Active;
            info!(plugin_id = %plugin_id, "plugin activated (manifest-only)");
            Ok(())
        }
    }

    /// Deactivate a plugin, calling its `shutdown` method.
    pub async fn deactivate(&self, plugin_id: PluginId) -> Result<(), String> {
        let mut entry = self
            .plugins
            .get_mut(&plugin_id)
            .ok_or_else(|| format!("plugin {plugin_id} not found"))?;

        if !matches!(entry.state, PluginState::Active) {
            return Ok(()); // Not active, nothing to do.
        }

        if let Some(ref mut instance) = entry.instance {
            instance.shutdown().await;
        }

        // Remove hooks registered by this plugin.
        self.hook_registry.unregister_plugin(plugin_id).await;

        entry.state = PluginState::Disabled;
        let name = entry.manifest.name.clone();
        info!(plugin_id = %plugin_id, name = %name, "plugin deactivated");
        Ok(())
    }

    /// Update a plugin's configuration.
    ///
    /// If the plugin is currently active, it is re-initialized with
    /// the new config.
    pub async fn configure(&self, plugin_id: PluginId, config: Value) -> Result<(), String> {
        let was_active = {
            let mut entry = self
                .plugins
                .get_mut(&plugin_id)
                .ok_or_else(|| format!("plugin {plugin_id} not found"))?;
            let was_active = matches!(entry.state, PluginState::Active);
            entry.config = config;
            was_active
        };

        // If active, re-initialize with new config.
        if was_active {
            self.deactivate(plugin_id).await?;
            self.activate(plugin_id).await?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::plugin::PluginCapability;
    use uuid::Uuid;

    fn make_manifest(name: &str, caps: Vec<PluginCapability>) -> PluginManifest {
        PluginManifest {
            id: Uuid::new_v4(),
            name: name.into(),
            version: "1.0.0".into(),
            author: "test".into(),
            description: format!("Test plugin: {name}"),
            homepage: None,
            capabilities: caps,
            config_schema: serde_json::json!({}),
            entry_point: String::new(),
        }
    }

    #[test]
    fn register_and_get() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let manifest = make_manifest("test", vec![]);
        let id = manifest.id;

        registry.register(manifest.clone());

        let fetched = registry.get(id).unwrap();
        assert_eq!(fetched.name, "test");
        assert_eq!(registry.count(), 1);
    }

    #[tokio::test]
    async fn unregister_removes_plugin() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let manifest = make_manifest("to-remove", vec![]);
        let id = manifest.id;

        registry.register(manifest);
        assert!(registry.unregister(id).await);
        assert!(registry.get(id).is_none());
        assert_eq!(registry.count(), 0);
    }

    #[tokio::test]
    async fn unregister_unknown_returns_false() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        assert!(!registry.unregister(Uuid::new_v4()).await);
    }

    #[test]
    fn list_by_capability() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);

        let m1 = make_manifest(
            "field-plugin",
            vec![PluginCapability::CustomField("color".into())],
        );
        let m2 = make_manifest(
            "view-plugin",
            vec![PluginCapability::CustomView("kanban".into())],
        );
        let m3 = make_manifest(
            "multi-plugin",
            vec![
                PluginCapability::CustomField("rating".into()),
                PluginCapability::CustomAction("export".into()),
            ],
        );

        registry.register(m1);
        registry.register(m2);
        registry.register(m3);

        let field_plugins = registry.list_by_capability("custom_field");
        assert_eq!(field_plugins.len(), 2);

        let view_plugins = registry.list_by_capability("custom_view");
        assert_eq!(view_plugins.len(), 1);

        let action_plugins = registry.list_by_capability("custom_action");
        assert_eq!(action_plugins.len(), 1);

        let webhook_plugins = registry.list_by_capability("webhook");
        assert!(webhook_plugins.is_empty());
    }

    #[test]
    fn list_all() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);

        registry.register(make_manifest("a", vec![]));
        registry.register(make_manifest("b", vec![]));

        let all = registry.list();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn get_state_initial() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let manifest = make_manifest("stateful", vec![]);
        let id = manifest.id;

        registry.register(manifest);
        assert_eq!(registry.get_state(id).unwrap(), PluginState::Installed);
    }

    #[tokio::test]
    async fn activate_manifest_only() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let manifest = make_manifest("script-plugin", vec![]);
        let id = manifest.id;

        registry.register(manifest);
        registry.activate(id).await.unwrap();
        assert_eq!(registry.get_state(id).unwrap(), PluginState::Active);
    }

    #[tokio::test]
    async fn deactivate_sets_disabled() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let manifest = make_manifest("to-disable", vec![]);
        let id = manifest.id;

        registry.register(manifest);
        registry.activate(id).await.unwrap();
        registry.deactivate(id).await.unwrap();
        assert_eq!(registry.get_state(id).unwrap(), PluginState::Disabled);
    }

    #[tokio::test]
    async fn configure_updates_config() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let manifest = make_manifest("configurable", vec![]);
        let id = manifest.id;

        registry.register(manifest);

        let new_config = serde_json::json!({ "api_key": "sk-123" });
        registry.configure(id, new_config.clone()).await.unwrap();

        let stored = registry.get_config(id).unwrap();
        assert_eq!(stored, new_config);
    }

    #[tokio::test]
    async fn activate_unknown_errors() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let result = registry.activate(Uuid::new_v4()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn deactivate_unknown_errors() {
        let hooks = Arc::new(HookRegistry::new());
        let registry = PluginRegistry::new(hooks);
        let result = registry.deactivate(Uuid::new_v4()).await;
        assert!(result.is_err());
    }
}
