//! Embedded mlua 0.10 Lua function runtime for DarshJDB.
//!
//! DarshJDB — created by Darshankumar Joshi (github.com/darshjme).
//!
//! Alternative to [`ProcessRuntime`](super::runtime::ProcessRuntime) for
//! deployments that want sub-millisecond cold starts on server functions
//! written in Lua. Instead of spawning a Deno/Node subprocess per invocation
//! (~100ms minimum cold start), this backend embeds a Lua 5.4 VM directly
//! via `mlua` 0.10 and runs the user function inside a sandboxed
//! [`mlua::Lua`] owned by the server process.
//!
//! Behind the `mlua-runtime` Cargo feature so default builds stay lean:
//! ```bash
//! cargo run -p ddb-server --features mlua-runtime
//! ```
//!
//! Selected at runtime when `DDB_FUNCTION_RUNTIME=mlua` and the feature is
//! compiled in; otherwise the server falls back to [`ProcessRuntime`].
//!
//! # Security — sandbox
//!
//! Before user code runs, [`install_sandbox`] strips the dangerous Lua
//! globals:
//!
//! - `io` — removed entirely (no filesystem).
//! - `os` — replaced with a whitelisted stub exposing only `time`, `date`,
//!   `clock`. `os.execute`, `os.exit`, `os.remove`, `os.rename`,
//!   `os.getenv`, `os.setenv` are unreachable.
//! - `package` — removed (disables `require`).
//! - `dofile`, `loadfile`, `load`, `loadstring` — set to `nil`.
//! - `debug.sethook` — removed (blocks hook-based VM escapes).
//!
//! A sandbox regression test in this module's `tests` submodule asserts
//! that `os.execute` is unreachable from user code.
//!
//! # The `ddb.*` API
//!
//! User Lua code talks to the database through a single top-level `ddb`
//! table registered at runtime construction. v0.3.2 ships the **API
//! shape** — every function is wired, callable, and type-checked by Lua —
//! but the host-side plumbing is **stubbed**. Calls panic with a clear
//! "not yet wired" message in v0.3.2 and will be wired to the real store
//! in v0.3.3.
//!
//! The API surface registered on the Lua instance:
//!
//! ```lua
//! local rows = ddb.query("SELECT * FROM users")           -- NotYetImplemented
//! local v    = ddb.kv.get("cache:foo")                    -- NotYetImplemented
//! ddb.kv.set("cache:foo", "bar")                          -- NotYetImplemented
//! ddb.log.info("hello")                                   -- wired -> tracing::info!
//! ddb.log.error("nope")                                   -- wired -> tracing::error!
//! local o = ddb.triples.get("subject", "predicate")       -- NotYetImplemented
//! ddb.triples.put("subject", "predicate", "object")       -- NotYetImplemented
//! ```
//!
//! Only the `ddb.log.*` paths are fully live in v0.3.2 because they have
//! no cross-crate dependencies. The rest return `NotYetImplemented` Lua
//! errors so user code that calls them fails loudly rather than silently.

#![cfg(feature = "mlua-runtime")]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use mlua::{Lua, LuaSerdeExt, Nil, Table, Value as LuaValue};
use serde_json::Value;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, error, info, instrument, warn};

use super::registry::FunctionDef;
use super::runtime::{
    ExecutionContext, ExecutionResult, LogEntry, ResourceLimits, RuntimeBackend, RuntimeError,
    RuntimeResult,
};

// ---------------------------------------------------------------------------
// MluaRuntime
// ---------------------------------------------------------------------------

/// Embedded mlua 0.10 Lua runtime.
///
/// Holds a single [`mlua::Lua`] instance per worker, behind a `Mutex`
/// because `Lua` is `!Sync`. Each [`MluaRuntime::execute`] call:
///
/// 1. Acquires a concurrency permit from the semaphore.
/// 2. Locks the shared Lua VM.
/// 3. Loads the user function file from disk (if not already registered).
/// 4. Calls the requested export with the JSON-serialized arguments.
/// 5. Maps the Lua return value back through `serde` into DDB's [`Value`].
///
/// A single shared VM keeps memory footprint predictable; all user code
/// runs inside the sandbox installed by [`install_sandbox`].
pub struct MluaRuntime {
    /// Base directory containing user function files (`.lua`).
    functions_dir: PathBuf,

    /// Shared Lua VM. `Mutex` because `mlua::Lua` is `!Sync`.
    lua: Arc<Mutex<Lua>>,

    /// Concurrency semaphore bounding simultaneously-live invocations.
    semaphore: Arc<Semaphore>,
}

