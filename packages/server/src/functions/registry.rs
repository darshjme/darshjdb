//! Function registry — discovery, indexing, and hot reload.
//!
//! Scans a `darshan/functions/` directory for `.ts` and `.js` files, parses
//! their exports to build a [`FunctionDef`] registry, and optionally watches
//! for filesystem changes to hot-reload in development mode.
//!
//! # Discovery
//!
//! Each file in the functions directory is scanned for named exports. An
//! export like `export const getUser = query({ ... })` produces a
//! [`FunctionDef`] with `kind: Query`, `export_name: "getUser"`, and
//! `file_path` relative to the functions root.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info};

use super::validator::ArgSchema;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during registry operations.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// The functions directory does not exist or is not readable.
    #[error("functions directory not found: {0}")]
    DirectoryNotFound(PathBuf),

    /// A function file could not be read.
    #[error("failed to read function file {path}: {source}")]
    FileReadError {
        /// Path to the file.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse exports from a function file.
    #[error("failed to parse exports in {path}: {reason}")]
    ParseError {
        /// Path to the file.
        path: PathBuf,
        /// What went wrong.
        reason: String,
    },

    /// A duplicate function name was found.
    #[error("duplicate function name `{name}` in {path1} and {path2}")]
    DuplicateName {
        /// The conflicting name.
        name: String,
        /// First file defining this name.
        path1: PathBuf,
        /// Second file defining this name.
        path2: PathBuf,
    },

    /// Failed to set up the filesystem watcher.
    #[error("filesystem watcher error: {0}")]
    WatcherError(#[from] notify::Error),
}

/// Result alias for registry operations.
pub type RegistryResult<T> = std::result::Result<T, RegistryError>;

// ---------------------------------------------------------------------------
// Function definitions
// ---------------------------------------------------------------------------

/// The kind of server function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FunctionKind {
    /// A read-only database query. Automatically cached and reactive.
    Query,
    /// A write operation that runs inside a transaction.
    Mutation,
    /// A general-purpose function that can have side effects (network, etc.).
    Action,
    /// A function triggered on a cron schedule.
    Scheduled,
    /// An internal function callable only from other server functions.
    Internal,
    /// An HTTP endpoint handler (`GET`, `POST`, etc.).
    HttpEndpoint,
}

impl FunctionKind {
    /// Returns the DarshJDB wrapper function name that declares this kind
    /// (e.g. `"query"`, `"mutation"`), used during export parsing.
    pub fn wrapper_name(&self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Mutation => "mutation",
            Self::Action => "action",
            Self::Scheduled => "scheduled",
            Self::Internal => "internalFn",
            Self::HttpEndpoint => "httpEndpoint",
        }
    }

    /// Try to parse a wrapper function name into a [`FunctionKind`].
    pub fn from_wrapper_name(name: &str) -> Option<Self> {
        match name {
            "query" => Some(Self::Query),
            "mutation" => Some(Self::Mutation),
            "action" => Some(Self::Action),
            "scheduled" => Some(Self::Scheduled),
            "internalFn" => Some(Self::Internal),
            "httpEndpoint" => Some(Self::HttpEndpoint),
            _ => None,
        }
    }
}

/// A registered server function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    /// Fully qualified name (e.g. `"users:getUser"`).
    pub name: String,

    /// Path to the source file, relative to the functions directory.
    pub file_path: PathBuf,

    /// The named export within the file (e.g. `"getUser"`).
    pub export_name: String,

    /// What kind of function this is.
    pub kind: FunctionKind,

    /// Optional argument schema for validation.
    pub args_schema: Option<ArgSchema>,

    /// Human-readable description parsed from JSDoc or metadata.
    pub description: Option<String>,

    /// When this file was last modified (for cache invalidation).
    #[serde(skip)]
    pub last_modified: Option<SystemTime>,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// Thread-safe registry of all discovered server functions.
///
/// Supports atomic reload on filesystem changes and concurrent read access
/// during request handling.
pub struct FunctionRegistry {
    /// The functions directory being watched.
    functions_dir: PathBuf,

    /// Map from fully qualified function name to definition.
    functions: Arc<RwLock<HashMap<String, FunctionDef>>>,

    /// Handle to the filesystem watcher (kept alive for the watcher thread).
    _watcher: Option<RecommendedWatcher>,
}

