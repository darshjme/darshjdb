//! VYASA — Embedded V8 isolate function runtime.
//!
//! DarshJDB — created by Darshankumar Joshi (github.com/darshjme).
//!
//! Alternative to [`ProcessRuntime`](super::runtime::ProcessRuntime) for
//! deployments that want sub-millisecond cold starts on JavaScript server
//! functions. Instead of spawning a Deno/Node subprocess per invocation
//! (~100ms minimum cold start), this backend embeds V8 directly via
//! `deno_core` and runs the user function inside an isolate owned by the
//! server process.
//!
//! Behind the `v8` Cargo feature so default builds stay lean:
//! ```bash
//! cargo run -p ddb-server --features v8
//! ```
//!
//! Selected at runtime when `DDB_FUNCTION_RUNTIME=v8` and the feature is
//! compiled in; otherwise the server falls back to [`ProcessRuntime`].
//!
//! # Security
//!
//! The embedded isolate runs with **no** filesystem or network access by
//! default. We deliberately use `deno_core` (minimal V8 bindings) rather
//! than `deno_runtime` (which pulls in the full Deno permission system
//! plus `fs`/`net` ops). User functions only see `__ddb_ctx` — the same
//! shape the subprocess harness exposes — and whatever pure-JS globals
//! V8 provides.

#![cfg(feature = "v8")]

use std::fmt::Display;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use deno_core::url::Url;
use deno_core::{JsRuntime, PollEventLoopOptions, RuntimeOptions, v8};
use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use super::registry::FunctionDef;
use super::runtime::{
    ExecutionContext, ExecutionResult, LogEntry, ResourceLimits, RuntimeBackend, RuntimeError,
    RuntimeResult,
};

// ---------------------------------------------------------------------------
// V8Runtime
// ---------------------------------------------------------------------------

/// Embedded V8 isolate runtime.
///
/// Each call to [`V8Runtime::execute`] spins up a fresh [`JsRuntime`] on a
/// dedicated OS thread (V8 isolates are `!Send`), injects the execution
/// context as `globalThis.__ddb_ctx`, loads the user's function file as an
/// ES module, invokes the requested export, and returns the serialized
/// result.
///
/// A concurrency semaphore caps the number of simultaneously live isolates
/// at [`ResourceLimits::max_concurrency`] to bound memory.
pub struct V8Runtime {
    /// Base directory containing user function files. The runtime resolves
    /// `FunctionDef::file_path` relative to this root.
    functions_dir: PathBuf,

    /// Concurrency semaphore bounding simultaneously-live isolates.
    semaphore: Arc<Semaphore>,
}

impl V8Runtime {
    /// Create a new embedded V8 runtime rooted at `functions_dir`.
    ///
    /// `max_concurrency` caps how many isolates can be alive at once; each
    /// isolate carries its own V8 heap so this directly bounds memory.
    pub fn new(functions_dir: PathBuf, max_concurrency: usize) -> Self {
        Self {
            functions_dir,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
        }
    }
}

impl RuntimeBackend for V8Runtime {
    fn execute(
        &self,
        function_def: &FunctionDef,
        context: &ExecutionContext,
        limits: &ResourceLimits,
    ) -> Pin<Box<dyn Future<Output = RuntimeResult<ExecutionResult>> + Send + '_>> {
        let function_def = function_def.clone();
        let context = context.clone();
        let limits = limits.clone();
        let functions_dir = self.functions_dir.clone();
        let semaphore = self.semaphore.clone();