impl MluaRuntime {
    /// Construct a new embedded Lua runtime rooted at `functions_dir`.
    ///
    /// Creates the Lua VM, installs the sandbox, and registers the `ddb.*`
    /// API table. Returns a [`RuntimeError`] if sandbox installation or
    /// API registration fails.
    pub fn new(functions_dir: PathBuf, max_concurrency: usize) -> RuntimeResult<Self> {
        let lua = Lua::new();
        install_sandbox(&lua)
            .map_err(|e| RuntimeError::Internal(format!("sandbox install failed: {e}")))?;
        install_ddb_api(&lua)
            .map_err(|e| RuntimeError::Internal(format!("ddb api install failed: {e}")))?;

        Ok(Self {
            functions_dir,
            lua: Arc::new(Mutex::new(lua)),
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
        })
    }

    /// Register a raw Lua chunk into the VM for testing. The chunk is
    /// executed in the sandboxed global environment so subsequent
    /// invocations can look up any globals it defined.
    #[cfg(test)]
    async fn load_chunk(&self, chunk: &str) -> RuntimeResult<()> {
        let guard = self.lua.lock().await;
        guard
            .load(chunk)
            .exec()
            .map_err(|e| RuntimeError::Internal(format!("lua load failed: {e}")))?;
        Ok(())
    }

    /// Invoke a global Lua function by name with JSON-serialized args and
    /// return its result converted back to [`Value`]. Test helper.
    #[cfg(test)]
    async fn invoke_global(&self, name: &str, args: Value) -> RuntimeResult<Value> {
        let guard = self.lua.lock().await;
        let func: mlua::Function = guard
            .globals()
            .get(name)
            .map_err(|e| RuntimeError::Internal(format!("lua global `{name}` missing: {e}")))?;
        let lua_args: LuaValue = guard
            .to_value(&args)
            .map_err(|e| RuntimeError::Internal(format!("args -> lua failed: {e}")))?;
        let ret: LuaValue = func
            .call(lua_args)
            .map_err(|e| RuntimeError::Internal(format!("lua call failed: {e}")))?;
        let out: Value = guard
            .from_value(ret)
            .map_err(|e| RuntimeError::Internal(format!("lua -> json failed: {e}")))?;
        Ok(out)
    }
}

impl RuntimeBackend for MluaRuntime {
    #[instrument(skip(self, function_def, context, _limits), fields(fn = %function_def.name))]
    fn execute(
        &self,
        function_def: &FunctionDef,
        context: &ExecutionContext,
        _limits: &ResourceLimits,
    ) -> Pin<Box<dyn Future<Output = RuntimeResult<ExecutionResult>> + Send + '_>> {
        let function_def = function_def.clone();
        let context = context.clone();
        let functions_dir = self.functions_dir.clone();
        let lua = self.lua.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| RuntimeError::Internal(format!("semaphore closed: {e}")))?;

            let started = Instant::now();

            debug!(
                invocation_id = %context.invocation_id,
                function = %context.function_name,
                "mlua invoking function"
            );

            let function_path = functions_dir.join(&function_def.file_path);
            let source = std::fs::read_to_string(&function_path).map_err(|e| {
                RuntimeError::Internal(format!(
                    "failed to read lua source `{}`: {e}",
                    function_path.display()
                ))
            })?;

            let guard = lua.lock().await;

            // Load the chunk. Executing it is expected to produce a global
            // with `function_def.export_name`, matching the shape the JS
            // harness uses (`export const foo = () => ...`).
            guard
                .load(source)
                .set_name(function_def.file_path.to_string_lossy().into_owned())
                .exec()
                .map_err(|e| RuntimeError::Internal(format!("lua load failed: {e}")))?;

            let func: mlua::Function = guard.globals().get(function_def.export_name.as_str()).map_err(|e| {
                RuntimeError::Internal(format!(
                    "lua export `{}` not found after load: {e}",
                    function_def.export_name
                ))
            })?;

            let lua_args: LuaValue = guard.to_value(&context.args).map_err(|e| {
                RuntimeError::Internal(format!("failed to convert args into lua: {e}"))
            })?;

            let ret: LuaValue = func.call(lua_args).map_err(|e| {
                error!(
                    invocation_id = %context.invocation_id,
                    error = %e,
                    "lua function raised error"
                );
                RuntimeError::Internal(format!("lua call failed: {e}"))
            })?;

            let value: Value = guard.from_value(ret).map_err(|e| {
                RuntimeError::Internal(format!("failed to convert lua return: {e}"))
            })?;

            let duration_ms = started.elapsed().as_millis() as u64;
            info!(
                invocation_id = %context.invocation_id,
                duration_ms,
                "mlua function completed"
            );

            Ok(ExecutionResult {
                value,
                duration_ms,
                peak_memory_bytes: None,
                logs: Vec::<LogEntry>::new(),
            })
        })
    }

    fn health_check(&self) -> Pin<Box<dyn Future<Output = RuntimeResult<()>> + Send + '_>> {
        let lua = self.lua.clone();
        Box::pin(async move {
            let guard = lua.lock().await;
            // Trivial eval to prove the VM is live.
            let v: i64 = guard
                .load("return 1 + 1")
                .eval()
                .map_err(|e| RuntimeError::Internal(format!("lua health eval failed: {e}")))?;
            if v == 2 {
                info!(backend = "mlua-embedded", "runtime healthy");
                Ok(())
            } else {
                Err(RuntimeError::Internal(format!(
                    "lua health eval returned {v}, expected 2"
                )))
            }
        })
    }

    fn name(&self) -> &str {
        "mlua-embedded"
    }
}

