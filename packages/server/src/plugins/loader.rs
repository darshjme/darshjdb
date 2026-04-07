//! Plugin loading — manifest parsing and runtime loaders.
//!
//! Supports three loading strategies:
//!
//! - **Native**: Rust plugins registered as trait objects at compile time.
//! - **Script**: TypeScript/JavaScript plugins using the existing function
//!   runtime (reuses the `functions` module infrastructure).
//! - **WASM**: WebAssembly plugins loaded from `.wasm` files (future).
//!
//! The [`ManifestLoader`] parses plugin manifests from YAML or JSON files,
//! and the [`PluginDirectoryWatcher`] monitors a directory for hot-reload.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tracing::{debug, error, info, warn};

use super::plugin::{Plugin, PluginManifest};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors during plugin loading.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// The plugin directory does not exist.
    #[error("plugin directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    /// A manifest file could not be read.
    #[error("failed to read manifest at {path}: {source}")]
    ManifestReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A manifest file could not be parsed.
    #[error("failed to parse manifest at {path}: {reason}")]
    ManifestParseError { path: PathBuf, reason: String },

    /// The plugin entry point file was not found.
    #[error("entry point not found: {0}")]
    EntryPointNotFound(PathBuf),

    /// WASM loading is not yet supported.
    #[error("WASM plugin loading is not yet implemented")]
    WasmNotSupported,

    /// A filesystem watcher error.
    #[error("filesystem watcher error: {0}")]
    WatcherError(String),
}

/// Result alias for loader operations.
pub type LoaderResult<T> = std::result::Result<T, LoaderError>;

// ---------------------------------------------------------------------------
// Manifest loader
// ---------------------------------------------------------------------------

/// Loads plugin manifests from JSON or YAML files.
pub struct ManifestLoader;

impl ManifestLoader {
    /// Load a plugin manifest from a JSON file.
    pub async fn load_json(path: &Path) -> LoaderResult<PluginManifest> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| LoaderError::ManifestReadError {
                path: path.to_path_buf(),
                source: e,
            })?;

        serde_json::from_str::<PluginManifest>(&content).map_err(|e| {
            LoaderError::ManifestParseError {
                path: path.to_path_buf(),
                reason: e.to_string(),
            }
        })
    }

    /// Load a plugin manifest from a YAML file.
    ///
    /// YAML is parsed as JSON via serde_json's `Value` — YAML support
    /// requires the content to be valid JSON-compatible YAML (no anchors,
    /// no multi-document). For full YAML support, add `serde_yaml` to deps.
    pub async fn load_yaml(path: &Path) -> LoaderResult<PluginManifest> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| LoaderError::ManifestReadError {
                path: path.to_path_buf(),
                source: e,
            })?;

        // Attempt JSON-compatible YAML parse via serde_json.
        // For production YAML support, integrate serde_yaml.
        serde_json::from_str::<PluginManifest>(&content).map_err(|e| {
            LoaderError::ManifestParseError {
                path: path.to_path_buf(),
                reason: format!("YAML parse (via JSON fallback): {e}"),
            }
        })
    }

    /// Auto-detect format from file extension and load.
    pub async fn load(path: &Path) -> LoaderResult<PluginManifest> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Self::load_json(path).await,
            Some("yaml" | "yml") => Self::load_yaml(path).await,
            _ => Err(LoaderError::ManifestParseError {
                path: path.to_path_buf(),
                reason: "unsupported manifest format (expected .json or .yaml/.yml)".into(),
            }),
        }
    }

    /// Scan a directory for manifest files (`plugin.json`, `plugin.yaml`,
    /// `plugin.yml`) and load all discovered manifests.
    pub async fn scan_directory(dir: &Path) -> LoaderResult<Vec<PluginManifest>> {
        if !dir.is_dir() {
            return Err(LoaderError::DirectoryNotFound(dir.to_path_buf()));
        }

        let mut manifests = Vec::new();
        let mut read_dir = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| LoaderError::ManifestReadError {
                path: dir.to_path_buf(),
                source: e,
            })?;

        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| LoaderError::ManifestReadError {
                path: dir.to_path_buf(),
                source: e,
            })?
        {
            let path = entry.path();

            if path.is_dir() {
                // Look for manifest file inside subdirectory.
                for name in ["plugin.json", "plugin.yaml", "plugin.yml"] {
                    let manifest_path = path.join(name);
                    if manifest_path.is_file() {
                        match Self::load(&manifest_path).await {
                            Ok(manifest) => {
                                debug!(
                                    plugin = %manifest.name,
                                    path = %manifest_path.display(),
                                    "discovered plugin manifest"
                                );
                                manifests.push(manifest);
                            }
                            Err(e) => {
                                warn!(
                                    path = %manifest_path.display(),
                                    error = %e,
                                    "skipping invalid plugin manifest"
                                );
                            }
                        }
                        break; // Only load the first manifest per directory.
                    }
                }
            }
        }

        info!(count = manifests.len(), dir = %dir.display(), "plugin directory scan complete");
        Ok(manifests)
    }
}