        Box::pin(async move {
            // Acquire a concurrency permit up-front so we don't spawn more
            // isolate threads than `max_concurrency`.
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|e| RuntimeError::Internal(format!("semaphore closed: {e}")))?;

            debug!(
                invocation_id = %context.invocation_id,
                function = %context.function_name,
                "spawning v8 isolate"
            );

            // V8 isolates are !Send. Run on a dedicated blocking thread so we
            // don't poison the tokio worker threads with thread-local V8 state.
            let join = tokio::task::spawn_blocking(move || {
                run_in_isolate(&functions_dir, &function_def, &context, &limits)
            });

            match join.await {
                Ok(inner) => inner,
                Err(join_err) => Err(RuntimeError::Internal(format!(
                    "v8 worker thread panicked: {join_err}"
                ))),
            }
        })
    }

    fn health_check(&self) -> Pin<Box<dyn Future<Output = RuntimeResult<()>> + Send + '_>> {
        Box::pin(async move {
            // Trivial self-test: spin up an isolate and evaluate `1 + 1`.
            // If V8 refuses to initialize at all (bad platform init, linker
            // mismatch, etc.) this surfaces immediately.
            let join = tokio::task::spawn_blocking(|| -> Result<(), String> {
                let mut rt = JsRuntime::new(RuntimeOptions::default());
                rt.execute_script("[v8-health-check]", "1 + 1")
                    .map_err(|e| format!("{e}"))?;
                Ok(())
            });

            match join.await {
                Ok(Ok(())) => {
                    info!(backend = "v8-embedded", "runtime healthy");
                    Ok(())
                }
                Ok(Err(e)) => Err(RuntimeError::Internal(format!(
                    "v8 health check failed: {e}"
                ))),
                Err(e) => Err(RuntimeError::Internal(format!(
                    "v8 health worker panicked: {e}"
                ))),
            }
        })
    }

    fn name(&self) -> &str {
        "v8-embedded"
    }
}

// ---------------------------------------------------------------------------
// Isolate driver (runs on a dedicated blocking thread)
// ---------------------------------------------------------------------------