// ---------------------------------------------------------------------------
// Sandbox
// ---------------------------------------------------------------------------

/// Strip every dangerous global from `lua` before exposing it to user code.
///
/// See the module-level doc for the full list. Leaves behind a whitelisted
/// `os` replacement containing only `time`, `date`, `clock`.
pub fn install_sandbox(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();

    // Build a whitelisted `os` table before nuking the original, so the
    // three safe helpers remain callable.
    let safe_os = lua.create_table()?;
    if let Ok(orig_os) = globals.get::<Table>("os") {
        if let Ok(f) = orig_os.get::<mlua::Function>("time") {
            safe_os.set("time", f)?;
        }
        if let Ok(f) = orig_os.get::<mlua::Function>("date") {
            safe_os.set("date", f)?;
        }
        if let Ok(f) = orig_os.get::<mlua::Function>("clock") {
            safe_os.set("clock", f)?;
        }
    }
    globals.set("os", safe_os)?;

    // Full filesystem I/O — no user code should touch this.
    globals.set("io", Nil)?;

    // Disable `require` and friends by removing the loader table.
    globals.set("package", Nil)?;

    // Kill raw code-loading primitives.
    globals.set("dofile", Nil)?;
    globals.set("loadfile", Nil)?;
    globals.set("load", Nil)?;
    globals.set("loadstring", Nil)?;

    // `debug.sethook` is a well-known escape hatch; strip it but leave
    // the rest of `debug` alone (tracebacks are still useful).
    if let Ok(debug_tbl) = globals.get::<Table>("debug") {
        debug_tbl.set("sethook", Nil)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// ddb.* API stubs
// ---------------------------------------------------------------------------

/// Register the `ddb` global table exposing the DDB host API to user Lua
/// code. v0.3.2 ships the **shape** — every function is present and
/// callable — but most implementations return Lua errors tagged
/// `NotYetImplemented`. Full wiring lands in v0.3.3.
///
/// The only live bindings are `ddb.log.info` / `ddb.log.warn` /
/// `ddb.log.error` / `ddb.log.debug`, which forward into `tracing`.
pub fn install_ddb_api(lua: &Lua) -> mlua::Result<()> {
    let ddb = lua.create_table()?;

    // ddb.query(darshanql_string) -> rows
    ddb.set(
        "query",
        lua.create_function(|_, _q: String| -> mlua::Result<LuaValue> {
            Err(mlua::Error::RuntimeError(
                "ddb.query: NotYetImplemented — wires up in v0.3.3".into(),
            ))
        })?,
    )?;

    // ddb.kv.{get,set}
    let kv = lua.create_table()?;
    kv.set(
        "get",
        lua.create_function(|_, _key: String| -> mlua::Result<LuaValue> {
            Err(mlua::Error::RuntimeError(
                "ddb.kv.get: NotYetImplemented — wires up in v0.3.3".into(),
            ))
        })?,
    )?;
    kv.set(
        "set",
        lua.create_function(|_, (_key, _val): (String, LuaValue)| -> mlua::Result<()> {
            Err(mlua::Error::RuntimeError(
                "ddb.kv.set: NotYetImplemented — wires up in v0.3.3".into(),
            ))
        })?,
    )?;
    ddb.set("kv", kv)?;

    // ddb.log.* — fully live, forwards into tracing.
    let log = lua.create_table()?;
    log.set(
        "debug",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            debug!(target: "ddb_functions::mlua::user", "{msg}");
            Ok(())
        })?,
    )?;
    log.set(
        "info",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            info!(target: "ddb_functions::mlua::user", "{msg}");
            Ok(())
        })?,
    )?;
    log.set(
        "warn",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            warn!(target: "ddb_functions::mlua::user", "{msg}");
            Ok(())
        })?,
    )?;
    log.set(
        "error",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            error!(target: "ddb_functions::mlua::user", "{msg}");
            Ok(())
        })?,
    )?;
    ddb.set("log", log)?;

    // ddb.triples.{get,put}
    let triples = lua.create_table()?;
    triples.set(
        "get",
        lua.create_function(
            |_, (_s, _p): (String, String)| -> mlua::Result<LuaValue> {
                Err(mlua::Error::RuntimeError(
                    "ddb.triples.get: NotYetImplemented — wires up in v0.3.3".into(),
                ))
            },
        )?,
    )?;
    triples.set(
        "put",
        lua.create_function(
            |_, (_s, _p, _o): (String, String, LuaValue)| -> mlua::Result<()> {
                Err(mlua::Error::RuntimeError(
                    "ddb.triples.put: NotYetImplemented — wires up in v0.3.3".into(),
                ))
            },
        )?,
    )?;
    ddb.set("triples", triples)?;

    lua.globals().set("ddb", ddb)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, feature = "mlua-runtime"))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn new_runtime() -> MluaRuntime {
        MluaRuntime::new(PathBuf::from("/tmp/ddb-test-functions"), 4)
            .expect("mlua runtime must construct")
    }

    #[tokio::test]
    async fn invoke_trivial_double() {
        let rt = new_runtime();
        rt.load_chunk("function double(x) return x * 2 end")
            .await
            .unwrap();
        let result = rt
            .invoke_global("double", serde_json::json!(5))
            .await
            .unwrap();
        assert_eq!(result, serde_json::json!(10));
    }

    #[tokio::test]
    async fn sandbox_blocks_os_execute() {
        let rt = new_runtime();
        // `os` is replaced with the whitelisted stub, so `os.execute`
        // must be `nil` and calling it must error.
        rt.load_chunk(
            r#"
            function try_exec()
                return os.execute ~= nil
            end
        "#,
        )
        .await
        .unwrap();
        let has_execute = rt
            .invoke_global("try_exec", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(
            has_execute,
            serde_json::json!(false),
            "os.execute must be unreachable after sandbox"
        );

        // And directly calling it must raise a Lua error.
        rt.load_chunk("function call_exec() return os.execute('echo pwned') end")
            .await
            .unwrap();
        let err = rt
            .invoke_global("call_exec", serde_json::json!(null))
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("lua call failed"),
            "expected lua error, got {err}"
        );
    }

    #[tokio::test]
    async fn sandbox_blocks_io_and_require_and_loaders() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function shape_check()
                return {
                    io_nil       = io == nil,
                    package_nil  = package == nil,
                    dofile_nil   = dofile == nil,
                    loadfile_nil = loadfile == nil,
                    load_nil     = load == nil,
                }
            end
            "#,
        )
        .await
        .unwrap();
        let shape = rt
            .invoke_global("shape_check", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(shape["io_nil"], serde_json::json!(true));
        assert_eq!(shape["package_nil"], serde_json::json!(true));
        assert_eq!(shape["dofile_nil"], serde_json::json!(true));
        assert_eq!(shape["loadfile_nil"], serde_json::json!(true));
        assert_eq!(shape["load_nil"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn os_whitelist_still_has_time() {
        let rt = new_runtime();
        rt.load_chunk("function t() return type(os.time()) end")
            .await
            .unwrap();
        let ty = rt
            .invoke_global("t", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(ty, serde_json::json!("number"));
    }

    #[tokio::test]
    async fn ddb_log_info_is_live() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function go()
                ddb.log.info("hello from lua")
                return "ok"
            end
        "#,
        )
        .await
        .unwrap();
        let out = rt.invoke_global("go", serde_json::json!(null)).await.unwrap();
        assert_eq!(out, serde_json::json!("ok"));
    }

    #[tokio::test]
    async fn ddb_query_stub_errors_clearly() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function go() return ddb.query("SELECT 1") end
        "#,
        )
        .await
        .unwrap();
        let err = rt
            .invoke_global("go", serde_json::json!(null))
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("NotYetImplemented"),
            "expected NotYetImplemented, got {err}"
        );
    }

    #[tokio::test]
    async fn backend_name_is_mlua_embedded() {
        let rt = new_runtime();
        assert_eq!(rt.name(), "mlua-embedded");
    }

    #[tokio::test]
    async fn health_check_passes() {
        let rt = new_runtime();
        rt.health_check().await.unwrap();
    }
}