// ---------------------------------------------------------------------------
// Native plugin loader
// ---------------------------------------------------------------------------

/// Loads native Rust plugins via trait objects.
///
/// Native plugins are compiled into the server binary and registered
/// at startup. This loader is a thin wrapper for type-safe registration.
pub struct NativePluginLoader;

impl NativePluginLoader {
    /// "Load" a native plugin — simply wraps the instance for consistency
    /// with the loader interface.
    pub fn load(plugin: Box<dyn Plugin>) -> LoaderResult<(PluginManifest, Box<dyn Plugin>)> {
        let manifest = plugin.manifest().clone();
        info!(
            plugin = %manifest.name,
            version = %manifest.version,
            "native plugin loaded"
        );
        Ok((manifest, plugin))
    }
}

// ---------------------------------------------------------------------------
// Script plugin loader (stub — delegates to function runtime)
// ---------------------------------------------------------------------------

/// Loads script-based plugins (TypeScript/JavaScript).
///
/// Delegates to the existing `functions` runtime for JS/TS execution.
/// Script plugins export a `plugin` object conforming to the Plugin
/// interface.
pub struct ScriptPluginLoader {
    /// Root directory for script plugins.
    plugins_dir: PathBuf,
}

impl ScriptPluginLoader {
    /// Create a new script loader pointing at the given directory.
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self { plugins_dir }
    }

    /// Load a script plugin by manifest.
    ///
    /// Verifies the entry point file exists. Actual execution happens
    /// through the function runtime at invocation time.
    pub async fn load(&self, manifest: &PluginManifest) -> LoaderResult<()> {
        let entry_path = self.plugins_dir.join(&manifest.entry_point);

        if !entry_path.is_file() {
            return Err(LoaderError::EntryPointNotFound(entry_path));
        }

        let ext = entry_path.extension().and_then(|e| e.to_str());
        match ext {
            Some("ts" | "js" | "mts" | "mjs") => {
                info!(
                    plugin = %manifest.name,
                    entry_point = %manifest.entry_point,
                    "script plugin entry point verified"
                );
                Ok(())
            }
            _ => Err(LoaderError::EntryPointNotFound(entry_path)),
        }
    }
}

// ---------------------------------------------------------------------------
// WASM plugin loader (future — stub)
// ---------------------------------------------------------------------------

/// Loads WebAssembly plugins from `.wasm` files.
///
/// This is a forward-looking stub. WASM plugin support will use
/// `wasmtime` or `wasmer` as the runtime, with a WASI-compatible
/// host interface for database access and hook registration.
pub struct WasmPluginLoader;

impl WasmPluginLoader {
    /// Attempt to load a WASM plugin.
    ///
    /// Currently returns an error — WASM support is planned for a
    /// future release.
    pub async fn load(_wasm_path: &Path) -> LoaderResult<()> {
        error!("WASM plugin loading is not yet implemented");
        Err(LoaderError::WasmNotSupported)
    }
}

// ---------------------------------------------------------------------------
// Plugin directory watcher (hot-reload)
// ---------------------------------------------------------------------------

/// Watches a plugin directory for changes and triggers reloads.
///
/// Uses the `notify` crate (same as the function registry) with
/// debounced event handling.
pub struct PluginDirectoryWatcher {
    plugins_dir: PathBuf,
    _watcher: Option<notify::RecommendedWatcher>,
}

impl PluginDirectoryWatcher {
    /// Create a watcher for the given plugin directory.
    ///
    /// Does not start watching — call [`Self::start`] to begin.
    pub fn new(plugins_dir: PathBuf) -> Self {
        Self {
            plugins_dir,
            _watcher: None,
        }
    }