/// Drive a single function invocation inside its own `JsRuntime`.
///
/// This function is synchronous from tokio's point of view (it's called
/// inside `spawn_blocking`) but internally builds a single-thread tokio
/// runtime to drive `deno_core`'s async event loop.
fn run_in_isolate(
    functions_dir: &std::path::Path,
    function_def: &FunctionDef,
    context: &ExecutionContext,
    limits: &ResourceLimits,
) -> RuntimeResult<ExecutionResult> {
    // 1. Resolve the user's function source path.
    let source_path = functions_dir.join(&function_def.file_path);
    let source_code = std::fs::read_to_string(&source_path).map_err(|e| {
        RuntimeError::Internal(format!(
            "failed to read function source {}: {e}",
            source_path.display()
        ))
    })?;

    // 2. Serialize the execution context once so we can embed it as a
    //    literal string in the preamble.
    let ctx_json = serde_json::to_string(context)
        .map_err(|e| RuntimeError::Internal(format!("failed to serialize context: {e}")))?;

    // 3. Build a single-thread tokio runtime to drive the event loop.
    //    `JsRuntime` is tightly bound to the current thread and uses async
    //    internally (promises, microtasks).
    let tokio_rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| {
            RuntimeError::Internal(format!("failed to build tokio current_thread rt: {e}"))
        })?;

    let started = Instant::now();
    let invocation_id = context.invocation_id.clone();
    let export_name = function_def.export_name.clone();
    let cpu_limit = Duration::from_millis(limits.cpu_time_ms);

    let value: Value = tokio_rt.block_on(async move {
        let mut js_runtime = JsRuntime::new(RuntimeOptions::default());

        // Spawn a watchdog on a separate OS thread that will terminate the
        // isolate if it exceeds the CPU budget. deno_core's IsolateHandle is
        // thread-safe precisely for this use case.
        let isolate_handle = js_runtime.v8_isolate().thread_safe_handle();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let watchdog = std::thread::spawn(move || {
            match stop_rx.recv_timeout(cpu_limit) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // Normal completion — worker signalled us or hung up.
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    warn!(invocation_id = %invocation_id, "v8 cpu budget exceeded, terminating isolate");
                    isolate_handle.terminate_execution();
                }
            }
        });

        // 4. Inject __ddb_ctx as a global.
        let preamble = format!("globalThis.__ddb_ctx = {ctx_json};");
        js_runtime
            .execute_script("[ddb:preamble]", preamble)
            .map_err(map_err)?;

        // 5. Load the user file as a side ES module.
        let module_specifier = Url::from_file_path(&source_path).map_err(|_| {
            RuntimeError::Internal(format!(
                "function source path is not absolute: {}",
                source_path.display()
            ))
        })?;

        let mod_id = js_runtime
            .load_side_es_module_from_code(&module_specifier, source_code)
            .await
            .map_err(map_err)?;

        // Drive mod_evaluate and the event loop concurrently — top-level
        // await in the user module would otherwise deadlock.
        let eval_fut = Box::pin(js_runtime.mod_evaluate(mod_id));
        js_runtime
            .with_event_loop_promise(eval_fut, PollEventLoopOptions::default())
            .await
            .map_err(map_err)?;

        // 6. Pull the module namespace, look up the requested export, and
        //    prepare a `v8::Global<v8::Function>` so we can drive the call
        //    through deno_core's call/event-loop plumbing.
        let namespace = js_runtime.get_module_namespace(mod_id).map_err(map_err)?;

        let fn_global: v8::Global<v8::Function> = {
            deno_core::scope!(scope, js_runtime);
            let ns_local = v8::Local::new(scope, namespace);
            let ns_obj: v8::Local<v8::Object> = ns_local
                .try_into()
                .map_err(|_| RuntimeError::Internal("module namespace is not an object".into()))?;

            let export_key = v8::String::new(scope, &export_name)
                .ok_or_else(|| RuntimeError::Internal("failed to allocate export key".into()))?;
            let export_val = ns_obj
                .get(scope, export_key.into())
                .ok_or_else(|| RuntimeError::Internal(format!("export '{export_name}' not found")))?;

            let export_fn: v8::Local<v8::Function> = export_val.try_into().map_err(|_| {
                RuntimeError::Internal(format!("export '{export_name}' is not a function"))
            })?;

            v8::Global::new(scope, export_fn)
        };

        // 7. Build the __ddb_ctx argument as a Global<Value>.
        let ctx_arg_global: v8::Global<v8::Value> = {
            deno_core::scope!(scope, js_runtime);
            let global = scope.get_current_context().global(scope);
            let ctx_key = v8::String::new(scope, "__ddb_ctx").unwrap();
            let ctx_val = global.get(scope, ctx_key.into()).ok_or_else(|| {
                RuntimeError::Internal("__ddb_ctx vanished from globalThis".into())
            })?;
            v8::Global::new(scope, ctx_val)
        };

        // 8. Call the function through deno_core, which returns a future
        //    that resolves to the function's return value (driving promises
        //    if necessary). We drive the event loop alongside it.
        let call_fut = Box::pin(js_runtime.call_with_args(&fn_global, &[ctx_arg_global]));
        let return_global = js_runtime
            .with_event_loop_promise(call_fut, PollEventLoopOptions::default())
            .await
            .map_err(map_err)?;

        // 9. Serialize the returned value to serde_json::Value.
        let json_value: Value = {
            deno_core::scope!(scope, js_runtime);
            let local = v8::Local::new(scope, return_global);
            deno_core::serde_v8::from_v8(scope, local).map_err(|e| {
                RuntimeError::Internal(format!("failed to convert v8 value to json: {e}"))
            })?
        };

        // Tell the watchdog we finished cleanly.
        let _ = stop_tx.send(());
        drop(stop_tx);
        let _ = watchdog.join();

        Ok::<Value, RuntimeError>(json_value)
    })?;

    let duration_ms = started.elapsed().as_millis() as u64;

    Ok(ExecutionResult {
        value,
        duration_ms,
        peak_memory_bytes: None,
        logs: Vec::<LogEntry>::new(),
    })
}

/// Convert any `Display` error into our local [`RuntimeError`]. deno_core
/// exposes `CoreError`, `Box<JsError>`, and a few other concrete error
/// types from different APIs; a `Display`-bounded helper lets us use one
/// `.map_err(map_err)` call at every site.
fn map_err<E: Display>(e: E) -> RuntimeError {
    RuntimeError::Internal(format!("v8 runtime error: {e}"))
}
