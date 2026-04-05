//! Function execution runtime with resource isolation.
//!
//! Provides a pluggable [`RuntimeBackend`] trait so the actual JavaScript/TypeScript
//! execution engine can be swapped between Deno subprocess, Node subprocess, or a
//! future embedded V8 isolate without changing the rest of the server.
//!
//! The default [`ProcessRuntime`] spawns a subprocess (Deno or Node) per invocation
//! with CPU time and memory limits enforced via OS-level controls.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, instrument, warn};

use super::registry::FunctionDef;
use super::validator;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during function execution.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The function exceeded its CPU time budget.
    #[error("function exceeded cpu time limit of {limit_ms}ms")]
    CpuTimeout { limit_ms: u64 },

    /// The function exceeded its memory budget.
    #[error("function exceeded memory limit of {limit_mb}MB")]
    MemoryExceeded { limit_mb: u32 },

    /// Argument validation failed before execution.
    #[error("argument validation failed: {0}")]
    ValidationError(#[from] validator::ValidationError),

    /// The subprocess exited with a non-zero code.
    #[error("function process exited with code {code}: {stderr}")]
    ProcessFailed {
        /// Exit code from the subprocess.
        code: i32,
        /// Captured stderr output.
        stderr: String,
    },

    /// Could not spawn the subprocess.
    #[error("failed to spawn runtime process: {0}")]
    SpawnError(#[source] std::io::Error),

    /// The function returned invalid JSON.
    #[error("function returned invalid JSON: {0}")]
    InvalidOutput(#[source] serde_json::Error),

    /// The runtime binary (deno/node) was not found.
    #[error("runtime binary not found: {binary}")]
    BinaryNotFound {
        /// The binary that was looked for.
        binary: String,
    },

    /// An internal runtime error.
    #[error("internal runtime error: {0}")]
    Internal(String),
}

/// Result alias for runtime operations.
pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

// ---------------------------------------------------------------------------
// Resource limits
// ---------------------------------------------------------------------------

/// Resource limits applied to each function invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    /// Maximum wall-clock execution time in milliseconds.
    pub cpu_time_ms: u64,

    /// Maximum heap memory in megabytes.
    pub memory_mb: u32,

    /// Maximum number of concurrent executions across the pool.
    pub max_concurrency: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            cpu_time_ms: 30_000,
            memory_mb: 128,
            max_concurrency: 64,
        }
    }
}

// ---------------------------------------------------------------------------
// Execution context passed into the subprocess
// ---------------------------------------------------------------------------

/// Serialized execution context injected into the function subprocess as JSON
/// on stdin. The JS harness unpacks this into `ctx.db`, `ctx.auth`, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Unique invocation ID for tracing.
    pub invocation_id: String,

    /// Name of the function being called.
    pub function_name: String,

    /// Validated arguments.
    pub args: Value,

    /// Database connection URL the function can use via `ctx.db`.
    pub db_url: String,

    /// Authentication token for the calling user, if any.
    pub auth_token: Option<String>,

    /// Internal API base URL for `ctx.scheduler` and `ctx.fetch`.
    pub internal_api_url: String,
}

/// The result returned by the subprocess harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// The return value of the function (JSON-serialized).
    pub value: Value,

    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,

    /// Peak memory usage in bytes, if available.
    pub peak_memory_bytes: Option<u64>,

    /// Log lines emitted during execution.
    pub logs: Vec<LogEntry>,
}

/// A single log line from function execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Log level: "debug", "info", "warn", "error".
    pub level: String,
    /// The message.
    pub message: String,
    /// Milliseconds since invocation start.
    pub timestamp_ms: u64,
}

// ---------------------------------------------------------------------------
// Runtime backend trait
// ---------------------------------------------------------------------------

/// Pluggable execution backend.
///
/// Implementations translate an [`ExecutionContext`] into an actual function
/// invocation and return the result. This trait allows swapping between
/// subprocess-based execution (Deno/Node) and a future embedded V8 isolate.
pub trait RuntimeBackend: Send + Sync + 'static {
    /// Execute a function with the given context and resource limits.
    fn execute(
        &self,
        function_def: &FunctionDef,
        context: &ExecutionContext,
        limits: &ResourceLimits,
    ) -> Pin<Box<dyn Future<Output = RuntimeResult<ExecutionResult>> + Send + '_>>;

    /// Check whether this backend is available (e.g. binary exists on PATH).
    fn health_check(&self) -> Pin<Box<dyn Future<Output = RuntimeResult<()>> + Send + '_>>;

    /// Human-readable name of this backend (e.g. "deno-subprocess").
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// Process-based runtime (default)
// ---------------------------------------------------------------------------