impl FunctionRegistry {
    /// Create a new registry by scanning the given functions directory.
    ///
    /// Does **not** start the filesystem watcher — call [`Self::enable_hot_reload`]
    /// separately in development mode.
    pub async fn new(functions_dir: PathBuf) -> RegistryResult<Self> {
        if !functions_dir.is_dir() {
            return Err(RegistryError::DirectoryNotFound(functions_dir));
        }

        let functions = Self::scan_directory(&functions_dir).await?;
        info!(
            count = functions.len(),
            dir = %functions_dir.display(),
            "function registry initialized"
        );

        Ok(Self {
            functions_dir,
            functions: Arc::new(RwLock::new(functions)),
            _watcher: None,
        })
    }

    /// Look up a function by its fully qualified name.
    pub async fn get(&self, name: &str) -> Option<FunctionDef> {
        self.functions.read().await.get(name).cloned()
    }

    /// Return all registered function definitions.
    pub async fn list(&self) -> Vec<FunctionDef> {
        self.functions.read().await.values().cloned().collect()
    }

    /// Return the number of registered functions.
    pub async fn count(&self) -> usize {
        self.functions.read().await.len()
    }

    /// Manually trigger a full rescan of the functions directory.
    pub async fn reload(&self) -> RegistryResult<usize> {
        let new_functions = Self::scan_directory(&self.functions_dir).await?;
        let count = new_functions.len();

        let mut lock = self.functions.write().await;
        *lock = new_functions;

        info!(count, "function registry reloaded");
        Ok(count)
    }

    /// Enable hot reload by watching the functions directory for changes.
    ///
    /// File creation, modification, and deletion events trigger an automatic
    /// registry reload. This should only be enabled in development mode.
    pub fn enable_hot_reload(&mut self) -> RegistryResult<()> {
        let _functions_dir = self.functions_dir.clone();
        let functions = Arc::clone(&self.functions);

        let (tx, mut rx) = mpsc::channel::<()>(16);

        let mut watcher =
            notify::recommended_watcher(move |result: Result<Event, notify::Error>| {
                match result {
                    Ok(event) => {
                        let dominated = matches!(
                            event.kind,
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                        );

                        if dominated && has_function_extension(&event.paths) {
                            // Best-effort send — if the channel is full we coalesce.
                            let _ = tx.try_send(());
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "filesystem watcher error");
                    }
                }
            })?;

        watcher.watch(&self.functions_dir, RecursiveMode::Recursive)?;

        // Spawn a background task that debounces and reloads.
        let dir = self.functions_dir.clone();
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                // Debounce: drain any queued events.
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                while rx.try_recv().is_ok() {}

                match Self::scan_directory(&dir).await {
                    Ok(new_functions) => {
                        let count = new_functions.len();
                        let mut lock = functions.write().await;
                        *lock = new_functions;
                        info!(count, "hot reload: function registry updated");
                    }
                    Err(e) => {
                        error!(error = %e, "hot reload: failed to rescan functions directory");
                    }
                }
            }
        });

        self._watcher = Some(watcher);
        info!(dir = %self.functions_dir.display(), "hot reload enabled");
        Ok(())
    }

    /// Scan the functions directory and build a function map.
    async fn scan_directory(dir: &Path) -> RegistryResult<HashMap<String, FunctionDef>> {
        let mut functions = HashMap::new();

        let entries = Self::collect_function_files(dir).await?;

        for entry_path in entries {
            let relative = entry_path
                .strip_prefix(dir)
                .unwrap_or(&entry_path)
                .to_path_buf();

            let content = tokio::fs::read_to_string(&entry_path).await.map_err(|e| {
                RegistryError::FileReadError {
                    path: entry_path.clone(),
                    source: e,
                }
            })?;

            let last_modified = tokio::fs::metadata(&entry_path)
                .await
                .ok()
                .and_then(|m| m.modified().ok());

            let exports = parse_exports(&content, &relative);

            for (export_name, kind, args_schema, description) in exports {
                let module_name = module_name_from_path(&relative);
                let fq_name = format!("{module_name}:{export_name}");

                if let Some(existing) = functions.get(&fq_name) {
                    let existing: &FunctionDef = existing;
                    return Err(RegistryError::DuplicateName {
                        name: fq_name,
                        path1: existing.file_path.clone(),
                        path2: relative,
                    });
                }

                debug!(name = %fq_name, kind = ?kind, "discovered function");

                functions.insert(
                    fq_name.clone(),
                    FunctionDef {
                        name: fq_name,
                        file_path: relative.clone(),
                        export_name,
                        kind,
                        args_schema,
                        description,
                        last_modified,
                    },
                );
            }
        }

        Ok(functions)
    }

    /// Recursively collect all `.ts` and `.js` files in the directory.
    async fn collect_function_files(dir: &Path) -> RegistryResult<Vec<PathBuf>> {
        let mut files = Vec::new();
        let mut stack = vec![dir.to_path_buf()];

        while let Some(current) = stack.pop() {
            let mut read_dir =
                tokio::fs::read_dir(&current)
                    .await
                    .map_err(|e| RegistryError::FileReadError {
                        path: current.clone(),
                        source: e,
                    })?;

            while let Some(entry) =
                read_dir
                    .next_entry()
                    .await
                    .map_err(|e| RegistryError::FileReadError {
                        path: current.clone(),
                        source: e,
                    })?
            {
                let path = entry.path();
                let file_type =
                    entry
                        .file_type()
                        .await
                        .map_err(|e| RegistryError::FileReadError {
                            path: path.clone(),
                            source: e,
                        })?;

                if file_type.is_dir() {
                    // Skip node_modules and hidden directories.
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if !name_str.starts_with('.') && name_str != "node_modules" {
                        stack.push(path);
                    }
                } else if file_type.is_file() && is_function_file(&path) {
                    files.push(path);
                }
            }
        }

        files.sort();
        Ok(files)
    }
}

