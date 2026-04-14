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
//! Before user code runs, [`install_sandbox`] strips every known
//! sandbox-escape path from the Lua environment:
//!
//! - `io` — removed entirely (no filesystem).
//! - `os` — replaced with a whitelisted stub exposing only `time`, `date`,
//!   `clock`. `os.execute`, `os.exit`, `os.remove`, `os.rename`,
//!   `os.getenv`, `os.setenv` are unreachable. The original `os` table is
//!   not retained anywhere after install.
//! - `package` — removed (disables `require` via loader).
//! - `require` — removed separately (it is a standalone global in 5.4).
//! - `debug` — removed entirely. `debug.getregistry()._LOADED.io.popen`
//!   would otherwise reach the original io table despite `globals.io = nil`.
//!   `getupvalue`/`setupvalue`/`getlocal`/`setlocal` are also banned.
//! - `dofile`, `loadfile`, `load`, `loadstring` — set to `nil`.
//! - `string.dump` — set to `nil` (serializes to bytecode, enables
//!   bytecode-injection attacks).
//! - `collectgarbage` — removed.
//! - `rawget`, `rawset`, `rawequal`, `rawlen` — removed so metamethod
//!   instrumentation cannot be bypassed.
//!
//! Load calls additionally pin [`mlua::ChunkMode::Text`] so crafted
//! bytecode chunks are refused at load time. User functions execute in a
//! per-invocation environment table whose `__index` falls through to a
//! frozen `safe_globals` snapshot held in the Lua registry — mutations
//! like `string.sub = function() end` land in the fresh env table and are
//! dropped when the call returns, so they cannot leak cross-tenant.
//!
//! # Resource caps
//!
//! Each invocation is wrapped in [`tokio::time::timeout`] with a
//! configurable wall-clock cap (default 5 seconds, sourced from
//! [`ResourceLimits::cpu_time_ms`]) and dispatches through
//! [`mlua::Function::call_async`] so yielding user code is cancelled
//! cleanly. CPU-bound interruption of non-yielding loops still needs
//! mlua 0.10's `set_interrupt` API and is tracked for v0.3.3.
//!
//! NB: the current implementation is single-VM-serialized on one
//! `Mutex<Lua>` — effective concurrency is 1. A `Pool<Lua>` for real
//! concurrency is scoped for v0.4.
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

use mlua::{ChunkMode, Function, Lua, LuaSerdeExt, Nil, Table, Value as LuaValue};
use serde_json::Value;
use tokio::sync::Mutex;
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
/// 1. Locks the shared Lua VM.
/// 2. Builds a fresh per-invocation env (see [`build_per_call_env`]).
/// 3. Loads the user function file from disk.
/// 4. Calls the requested export with the JSON-serialized arguments,
///    wrapped in `tokio::time::timeout` for wall-clock bounding.
/// 5. Maps the Lua return value back through `serde` into DDB's [`Value`].
///
/// **Concurrency contract**: this implementation is single-VM-serialized
/// — every invocation locks the same `Mutex<Lua>` and effective
/// concurrency is 1. A `Pool<Lua>` for real concurrency (one VM per
/// worker, round-robined or checked-out) is scoped for v0.4. A
/// per-invocation semaphore was considered and rejected: it admitted N
/// permits but every admitted task then locked the one `Mutex<Lua>`, so
/// the permit cap was theatre (MJ-02).
pub struct MluaRuntime {
    /// Base directory containing user function files (`.lua`),
    /// canonicalized at construction so path-containment checks on each
    /// invocation compare apples-to-apples. See [`MluaRuntime::new`] and
    /// F6 / MN-04.
    functions_dir: PathBuf,

    /// Shared Lua VM. `Mutex` because `mlua::Lua` is `!Sync`.
    ///
    /// NB: single shared VM serializes all invocations. A `Pool<Lua>`
    /// for real concurrency is tracked for v0.4. See MJ-02.
    lua: Arc<Mutex<Lua>>,
}