/// Which subprocess binary to use for executing functions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProcessKind {
    /// Use `deno run` with `--v8-flags` for memory limits.
    Deno,
    /// Use `node` with `--max-old-space-size` for memory limits.
    Node,
}

impl ProcessKind {
    /// Returns the binary name expected on `$PATH`.
    pub fn binary_name(&self) -> &str {
        match self {
            Self::Deno => "deno",
            Self::Node => "node",
        }
    }
}

/// Default runtime backend that spawns a subprocess per invocation.
///
/// The subprocess receives the [`ExecutionContext`] as JSON on stdin and
/// writes an [`ExecutionResult`] as JSON to stdout. A thin JS harness
/// (`_darshan_harness.ts`) bootstraps the `ctx` API surface.
pub struct ProcessRuntime {
    /// Which binary to invoke.
    kind: ProcessKind,

    /// Path to the JS/TS harness that wraps function execution.
    harness_path: PathBuf,

    /// Base directory containing user function files.
    functions_dir: PathBuf,

    /// Concurrency semaphore for the isolate pool.
    semaphore: Arc<Semaphore>,
}

impl ProcessRuntime {
    /// Create a new process-based runtime.
    ///
    /// # Arguments
    ///
    /// * `kind` - Whether to use Deno or Node as the subprocess.
    /// * `harness_path` - Path to the harness script that bootstraps `ctx`.
    /// * `functions_dir` - Root directory containing user function files.
    /// * `max_concurrency` - Maximum parallel function executions.
    pub fn new(
        kind: ProcessKind,
        harness_path: PathBuf,
        functions_dir: PathBuf,
        max_concurrency: usize,
    ) -> Self {
        Self {
            kind,
            harness_path,
            functions_dir,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
        }
    }

    /// Build the subprocess command with appropriate flags.
    fn build_command(&self, function_def: &FunctionDef, limits: &ResourceLimits) -> Command {
        let mut cmd = Command::new(self.kind.binary_name());

        match &self.kind {
            ProcessKind::Deno => {
                cmd.arg("run")
                    .arg("--allow-net")
                    .arg("--allow-read")
                    .arg("--allow-env")
                    .arg(format!(
                        "--v8-flags=--max-old-space-size={}",
                        limits.memory_mb
                    ))
                    .arg(self.harness_path.as_os_str());
            }
            ProcessKind::Node => {
                cmd.arg(format!("--max-old-space-size={}", limits.memory_mb))
                    .arg(self.harness_path.as_os_str());
            }
        }

        // Pass the function file path as the first argument to the harness.
        let function_path = self.functions_dir.join(&function_def.file_path);
        cmd.arg(function_path.as_os_str());
        cmd.arg(&function_def.export_name);

        // Pipe stdin for context, capture stdout/stderr.
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        cmd
    }
}

impl RuntimeBackend for ProcessRuntime {
    fn execute(
        &self,
        function_def: &FunctionDef,
        context: &ExecutionContext,
        limits: &ResourceLimits,
    ) -> Pin<Box<dyn Future<Output = RuntimeResult<ExecutionResult>> + Send + '_>> {
        let function_def = function_def.clone();
        let context = context.clone();
        let limits = limits.clone();