// ---------------------------------------------------------------------------
// Export parsing
// ---------------------------------------------------------------------------

/// Parse a function file's source code to extract exported function definitions.
///
/// Looks for patterns like:
/// - `export const foo = query({ ... })`
/// - `export const bar = mutation({ ... })`
/// - `export default httpEndpoint({ ... })`
///
/// Returns a list of `(export_name, kind, optional_args_schema, optional_description)`.
fn parse_exports(
    content: &str,
    _file_path: &Path,
) -> Vec<(String, FunctionKind, Option<ArgSchema>, Option<String>)> {
    let mut results = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Match: export const NAME = KIND({
        if let Some(rest) = trimmed.strip_prefix("export const ")
            && let Some((name, after_eq)) = rest.split_once('=')
        {
            let name = name.trim();
            let after_eq = after_eq.trim();

            // Try to match KIND( or KIND({
            for kind in [
                FunctionKind::Query,
                FunctionKind::Mutation,
                FunctionKind::Action,
                FunctionKind::Scheduled,
                FunctionKind::Internal,
                FunctionKind::HttpEndpoint,
            ] {
                let wrapper = kind.wrapper_name();
                if after_eq.starts_with(&format!("{wrapper}("))
                    || after_eq.starts_with(&format!("{wrapper} ("))
                {
                    results.push((name.to_string(), kind, None, None));
                    break;
                }
            }
        }

        // Match: export default KIND({
        if let Some(rest) = trimmed.strip_prefix("export default ") {
            let mut matched = false;
            for kind in [
                FunctionKind::Query,
                FunctionKind::Mutation,
                FunctionKind::Action,
                FunctionKind::Scheduled,
                FunctionKind::Internal,
                FunctionKind::HttpEndpoint,
            ] {
                let wrapper = kind.wrapper_name();
                if rest.starts_with(&format!("{wrapper}("))
                    || rest.starts_with(&format!("{wrapper} ("))
                {
                    results.push(("default".to_string(), kind, None, None));
                    matched = true;
                    break;
                }
            }

            // Match: export default function NAME(...) or export default function(...)
            // Plain exported functions are treated as Action kind.
            if !matched && rest.starts_with("function") {
                let after_kw = &rest["function".len()..];
                if after_kw.starts_with(' ') || after_kw.starts_with('(') {
                    let trimmed = after_kw.trim_start();
                    let name: String = trimmed
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if !name.is_empty() {
                        results.push((name, FunctionKind::Action, None, None));
                    } else {
                        results.push(("default".to_string(), FunctionKind::Action, None, None));
                    }
                }
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check whether any path in the list has a `.ts` or `.js` extension.
fn has_function_extension(paths: &[PathBuf]) -> bool {
    paths.iter().any(|p| is_function_file(p))
}

/// Check if a file path has a recognized function file extension.
fn is_function_file(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts" | "js" | "mts" | "mjs") => {
            // Exclude declaration and test files.
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            !name.ends_with(".d")
                && !name.ends_with(".test")
                && !name.ends_with(".spec")
                && !name.starts_with('_')
        }
        _ => false,
    }
}

/// Derive a module name from a relative file path.
///
/// `users/queries.ts` -> `"users/queries"`
/// `tasks.ts` -> `"tasks"`
fn module_name_from_path(path: &Path) -> String {
    path.with_extension("").to_string_lossy().replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_query_export() {
        let source = r#"
import { query } from "darshan/server";

export const getUser = query({
  args: { id: v.id() },
  handler: async (ctx, args) => {
    return await ctx.db.get(args.id);
  },
});
"#;

        let exports = parse_exports(source, Path::new("users.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "getUser");
        assert_eq!(exports[0].1, FunctionKind::Query);
    }

    #[test]
    fn test_parse_multiple_exports() {
        let source = r#"
export const list = query({
  handler: async (ctx) => ctx.db.query("tasks").collect(),
});

export const create = mutation({
  args: { title: v.string() },
  handler: async (ctx, args) => {
    return await ctx.db.insert("tasks", { title: args.title });
  },
});

export const sendEmail = action({
  handler: async (ctx) => {},
});
"#;

        let exports = parse_exports(source, Path::new("tasks.ts"));
        assert_eq!(exports.len(), 3);
        assert_eq!(exports[0].1, FunctionKind::Query);
        assert_eq!(exports[1].1, FunctionKind::Mutation);
        assert_eq!(exports[2].1, FunctionKind::Action);
    }

    #[test]
    fn test_parse_default_export() {
        let source = r#"
export default httpEndpoint({
  method: "GET",
  handler: async (ctx, req) => new Response("ok"),
});
"#;

        let exports = parse_exports(source, Path::new("api/health.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "default");
        assert_eq!(exports[0].1, FunctionKind::HttpEndpoint);
    }

    #[test]
    fn test_parse_ignores_non_function_exports() {
        let source = r#"
export const CACHE_TTL = 60_000;
export type User = { name: string };
"#;

        let exports = parse_exports(source, Path::new("config.ts"));
        assert!(exports.is_empty());
    }

    #[test]
    fn test_module_name_from_path() {
        assert_eq!(module_name_from_path(Path::new("users.ts")), "users");
        assert_eq!(
            module_name_from_path(Path::new("api/tasks.ts")),
            "api/tasks"
        );
    }

    #[test]
    fn test_is_function_file() {
        assert!(is_function_file(Path::new("users.ts")));
        assert!(is_function_file(Path::new("tasks.js")));
        assert!(is_function_file(Path::new("api.mts")));
        assert!(!is_function_file(Path::new("types.d.ts")));
        assert!(!is_function_file(Path::new("users.test.ts")));
        assert!(!is_function_file(Path::new("_helpers.ts")));
        assert!(!is_function_file(Path::new("schema.json")));
    }

    #[test]
    fn test_function_kind_roundtrip() {
        for kind in [
            FunctionKind::Query,
            FunctionKind::Mutation,
            FunctionKind::Action,
            FunctionKind::Scheduled,
            FunctionKind::Internal,
            FunctionKind::HttpEndpoint,
        ] {
            let name = kind.wrapper_name();
            assert_eq!(FunctionKind::from_wrapper_name(name), Some(kind));
        }
    }

    // -----------------------------------------------------------------------
    // FunctionKind edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_function_kind_unknown_wrapper() {
        assert_eq!(FunctionKind::from_wrapper_name("unknown"), None);
        assert_eq!(FunctionKind::from_wrapper_name(""), None);
        assert_eq!(FunctionKind::from_wrapper_name("Query"), None); // case-sensitive
    }

    // -----------------------------------------------------------------------
    // parse_exports edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_scheduled_export() {
        let source = r#"export const cleanup = scheduled({
  cron: "0 * * * * *",
  handler: async (ctx) => {},
});"#;
        let exports = parse_exports(source, Path::new("crons.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "cleanup");
        assert_eq!(exports[0].1, FunctionKind::Scheduled);
    }

    #[test]
    fn test_parse_internal_fn_export() {
        let source = r#"export const helper = internalFn({
  handler: async (ctx) => {},
});"#;
        let exports = parse_exports(source, Path::new("internal.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "helper");
        assert_eq!(exports[0].1, FunctionKind::Internal);
    }

    #[test]
    fn test_parse_with_space_before_paren() {
        let source = r#"export const getUser = query ({
  handler: async (ctx) => {},
});"#;
        let exports = parse_exports(source, Path::new("users.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "getUser");
        assert_eq!(exports[0].1, FunctionKind::Query);
    }

    #[test]
    fn test_parse_empty_file() {
        let exports = parse_exports("", Path::new("empty.ts"));
        assert!(exports.is_empty());
    }

    #[test]
    fn test_parse_comments_only() {
        let source = r#"
// export const foo = query({})
/* export const bar = mutation({}) */
"#;
        // Line comments will match -- this is a known limitation of line-based
        // parsing. The comment prefix is NOT stripped before matching.
        let exports = parse_exports(source, Path::new("commented.ts"));
        // The "// export const" line starts with "//", not "export", so no match.
        assert!(exports.is_empty());
    }

    #[test]
    fn test_parse_non_function_constant() {
        let source = r#"
export const MAX_SIZE = 100;
export const config = { key: "value" };
"#;
        let exports = parse_exports(source, Path::new("config.ts"));
        assert!(exports.is_empty());
    }

    #[test]
    fn test_parse_default_mutation() {
        let source = r#"export default mutation({
  handler: async (ctx) => {},
});"#;
        let exports = parse_exports(source, Path::new("api.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "default");
        assert_eq!(exports[0].1, FunctionKind::Mutation);
    }

    #[test]
    fn test_parse_mixed_exports_and_constants() {
        let source = r#"
export const TIMEOUT = 5000;
export const getAll = query({
  handler: async (ctx) => ctx.db.query("items").collect(),
});
export type Item = { name: string };
export const deleteAll = mutation({
  handler: async (ctx) => {},
});
"#;
        let exports = parse_exports(source, Path::new("items.ts"));
        assert_eq!(exports.len(), 2);
        assert_eq!(exports[0].0, "getAll");
        assert_eq!(exports[0].1, FunctionKind::Query);
        assert_eq!(exports[1].0, "deleteAll");
        assert_eq!(exports[1].1, FunctionKind::Mutation);
    }

    // -----------------------------------------------------------------------
    // is_function_file edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_function_file_mjs() {
        assert!(is_function_file(Path::new("utils.mjs")));
    }

    #[test]
    fn test_is_function_file_mts() {
        assert!(is_function_file(Path::new("utils.mts")));
    }

    #[test]
    fn test_is_function_file_spec_excluded() {
        assert!(!is_function_file(Path::new("users.spec.ts")));
        assert!(!is_function_file(Path::new("users.spec.js")));
    }

    #[test]
    fn test_is_function_file_underscore_excluded() {
        assert!(!is_function_file(Path::new("_internal.ts")));
        assert!(!is_function_file(Path::new("_darshan_harness.ts")));
    }

    #[test]
    fn test_is_function_file_non_js_extensions() {
        assert!(!is_function_file(Path::new("readme.md")));
        assert!(!is_function_file(Path::new("config.json")));
        assert!(!is_function_file(Path::new("data.csv")));
        assert!(!is_function_file(Path::new("image.png")));
    }

    #[test]
    fn test_is_function_file_no_extension() {
        assert!(!is_function_file(Path::new("Makefile")));
    }

    // -----------------------------------------------------------------------
    // module_name_from_path edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn test_module_name_nested() {
        assert_eq!(
            module_name_from_path(Path::new("api/v2/users.ts")),
            "api/v2/users"
        );
    }

    #[test]
    fn test_module_name_windows_separators() {
        // Backslashes get normalized to forward slashes.
        assert_eq!(
            module_name_from_path(Path::new("api\\users.ts")),
            "api/users"
        );
    }

    // -----------------------------------------------------------------------
    // has_function_extension
    // -----------------------------------------------------------------------

    #[test]
    fn test_has_function_extension_mixed() {
        let paths = vec![PathBuf::from("readme.md"), PathBuf::from("users.ts")];
        assert!(has_function_extension(&paths));
    }

    #[test]
    fn test_has_function_extension_none() {
        let paths = vec![PathBuf::from("readme.md"), PathBuf::from("config.json")];
        assert!(!has_function_extension(&paths));
    }

    #[test]
    fn test_has_function_extension_empty() {
        assert!(!has_function_extension(&[]));
    }

    // -----------------------------------------------------------------------
    // Plain export default function parsing
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_default_function_named() {
        let source = r#"export default function hello(args) {
  return { message: "Hello " + args.name };
}"#;
        let exports = parse_exports(source, Path::new("hello.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "hello");
        assert_eq!(exports[0].1, FunctionKind::Action);
    }

    #[test]
    fn test_parse_default_function_anonymous() {
        let source = r#"export default function(args) {
  return { result: true };
}"#;
        let exports = parse_exports(source, Path::new("anon.ts"));
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].0, "default");
        assert_eq!(exports[0].1, FunctionKind::Action);
    }
}