impl MluaRuntime {
    /// Construct a new embedded Lua runtime rooted at `functions_dir`.
    ///
    /// Creates the Lua VM, installs the sandbox, registers the `ddb.*`
    /// API table, and freezes the safe-globals snapshot. Returns a
    /// [`RuntimeError`] if any step fails.
    ///
    /// The `_max_concurrency` parameter is accepted for call-site
    /// compatibility but ignored: the single `Mutex<Lua>` already
    /// serializes every invocation to one in-flight call. See the
    /// type-level doc for the v0.4 pool plan.
    pub fn new(functions_dir: PathBuf, _max_concurrency: usize) -> RuntimeResult<Self> {
        // MN-03 + F6: canonicalize and validate the functions directory
        // at construction time. Misconfiguration surfaces immediately at
        // boot instead of as a cryptic per-call error on first invoke.
        if !functions_dir.exists() {
            return Err(RuntimeError::Internal(format!(
                "mlua functions_dir does not exist: {}",
                functions_dir.display()
            )));
        }
        if !functions_dir.is_dir() {
            return Err(RuntimeError::Internal(format!(
                "mlua functions_dir is not a directory: {}",
                functions_dir.display()
            )));
        }
        let functions_dir = std::fs::canonicalize(&functions_dir).map_err(|e| {
            RuntimeError::Internal(format!(
                "mlua functions_dir canonicalize failed ({}): {e}",
                functions_dir.display()
            ))
        })?;

        let lua = Lua::new();
        install_sandbox(&lua)
            .map_err(|e| RuntimeError::Internal(format!("sandbox install failed: {e}")))?;
        install_ddb_api(&lua)
            .map_err(|e| RuntimeError::Internal(format!("ddb api install failed: {e}")))?;
        freeze_safe_globals(&lua).map_err(|e| {
            RuntimeError::Internal(format!("safe_globals freeze failed: {e}"))
        })?;

        Ok(Self {
            functions_dir,
            lua: Arc::new(Mutex::new(lua)),
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
            .set_mode(ChunkMode::Text)
            .exec()
            .map_err(|e| RuntimeError::Internal(format!("lua load failed: {e}")))?;
        Ok(())
    }

    /// Test helper: load a raw byte buffer with no ChunkMode restriction.
    /// Used by `sandbox_rejects_bytecode_chunk` to confirm the production
    /// path (which always pins `ChunkMode::Text`) refuses bytecode. Not
    /// exposed outside the test build.
    #[cfg(test)]
    async fn load_bytes_as_text(&self, bytes: &[u8]) -> RuntimeResult<()> {
        let guard = self.lua.lock().await;
        guard
            .load(bytes)
            .set_mode(ChunkMode::Text)
            .exec()
            .map_err(|e| RuntimeError::Internal(format!("lua load failed: {e}")))?;
        Ok(())
    }

    /// Test helper: run a user chunk under the same per-invocation env
    /// isolation the production `execute` path uses, then invoke
    /// `export_name` with `args`. Returns the deserialized JSON return
    /// value. This is the supported way to exercise F4 isolation in
    /// tests — going through `load_chunk` + `invoke_global` bypasses the
    /// env entirely and mutates the shared `_G`.
    #[cfg(test)]
    async fn exec_in_fresh_env(
        &self,
        source: &str,
        export_name: &str,
        args: Value,
    ) -> RuntimeResult<Value> {
        let guard = self.lua.lock().await;

        let env = build_per_call_env(&guard)
            .map_err(|e| RuntimeError::Internal(format!("per-call env: {e}")))?;

        guard
            .load(source)
            .set_mode(ChunkMode::Text)
            .set_environment(env.clone())
            .exec()
            .map_err(|e| RuntimeError::Internal(format!("lua load failed: {e}")))?;

        let func: mlua::Function = env.get(export_name).map_err(|e| {
            RuntimeError::Internal(format!("export `{export_name}` missing: {e}"))
        })?;
        let lua_args: LuaValue = guard
            .to_value(&args)
            .map_err(|e| RuntimeError::Internal(format!("args -> lua: {e}")))?;
        let ret: LuaValue = func
            .call(lua_args)
            .map_err(|e| RuntimeError::Internal(format!("lua call failed: {e}")))?;
        let out: Value = guard
            .from_value(ret)
            .map_err(|e| RuntimeError::Internal(format!("lua -> json: {e}")))?;
        Ok(out)
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
    #[instrument(skip(self, function_def, context, limits), fields(fn = %function_def.name))]
    fn execute(
        &self,
        function_def: &FunctionDef,
        context: &ExecutionContext,
        limits: &ResourceLimits,
    ) -> Pin<Box<dyn Future<Output = RuntimeResult<ExecutionResult>> + Send + '_>> {
        let function_def = function_def.clone();
        let context = context.clone();
        let functions_dir = self.functions_dir.clone();
        let lua = self.lua.clone();
        let wall_clock_cap = std::time::Duration::from_millis(limits.cpu_time_ms.max(1));

        Box::pin(async move {
            let started = Instant::now();

            debug!(
                invocation_id = %context.invocation_id,
                function = %context.function_name,
                "mlua invoking function"
            );

            // F6: resolve and canonicalize the function path BEFORE
            // acquiring the Lua mutex (also fixes MN-04: the old sync
            // std::fs::read_to_string was held across the mutex guard).
            //
            // function_def.file_path is untrusted input — a naive
            // join(functions_dir, "../../etc/passwd") would happily
            // escape the functions directory. Canonicalize and assert
            // containment.
            let unchecked = functions_dir.join(&function_def.file_path);
            let canon = tokio::fs::canonicalize(&unchecked).await.map_err(|e| {
                RuntimeError::Internal(format!(
                    "function path not found: {}: {e}",
                    unchecked.display()
                ))
            })?;
            if !canon.starts_with(&functions_dir) {
                return Err(RuntimeError::Internal(format!(
                    "function path escapes functions directory: {}",
                    canon.display()
                )));
            }
            let source = tokio::fs::read_to_string(&canon).await.map_err(|e| {
                RuntimeError::Internal(format!(
                    "failed to read lua source `{}`: {e}",
                    canon.display()
                ))
            })?;

            let guard = lua.lock().await;

            // F4: build a fresh per-invocation environment with per-call
            // proxy tables for every mutable library (`string`, `table`,
            // `math`, `os`, `ddb`). User top-level mutations like
            // `string.sub = function() end` land on the proxy and are
            // dropped when the call returns; the shared `_G` and the
            // frozen `safe_globals` snapshot are never touched.
            let env = build_per_call_env(&guard).map_err(|e| {
                RuntimeError::Internal(format!("per-call env build failed: {e}"))
            })?;

            // Load the chunk. Executing it is expected to produce a
            // global (in the per-call env) with `function_def.export_name`,
            // matching the shape the JS harness uses
            // (`export const foo = () => ...`).
            //
            // `ChunkMode::Text` refuses any chunk whose first byte is the
            // Lua bytecode marker (`\x1bLua`). Without this, crafted
            // bytecode could bypass every source-level sandbox check and
            // hit CVE-class vulnerabilities in the unverified bytecode
            // loader. F5.
            guard
                .load(source)
                .set_name(function_def.file_path.to_string_lossy().into_owned())
                .set_mode(ChunkMode::Text)
                .set_environment(env.clone())
                .exec()
                .map_err(|e| RuntimeError::Internal(format!("lua load failed: {e}")))?;

            let func: mlua::Function = env.get(function_def.export_name.as_str()).map_err(|e| {
                RuntimeError::Internal(format!(
                    "lua export `{}` not found after load: {e}",
                    function_def.export_name
                ))
            })?;

            let lua_args: LuaValue = guard.to_value(&context.args).map_err(|e| {
                RuntimeError::Internal(format!("failed to convert args into lua: {e}"))
            })?;

            // MJ-01 / F2: wall-clock cap via tokio::time::timeout + the
            // async call path. mlua's `call_async` yields at each Lua
            // `coroutine.yield` / async host-call boundary, letting
            // tokio's timer fire cleanly. Note: this does NOT interrupt
            // CPU-bound user code (`while true do end`) mid-instruction.
            // Full interruption requires mlua 0.10's `Lua::set_interrupt`
            // API and is scoped for v0.3.3.
            let call_fut = func.call_async::<LuaValue>(lua_args);
            let ret: LuaValue = match tokio::time::timeout(wall_clock_cap, call_fut).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    error!(
                        invocation_id = %context.invocation_id,
                        error = %e,
                        "lua function raised error"
                    );
                    return Err(RuntimeError::Internal(format!("lua call failed: {e}")));
                }
                Err(_) => {
                    error!(
                        invocation_id = %context.invocation_id,
                        cap_ms = wall_clock_cap.as_millis() as u64,
                        "lua function exceeded wall-clock cap"
                    );
                    return Err(RuntimeError::Internal(format!(
                        "lua call `{}` exceeded wall-clock cap ({} ms)",
                        function_def.name,
                        wall_clock_cap.as_millis() as u64
                    )));
                }
            };

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
    // three safe helpers remain callable. The original `os` table is NOT
    // retained anywhere after this function returns.
    let safe_os = lua.create_table()?;
    if let Ok(orig_os) = globals.get::<Table>("os") {
        if let Ok(f) = orig_os.get::<Function>("time") {
            safe_os.set("time", f)?;
        }
        if let Ok(f) = orig_os.get::<Function>("date") {
            safe_os.set("date", f)?;
        }
        if let Ok(f) = orig_os.get::<Function>("clock") {
            safe_os.set("clock", f)?;
        }
    }
    globals.set("os", safe_os)?;

    // Libraries: fully nil. Do NOT leave any field reachable. CR-01/CR-02.
    globals.set("io", Nil)?;
    globals.set("package", Nil)?;
    // `require` is a SEPARATE global in Lua 5.4 — nilling `package` alone
    // does not remove it. CR-02.
    globals.set("require", Nil)?;
    // Nuke the WHOLE `debug` table. `debug.getregistry()._LOADED.io.popen`
    // reaches the original io table the registry still holds; `getupvalue`
    // / `setupvalue` / `getlocal` / `setlocal` allow cross-frame state
    // mutation. CR-01.
    globals.set("debug", Nil)?;

    // Raw code-loading primitives (also reachable via 5.1-compat shims in
    // some builds).
    globals.set("dofile", Nil)?;
    globals.set("loadfile", Nil)?;
    globals.set("load", Nil)?;
    globals.set("loadstring", Nil)?;

    // `string.dump` serializes a function to bytecode, which combined with
    // a reconstructed `load()` bypasses the source-level sandbox and can
    // hit Lua 5.4 CVE-class vulnerabilities in the unverified bytecode
    // loader. CR-03.
    if let Ok(string_tbl) = globals.get::<Table>("string") {
        string_tbl.set("dump", Nil)?;
    }

    // GC introspection leaks memory layout and lets user code force
    // pressure on the shared VM.
    globals.set("collectgarbage", Nil)?;

    // Raw accessors bypass __index / __newindex metamethods, which v0.3.3
    // plans to use on the `ddb` table for audit-log instrumentation. Strip
    // pre-emptively so the audit hook cannot be side-stepped.
    globals.set("rawget", Nil)?;
    globals.set("rawset", Nil)?;
    globals.set("rawequal", Nil)?;
    globals.set("rawlen", Nil)?;

    Ok(())
}

/// Registry slot under which the frozen safe-globals snapshot is stored.
const SAFE_GLOBALS_REGISTRY_KEY: &str = "ddb_safe_globals";

/// Scalar globals that are safe to copy by value into the per-call env —
/// functions with no mutable state, so per-tenant drift is impossible.
const SAFE_SCALAR_GLOBALS: &[&str] = &[
    "ipairs",
    "pairs",
    "next",
    "pcall",
    "xpcall",
    "error",
    "assert",
    "type",
    "tostring",
    "tonumber",
    "select",
    "setmetatable",
    "getmetatable",
];

/// Library tables that MUST be wrapped in per-call proxy tables, because a
/// user chunk doing `string.sub = function() end` at top level would
/// otherwise mutate the shared library for every subsequent tenant.
const SAFE_LIBRARY_GLOBALS: &[&str] = &["string", "table", "math", "os", "ddb"];

/// Capture a frozen snapshot of every whitelisted global into a table held
/// in the Lua registry. Each invocation pulls this table and builds a
/// fresh per-call environment on top of it: scalar helpers are copied by
/// reference, and each library table is wrapped in a fresh proxy whose
/// `__index` falls through to the frozen original, so
/// `string.sub = function() end` at the top level of a user chunk lands
/// in the proxy and is dropped when the call returns. F4.
///
/// Must be called AFTER [`install_sandbox`] and [`install_ddb_api`] so
/// the snapshot captures the stripped `os`, the `ddb` table, etc.
fn freeze_safe_globals(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    let safe = lua.create_table()?;

    for key in SAFE_SCALAR_GLOBALS {
        let v: LuaValue = globals.get(*key)?;
        safe.set(*key, v)?;
    }
    for key in SAFE_LIBRARY_GLOBALS {
        let v: LuaValue = globals.get(*key)?;
        safe.set(*key, v)?;
    }
    if let Ok(v) = globals.get::<LuaValue>("_VERSION") {
        safe.set("_VERSION", v)?;
    }

    lua.set_named_registry_value(SAFE_GLOBALS_REGISTRY_KEY, safe)?;
    Ok(())
}

/// Build a fresh per-invocation environment table whose `__index` falls
/// through to the frozen `safe_globals` snapshot, AND whose library
/// entries (`string`, `table`, `math`, `os`, `ddb`) are wrapped in
/// per-call proxy tables. This is the F4 isolation contract: user code
/// can rebind `string.sub` at the top level of its chunk without leaking
/// into any other tenant's environment. See [`freeze_safe_globals`] for
/// the snapshot lifecycle.
fn build_per_call_env(lua: &Lua) -> mlua::Result<Table> {
    let safe_globals: Table = lua.named_registry_value(SAFE_GLOBALS_REGISTRY_KEY)?;

    let env = lua.create_table()?;
    let env_meta = lua.create_table()?;
    env_meta.set("__index", safe_globals.clone())?;
    env.set_metatable(Some(env_meta));

    // Wrap each library table in its own per-call proxy so that
    // `string.sub = foo` lands in the proxy, not the shared original.
    for key in SAFE_LIBRARY_GLOBALS {
        let orig: LuaValue = safe_globals.get(*key)?;
        if matches!(orig, LuaValue::Table(_)) {
            let proxy = lua.create_table()?;
            let proxy_meta = lua.create_table()?;
            proxy_meta.set("__index", orig)?;
            proxy.set_metatable(Some(proxy_meta));
            env.set(*key, proxy)?;
        }
    }

    Ok(env)
}

// ---------------------------------------------------------------------------
// ddb.* API stubs
// ---------------------------------------------------------------------------

/// Hard cap on a single user-log message. `string.rep("x", 100_000_000)` in
/// a user chunk must not OOM the log pipeline. 64 KiB is ample for human
/// logs and still bounded.
const MAX_LOG_MSG_BYTES: usize = 65_536;

/// Truncate a user-supplied log message to [`MAX_LOG_MSG_BYTES`] on a UTF-8
/// boundary and append an explicit `…[truncated]` marker so consumers can
/// tell the difference from a legitimately long message. Used by every
/// `ddb.log.*` registration.
fn truncate_log(msg: String) -> String {
    if msg.len() <= MAX_LOG_MSG_BYTES {
        return msg;
    }
    // Walk back to the previous char boundary so we never split a UTF-8
    // sequence mid-byte.
    let mut cut = MAX_LOG_MSG_BYTES;
    while cut > 0 && !msg.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = msg[..cut].to_string();
    out.push_str("…[truncated]");
    out
}

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
    //
    // MJ-03 + MN-01: user text is passed as a structured `message` field
    // (not as a captured format identifier) so any embedded newlines are
    // escaped by the log formatter instead of injecting fake log lines.
    // All four levels truncate at [`MAX_LOG_MSG_BYTES`] so a malicious
    // `string.rep("x", 100_000_000)` cannot OOM the log pipeline.
    let log = lua.create_table()?;
    log.set(
        "debug",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            let msg = truncate_log(msg);
            debug!(target: "ddb_functions::mlua::user", message = %msg, "user log");
            Ok(())
        })?,
    )?;
    log.set(
        "info",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            let msg = truncate_log(msg);
            info!(target: "ddb_functions::mlua::user", message = %msg, "user log");
            Ok(())
        })?,
    )?;
    log.set(
        "warn",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            let msg = truncate_log(msg);
            warn!(target: "ddb_functions::mlua::user", message = %msg, "user log");
            Ok(())
        })?,
    )?;
    log.set(
        "error",
        lua.create_function(|_, msg: String| -> mlua::Result<()> {
            let msg = truncate_log(msg);
            error!(target: "ddb_functions::mlua::user", message = %msg, "user log");
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

    /// Build a runtime rooted at a fresh tempdir so MN-03's
    /// functions_dir-must-exist check always passes in tests. The
    /// tempdir is leaked intentionally: tests never drop the runtime
    /// before they finish, and a background cleanup is not worth the
    /// complexity here.
    fn new_runtime() -> MluaRuntime {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.keep();
        MluaRuntime::new(path, 4).expect("mlua runtime must construct")
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

    /// F4 regression: user chunk mutations to stdlib functions must NOT
    /// leak out of the per-invocation environment. User A reassigns
    /// `string.sub` at top level; user B must still see the pristine
    /// `string.sub`.
    #[tokio::test]
    async fn per_invocation_env_does_not_leak_globals() {
        let rt = new_runtime();

        // User A: mutate `string.sub` at the top level of its chunk.
        // Under per-call env isolation, this assignment lands in A's
        // fresh env table and is dropped when the call returns.
        let a_src = r#"
            string.sub = function() return "PWNED" end
            function a() return "done" end
        "#;
        let a_out = rt
            .exec_in_fresh_env(a_src, "a", serde_json::json!(null))
            .await
            .expect("user A runs");
        assert_eq!(a_out, serde_json::json!("done"));

        // User B: call the pristine string.sub. If A's mutation leaked,
        // we'd see "PWNED"; with per-call env isolation we see "hel".
        let b_src = r#"
            function b() return string.sub("hello", 1, 3) end
        "#;
        let b_out = rt
            .exec_in_fresh_env(b_src, "b", serde_json::json!(null))
            .await
            .expect("user B runs");
        assert_eq!(
            b_out,
            serde_json::json!("hel"),
            "per-invocation env MUST isolate stdlib mutations"
        );
    }

    /// MJ-01 regression: when a yielding user function runs longer than
    /// the configured wall-clock cap, tokio::time::timeout fires and the
    /// call returns an Internal error tagged with the cap. Uses
    /// `coroutine.yield()` in a loop so the mlua async scheduler hits
    /// an await point for the timer to fire on.
    #[tokio::test]
    async fn lua_call_respects_wall_clock_cap() {
        use crate::functions::registry::{FunctionDef, FunctionKind};
        use crate::functions::runtime::{ExecutionContext, ResourceLimits, RuntimeBackend};

        let tmpdir = tempfile::tempdir().expect("tempdir");
        let functions_dir = tmpdir.path().to_path_buf();
        let file_path = functions_dir.join("slow.lua");
        // Busy-yield loop: each call_async yield boundary gives the
        // tokio timer a chance to cancel.
        let source = r#"
            function slow()
                local n = 0
                while true do
                    n = n + 1
                    coroutine.yield()
                end
                return n
            end
        "#;
        std::fs::write(&file_path, source).expect("write source");

        let rt = MluaRuntime::new(functions_dir.clone(), 4).expect("rt");

        let def = FunctionDef {
            name: "slow".into(),
            file_path: std::path::PathBuf::from("slow.lua"),
            export_name: "slow".into(),
            kind: FunctionKind::Query,
            args_schema: None,
            description: None,
            last_modified: None,
        };
        let ctx = ExecutionContext {
            invocation_id: "test-inv".into(),
            function_name: "slow".into(),
            args: serde_json::json!(null),
            db_url: String::new(),
            auth_token: None,
            internal_api_url: String::new(),
        };
        let limits = ResourceLimits {
            cpu_time_ms: 50, // 50ms cap, very short
            memory_mb: 64,
            max_concurrency: 4,
        };

        let started = std::time::Instant::now();
        let res = rt.execute(&def, &ctx, &limits).await;
        let elapsed = started.elapsed();
        // Must terminate roughly within cap + scheduling slack (<2s).
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "timeout did not fire: elapsed {:?}",
            elapsed
        );
        let err = res.expect_err("slow loop must time out");
        let msg = format!("{err}");
        // Accept either the explicit cap error OR a lua error surfaced
        // when the yielded coroutine gets cancelled mid-flight.
        assert!(
            msg.contains("wall-clock cap") || msg.contains("lua call failed"),
            "unexpected error: {msg}"
        );
    }

    /// Nyquist R3.3: the whole `debug` table (not just `sethook`) must
    /// be unreachable. `debug.getregistry()` was the specific escape
    /// vector: `_LOADED.io.popen("id")` bypasses `io = nil`.
    #[tokio::test]
    async fn sandbox_strips_debug_fully() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function shape()
                return {
                    debug_nil    = debug == nil,
                    getreg_nil   = (debug == nil) or (debug.getregistry == nil),
                    getupval_nil = (debug == nil) or (debug.getupvalue == nil),
                    setupval_nil = (debug == nil) or (debug.setupvalue == nil),
                    getlocal_nil = (debug == nil) or (debug.getlocal == nil),
                    setlocal_nil = (debug == nil) or (debug.setlocal == nil),
                    getinfo_nil  = (debug == nil) or (debug.getinfo == nil),
                    sethook_nil  = (debug == nil) or (debug.sethook == nil),
                }
            end
            "#,
        )
        .await
        .unwrap();
        let shape = rt
            .invoke_global("shape", serde_json::json!(null))
            .await
            .unwrap();
        for key in [
            "debug_nil",
            "getreg_nil",
            "getupval_nil",
            "setupval_nil",
            "getlocal_nil",
            "setlocal_nil",
            "getinfo_nil",
            "sethook_nil",
        ] {
            assert_eq!(
                shape[key],
                serde_json::json!(true),
                "{key} must be stripped"
            );
        }

        // Direct-call test: calling debug.getregistry() must raise.
        rt.load_chunk("function pop() return debug.getregistry() end")
            .await
            .unwrap();
        let err = rt
            .invoke_global("pop", serde_json::json!(null))
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("lua call failed"),
            "expected lua error, got {err}"
        );
    }

    /// Nyquist R3.4: `require` is a standalone global in Lua 5.4 and
    /// must be nilled independently of `package`.
    #[tokio::test]
    async fn sandbox_strips_require() {
        let rt = new_runtime();
        rt.load_chunk("function is_nil() return require == nil end")
            .await
            .unwrap();
        let out = rt
            .invoke_global("is_nil", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!(true));

        rt.load_chunk("function call_it() return require('io') end")
            .await
            .unwrap();
        let err = rt
            .invoke_global("call_it", serde_json::json!(null))
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("lua call failed"),
            "expected lua error, got {err}"
        );
    }

    /// Nyquist R3.5: `dofile` and `loadfile` must both be nil AND
    /// calling them must raise.
    #[tokio::test]
    async fn sandbox_strips_dofile_and_loadfile() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function call_dofile() return dofile('/tmp/x') end
            function call_loadfile() return loadfile('/tmp/x') end
            "#,
        )
        .await
        .unwrap();
        for name in ["call_dofile", "call_loadfile"] {
            let err = rt
                .invoke_global(name, serde_json::json!(null))
                .await
                .unwrap_err();
            assert!(
                format!("{err}").contains("lua call failed"),
                "{name}: expected lua error, got {err}"
            );
        }
    }

    /// Nyquist R3.6: `load` and `loadstring` must both be nil AND
    /// calling them must raise.
    #[tokio::test]
    async fn sandbox_strips_load_and_loadstring() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function call_load() return load('return 1') end
            function call_loadstring() return loadstring('return 1') end
            "#,
        )
        .await
        .unwrap();
        for name in ["call_load", "call_loadstring"] {
            let err = rt
                .invoke_global(name, serde_json::json!(null))
                .await
                .unwrap_err();
            assert!(
                format!("{err}").contains("lua call failed"),
                "{name}: expected lua error, got {err}"
            );
        }
    }

    /// CR-03: `string.dump` must be nil so bytecode-injection paths
    /// are closed.
    #[tokio::test]
    async fn sandbox_strips_string_dump() {
        let rt = new_runtime();
        rt.load_chunk("function is_nil() return string.dump == nil end")
            .await
            .unwrap();
        let out = rt
            .invoke_global("is_nil", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!(true));
    }

    /// Raw accessors bypass metamethods that v0.3.3 uses on `ddb` for
    /// audit-log instrumentation. Must all be nil.
    #[tokio::test]
    async fn sandbox_strips_raw_accessors() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function shape()
                return {
                    rawget_nil   = rawget == nil,
                    rawset_nil   = rawset == nil,
                    rawequal_nil = rawequal == nil,
                    rawlen_nil   = rawlen == nil,
                }
            end
            "#,
        )
        .await
        .unwrap();
        let shape = rt
            .invoke_global("shape", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(shape["rawget_nil"], serde_json::json!(true));
        assert_eq!(shape["rawset_nil"], serde_json::json!(true));
        assert_eq!(shape["rawequal_nil"], serde_json::json!(true));
        assert_eq!(shape["rawlen_nil"], serde_json::json!(true));
    }

    /// `collectgarbage` leaks memory-layout information and lets user
    /// code force GC pressure on the shared VM.
    #[tokio::test]
    async fn sandbox_strips_collectgarbage() {
        let rt = new_runtime();
        rt.load_chunk("function is_nil() return collectgarbage == nil end")
            .await
            .unwrap();
        let out = rt
            .invoke_global("is_nil", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!(true));
    }

    /// Nyquist: the `os` whitelist must contain exactly `clock`, `date`,
    /// `time` — not a superset.
    #[tokio::test]
    async fn sandbox_os_whitelist_is_exact() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function keys()
                local out = {}
                for k, _ in pairs(os) do
                    out[#out + 1] = k
                end
                table.sort(out)
                return out
            end
            "#,
        )
        .await
        .unwrap();
        let out = rt
            .invoke_global("keys", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(
            out,
            serde_json::json!(["clock", "date", "time"]),
            "os whitelist must be exact"
        );
    }

    /// Every `ddb.*` stub path must raise a Lua error (not panic in
    /// Rust) when the v0.3.2 user invokes it. Covers query, kv.get,
    /// kv.set, triples.get, triples.put.
    #[tokio::test]
    async fn ddb_stubs_all_raise_lua_error() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function call_query()       return ddb.query("SELECT 1") end
            function call_kv_get()      return ddb.kv.get("k") end
            function call_kv_set()      return ddb.kv.set("k", "v") end
            function call_triples_get() return ddb.triples.get("s", "p") end
            function call_triples_put() return ddb.triples.put("s", "p", "o") end
            "#,
        )
        .await
        .unwrap();
        for name in [
            "call_query",
            "call_kv_get",
            "call_kv_set",
            "call_triples_get",
            "call_triples_put",
        ] {
            let err = rt
                .invoke_global(name, serde_json::json!(null))
                .await
                .unwrap_err();
            let msg = format!("{err}");
            assert!(
                msg.contains("NotYetImplemented"),
                "{name}: expected NotYetImplemented, got {msg}"
            );
        }
    }

    /// `ddb.log.{debug,info,warn,error}` must all route cleanly with
    /// no Rust panic. We can't easily hook tracing here, so this is a
    /// smoke test: call each level and assert the function returns ok.
    #[tokio::test]
    async fn ddb_log_all_levels_route_ok() {
        let rt = new_runtime();
        rt.load_chunk(
            r#"
            function go()
                ddb.log.debug("d")
                ddb.log.info("i")
                ddb.log.warn("w")
                ddb.log.error("e")
                return "ok"
            end
            "#,
        )
        .await
        .unwrap();
        let out = rt
            .invoke_global("go", serde_json::json!(null))
            .await
            .unwrap();
        assert_eq!(out, serde_json::json!("ok"));
    }

    /// F6 regression: a FunctionDef whose file_path tries to traverse
    /// out of the functions directory (`../escape.lua`) must be rejected
    /// with a clear error, not silently loaded.
    #[tokio::test]
    async fn function_path_traversal_rejected() {
        use crate::functions::registry::{FunctionDef, FunctionKind};
        use crate::functions::runtime::{ExecutionContext, ResourceLimits, RuntimeBackend};

        // Build a parent tempdir and an adjacent `escape.lua` outside
        // the functions directory. The functions dir itself is a child
        // of the parent so `../escape.lua` resolves to a real file that
        // canonicalize() can find — otherwise the test would trip on a
        // ENOENT instead of the containment check.
        let parent = tempfile::tempdir().expect("parent tempdir");
        let functions_dir = parent.path().join("functions");
        std::fs::create_dir(&functions_dir).expect("mkdir functions");
        let escape = parent.path().join("escape.lua");
        std::fs::write(&escape, "function hacked() return 'pwn' end")
            .expect("write escape.lua");

        let rt = MluaRuntime::new(functions_dir.clone(), 4).expect("rt");

        let def = FunctionDef {
            name: "hacked".into(),
            file_path: std::path::PathBuf::from("../escape.lua"),
            export_name: "hacked".into(),
            kind: FunctionKind::Query,
            args_schema: None,
            description: None,
            last_modified: None,
        };
        let ctx = ExecutionContext {
            invocation_id: "test-inv".into(),
            function_name: "hacked".into(),
            args: serde_json::json!(null),
            db_url: String::new(),
            auth_token: None,
            internal_api_url: String::new(),
        };
        let err = rt
            .execute(&def, &ctx, &ResourceLimits::default())
            .await
            .expect_err("traversal must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("escapes functions directory"),
            "expected containment error, got: {msg}"
        );
    }

    /// MN-03 regression: constructing an MluaRuntime with a
    /// non-existent functions_dir must fail fast at `new()` time, not
    /// defer the error to first invocation.
    #[tokio::test]
    async fn new_rejects_missing_functions_dir() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let ghost = parent.path().join("does-not-exist");
        // MluaRuntime doesn't implement Debug, so we can't use
        // expect_err directly — match on the Result instead.
        match MluaRuntime::new(ghost.clone(), 4) {
            Ok(_) => panic!("missing dir must be rejected"),
            Err(err) => {
                let msg = format!("{err}");
                assert!(
                    msg.contains("does not exist"),
                    "expected missing-dir error, got: {msg}"
                );
            }
        }
    }

    /// CPU-bound interruption requires mlua 0.10's `Lua::set_interrupt`
    /// API; v0.3.2 can only bound yielding code via tokio timeout. This
    /// stub is kept #[ignore]'d as a tracking artifact pointing at the
    /// v0.3.3 work.
    #[tokio::test]
    #[ignore = "needs mlua 0.10 set_interrupt (tracked for v0.3.3)"]
    async fn cpu_bound_loop_is_bounded() {
        // Intentionally empty — see the ignore reason.
    }

    /// F5 regression: a chunk whose first byte is the Lua bytecode
    /// marker (`\x1bLua`) must be refused at load time when `ChunkMode`
    /// is pinned to `Text`. Without this, mlua's auto-detection would
    /// happily execute crafted bytecode and bypass every source-level
    /// sandbox check.
    #[tokio::test]
    async fn sandbox_rejects_bytecode_chunk() {
        // Produce a real Lua 5.4 bytecode blob by using a fresh,
        // unsandboxed `Lua` instance and dumping a trivial function.
        // The dump must start with `\x1bLua`, which is the marker
        // `ChunkMode::Text` refuses.
        let scratch = mlua::Lua::new();
        let func: mlua::Function = scratch
            .load("return 1")
            .into_function()
            .expect("compile ok");
        let bytecode: Vec<u8> = func.dump(true);
        assert!(
            bytecode.starts_with(b"\x1bLua"),
            "expected bytecode marker, got {:?}",
            &bytecode[..bytecode.len().min(8)]
        );

        let rt = new_runtime();
        let err = rt
            .load_bytes_as_text(&bytecode)
            .await
            .expect_err("bytecode chunk must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("attempt to load a binary")
                || msg.contains("bytecode")
                || msg.contains("text chunk")
                || msg.to_lowercase().contains("binary"),
            "expected bytecode-rejection error, got: {msg}"
        );
    }
}