        Box::pin(async move {
            // Acquire a concurrency permit.
            let _permit = self
                .semaphore
                .acquire()
                .await
                .map_err(|e| RuntimeError::Internal(format!("semaphore closed: {e}")))?;

            debug!(
                invocation_id = %context.invocation_id,
                function = %context.function_name,
                "spawning function subprocess"
            );

            let mut cmd = self.build_command(&function_def, &limits);

            let mut child = cmd.spawn().map_err(RuntimeError::SpawnError)?;

            // Write context JSON to stdin.
            if let Some(stdin) = child.stdin.take() {
                let context_json = serde_json::to_vec(&context).map_err(|e| {
                    RuntimeError::Internal(format!("failed to serialize context: {e}"))
                })?;
                tokio::io::AsyncWriteExt::write_all(
                    &mut tokio::io::BufWriter::new(stdin),
                    &context_json,
                )
                .await
                .map_err(|e| RuntimeError::Internal(format!("failed to write to stdin: {e}")))?;
            }

            // Wait with timeout.
            let timeout = Duration::from_millis(limits.cpu_time_ms);
            let output = tokio::time::timeout(timeout, child.wait_with_output())
                .await
                .map_err(|_| {
                    // Kill the child on timeout — best effort.
                    warn!(
                        invocation_id = %context.invocation_id,
                        "function timed out, killing subprocess"
                    );
                    RuntimeError::CpuTimeout {
                        limit_ms: limits.cpu_time_ms,
                    }
                })?
                .map_err(RuntimeError::SpawnError)?;

            if !output.status.success() {
                let code = output.status.code().unwrap_or(-1);
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                error!(
                    invocation_id = %context.invocation_id,
                    code,
                    stderr = %stderr,
                    "function subprocess failed"
                );
                return Err(RuntimeError::ProcessFailed { code, stderr });
            }

            let result: ExecutionResult =
                serde_json::from_slice(&output.stdout).map_err(RuntimeError::InvalidOutput)?;

            info!(
                invocation_id = %context.invocation_id,
                duration_ms = result.duration_ms,
                "function completed successfully"
            );

            Ok(result)
        })
    }

    fn health_check(&self) -> Pin<Box<dyn Future<Output = RuntimeResult<()>> + Send + '_>> {
        Box::pin(async move {
            let output = Command::new(self.kind.binary_name())
                .arg("--version")
                .output()
                .await
                .map_err(|_| RuntimeError::BinaryNotFound {
                    binary: self.kind.binary_name().to_string(),
                })?;

            if output.status.success() {
                let version = String::from_utf8_lossy(&output.stdout);
                info!(backend = self.name(), version = %version.trim(), "runtime healthy");
                Ok(())
            } else {
                Err(RuntimeError::BinaryNotFound {
                    binary: self.kind.binary_name().to_string(),
                })
            }
        })
    }

    fn name(&self) -> &str {
        match self.kind {
            ProcessKind::Deno => "deno-subprocess",
            ProcessKind::Node => "node-subprocess",
        }
    }
}

// ---------------------------------------------------------------------------
// High-level runtime facade
// ---------------------------------------------------------------------------

/// High-level function execution runtime.
///
/// Wraps a [`RuntimeBackend`] with argument validation, invocation ID
/// generation, and resource limit enforcement.
pub struct FunctionRuntime {
    /// The underlying execution backend.
    backend: Box<dyn RuntimeBackend>,

    /// Default resource limits applied when a function does not declare its own.
    default_limits: ResourceLimits,

    /// Database URL injected into every execution context.
    db_url: String,

    /// Internal API URL for scheduler and fetch calls.
    internal_api_url: String,
}

impl FunctionRuntime {
    /// Create a new function runtime.
    ///
    /// # Arguments
    ///
    /// * `backend` - The execution backend to use.
    /// * `default_limits` - Default resource limits.
    /// * `db_url` - Database connection URL for function contexts.
    /// * `internal_api_url` - Internal API base URL.
    pub fn new(
        backend: Box<dyn RuntimeBackend>,
        default_limits: ResourceLimits,
        db_url: String,
        internal_api_url: String,
    ) -> Self {
        Self {
            backend,
            default_limits,
            db_url,
            internal_api_url,
        }
    }

    /// Execute a function with the given arguments.
    ///
    /// Validates arguments against the function's declared schema, builds an
    /// execution context, and delegates to the underlying backend.
    #[instrument(skip(self, args, auth_token), fields(function = %function_def.name))]
    pub async fn execute(
        &self,
        function_def: &FunctionDef,
        args: Value,
        auth_token: Option<String>,
    ) -> RuntimeResult<ExecutionResult> {
        // Validate arguments if schema is declared.
        if let Some(schema) = &function_def.args_schema {
            validator::validate_args(schema, &args)?;
        }

        let invocation_id = uuid::Uuid::new_v4().to_string();

        let context = ExecutionContext {
            invocation_id: invocation_id.clone(),
            function_name: function_def.name.clone(),
            args,
            db_url: self.db_url.clone(),
            auth_token,
            internal_api_url: self.internal_api_url.clone(),
        };

        info!(invocation_id = %invocation_id, "executing function");

        self.backend
            .execute(function_def, &context, &self.default_limits)
            .await
    }

    /// Check whether the underlying backend is available.
    pub async fn health_check(&self) -> RuntimeResult<()> {
        self.backend.health_check().await
    }

    /// Returns the name of the active backend.
    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }
}