    /// Start watching the plugin directory for manifest changes.
    ///
    /// When a `plugin.json` or `plugin.yaml` file is created, modified,
    /// or deleted, the `on_change` callback is invoked with the path.
    pub fn start<F>(&mut self, on_change: F) -> LoaderResult<()>
    where
        F: Fn(PathBuf) + Send + Sync + 'static,
    {
        use notify::{Event, EventKind, RecursiveMode, Watcher};

        let watcher = notify::recommended_watcher(move |result: Result<Event, notify::Error>| {
            match result {
                Ok(event) => {
                    let dominated = matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    );

                    if dominated {
                        for path in event.paths {
                            if is_manifest_file(&path) {
                                on_change(path);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "plugin directory watcher error");
                }
            }
        })
        .map_err(|e| LoaderError::WatcherError(e.to_string()))?;

        let mut watcher = watcher;
        watcher
            .watch(&self.plugins_dir, RecursiveMode::Recursive)
            .map_err(|e| LoaderError::WatcherError(e.to_string()))?;

        self._watcher = Some(watcher);
        info!(dir = %self.plugins_dir.display(), "plugin directory watcher started");
        Ok(())
    }
}

/// Check if a path looks like a plugin manifest file.
fn is_manifest_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    matches!(name, "plugin.json" | "plugin.yaml" | "plugin.yml")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// Create a unique temp directory for each test.
    fn make_temp_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("darshjdb_plugin_test")
            .join(format!("{}_{}", suffix, Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Clean up a temp directory (best-effort).
    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    fn sample_manifest_json() -> String {
        serde_json::json!({
            "id": Uuid::new_v4(),
            "name": "test-plugin",
            "version": "1.0.0",
            "author": "DarshJ",
            "description": "A test plugin",
            "capabilities": [
                { "kind": "custom_field", "name": "rating" }
            ],
            "entry_point": "main.ts"
        })
        .to_string()
    }

    #[tokio::test]
    async fn load_json_manifest() {
        let dir = make_temp_dir("load_json");
        let path = dir.join("plugin.json");
        std::fs::write(&path, sample_manifest_json()).unwrap();

        let manifest = ManifestLoader::load_json(&path).await.unwrap();
        assert_eq!(manifest.name, "test-plugin");
        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.capabilities.len(), 1);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn load_auto_detect_json() {
        let dir = make_temp_dir("auto_detect");
        let path = dir.join("plugin.json");
        std::fs::write(&path, sample_manifest_json()).unwrap();

        let manifest = ManifestLoader::load(&path).await.unwrap();
        assert_eq!(manifest.name, "test-plugin");
        cleanup(&dir);
    }

    #[tokio::test]
    async fn load_unsupported_format() {
        let dir = make_temp_dir("unsupported");
        let path = dir.join("plugin.toml");
        std::fs::write(&path, "name = 'test'").unwrap();

        let err = ManifestLoader::load(&path).await.unwrap_err();
        assert!(matches!(err, LoaderError::ManifestParseError { .. }));
        cleanup(&dir);
    }

    #[tokio::test]
    async fn load_missing_file() {
        let path = PathBuf::from("/nonexistent/plugin.json");
        let err = ManifestLoader::load_json(&path).await.unwrap_err();
        assert!(matches!(err, LoaderError::ManifestReadError { .. }));
    }

    #[tokio::test]
    async fn scan_directory_finds_plugins() {
        let dir = make_temp_dir("scan");

        // Create two plugin subdirectories.
        let plugin_a = dir.join("plugin-a");
        std::fs::create_dir(&plugin_a).unwrap();
        std::fs::write(plugin_a.join("plugin.json"), sample_manifest_json()).unwrap();

        let plugin_b = dir.join("plugin-b");
        std::fs::create_dir(&plugin_b).unwrap();
        let mut manifest_b = serde_json::from_str::<Value>(&sample_manifest_json()).unwrap();
        manifest_b["id"] = serde_json::json!(Uuid::new_v4());
        manifest_b["name"] = serde_json::json!("plugin-b");
        std::fs::write(
            plugin_b.join("plugin.json"),
            serde_json::to_string(&manifest_b).unwrap(),
        )
        .unwrap();

        let manifests = ManifestLoader::scan_directory(&dir).await.unwrap();
        assert_eq!(manifests.len(), 2);
        cleanup(&dir);
    }

    #[tokio::test]
    async fn scan_nonexistent_directory() {
        let err = ManifestLoader::scan_directory(Path::new("/nonexistent"))
            .await
            .unwrap_err();
        assert!(matches!(err, LoaderError::DirectoryNotFound(_)));
    }

    #[tokio::test]
    async fn wasm_loader_returns_not_supported() {
        let err = WasmPluginLoader::load(Path::new("test.wasm"))
            .await
            .unwrap_err();
        assert!(matches!(err, LoaderError::WasmNotSupported));
    }

    #[test]
    fn is_manifest_file_checks() {
        assert!(is_manifest_file(Path::new("/plugins/foo/plugin.json")));
        assert!(is_manifest_file(Path::new("/plugins/bar/plugin.yaml")));
        assert!(is_manifest_file(Path::new("plugin.yml")));
        assert!(!is_manifest_file(Path::new("package.json")));
        assert!(!is_manifest_file(Path::new("main.ts")));
    }
}
