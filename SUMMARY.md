# v0.3.2 Sprint — Agent 3 (mlua runtime) summary

Branch: `feat/v0.3.2-mlua-runtime`
Base: `v0.3.1` (commit `32e9b18`)

## Landed

- **Cargo wiring**: new `mlua-runtime` feature in `packages/server/Cargo.toml`
  pulling `mlua = "0.10"` with features `lua54 + vendored + async +
  serialize + send`. `vendored` means no system Lua is required, `send`
  lets the VM cross tokio task boundaries, `serialize` provides the
  serde bridge between `serde_json::Value` and Lua values. The `send`
  feature was added after the first build failed on `Lua: !Send`.
- **`MluaRuntime`** in `packages/server/src/functions/mlua.rs`:
  - Implements the existing `RuntimeBackend` trait — `execute`,
    `health_check`, `name` — so it slots in wherever `ProcessRuntime`
    or `V8Runtime` do.
  - Holds a single shared `mlua::Lua` behind a `tokio::sync::Mutex`
    (Lua is `!Sync` even with the `send` feature).
  - Concurrency bounded by a `tokio::sync::Semaphore` sized from
    `ResourceLimits::max_concurrency`.
  - `execute()` reads the user `.lua` source from disk, loads it with
    `set_name(file_path)` for tracebacks, pulls the requested export
    off `globals()`, calls it with JSON-serialized args via
    `LuaSerdeExt`, and returns the result converted back to
    `serde_json::Value`.
  - `health_check()` does a trivial `return 1 + 1` eval.
- **Sandbox** (`install_sandbox`):
  - `io` → `nil`.
  - `package` → `nil` (disables `require`).
  - `dofile`, `loadfile`, `load`, `loadstring` → `nil`.
  - `debug.sethook` → `nil`.
  - `os` is **replaced** with a fresh whitelisted table containing only
    `os.time`, `os.date`, `os.clock` copied from the original. Every
    other `os.*` — including `os.execute`, `os.exit`, `os.remove`,
    `os.rename`, `os.getenv`, `os.setenv` — becomes unreachable.
- **`ddb.*` API shape** (`install_ddb_api`):
  - `ddb.query(sql)` — stub, raises `NotYetImplemented` Lua error.
  - `ddb.kv.get(k)` / `ddb.kv.set(k, v)` — stubs.
  - `ddb.triples.get(s, p)` / `ddb.triples.put(s, p, o)` — stubs.
  - `ddb.log.debug|info|warn|error(msg)` — **fully wired** into
    `tracing` under the `ddb_functions::mlua::user` target.
- **Module re-exports**: `functions/mod.rs` gains
  `#[cfg(feature = "mlua-runtime")] pub mod mlua;` and a re-export of
  `MluaRuntime`, mirroring the existing `v8` pattern.
- **8 unit tests** under `#[cfg(all(test, feature = "mlua-runtime"))]`,
  all green:
  1. `invoke_trivial_double` — registers `double(x)=x*2`, calls with 5,
     asserts result is 10.
  2. `sandbox_blocks_os_execute` — asserts `os.execute == nil` AND that
     directly calling `os.execute("echo pwned")` raises a Lua error.
  3. `sandbox_blocks_io_and_require_and_loaders` — asserts `io`,
     `package`, `dofile`, `loadfile`, `load` are all `nil`.
  4. `os_whitelist_still_has_time` — asserts `os.time()` remains callable.
  5. `ddb_log_info_is_live` — calls `ddb.log.info("hello")`, asserts the
     wrapping function returns normally.
  6. `ddb_query_stub_errors_clearly` — asserts `ddb.query("SELECT 1")`
     raises an error containing `NotYetImplemented`.
  7. `backend_name_is_mlua_embedded` — asserts `name() == "mlua-embedded"`.
  8. `health_check_passes` — asserts the VM health eval succeeds.

## Deferred (v0.3.3)

- Real wiring for `ddb.query`, `ddb.kv.{get,set}`, `ddb.triples.{get,put}`.
  The API shape is locked in; only the closure bodies need swapping.
- Resource limit enforcement. `execute()` currently ignores
  `_limits.cpu_time_ms` / `_limits.memory_mb`; mlua 0.10 exposes
  `Lua::set_hook` and `set_memory_limit` which can be wired in v0.3.3.
- Per-function compiled-chunk caching. Today `execute()` re-reads and
  re-loads the `.lua` source on every invocation.
- `main.rs` dispatch. The instructions explicitly said not to touch
  `main.rs`; the current selection logic at
  `packages/server/src/main.rs:739` only knows about `v8`. A
  post-merge follow-up needs to extend it to also recognize
  `DDB_FUNCTION_RUNTIME=mlua` when `mlua-runtime` is compiled in. The
  re-export in `functions/mod.rs` is ready.
- Execution log capture. `ExecutionResult.logs` is returned empty —
  `ddb.log.*` currently goes straight to `tracing` without populating a
  per-invocation buffer.

## Blocked

Nothing blocked.

## Files touched

- `packages/server/Cargo.toml` — added `mlua` optional dep +
  `mlua-runtime` feature. Every new line carries
  `# TODO(v0.3.2-sprint-merge):` so the orchestrator can verify
  non-collision during the 3-way merge with Agent 1.
- `packages/server/src/functions/mod.rs` — feature-gated
  `pub mod mlua;` + `pub use self::mlua::MluaRuntime;`.
- `packages/server/src/functions/mlua.rs` — **new**.
- `Cargo.lock` — auto-updated to include `mlua 0.10.5` and vendored
  Lua 5.4.
- `SUMMARY.md` — this file.

## Commit graph

```
391e58c feat(functions): MluaRuntime skeleton + sandbox + ddb.* API + tests
253f3d3 feat(functions): mlua 0.10 dep behind mlua-runtime feature
```

(plus the final `docs(sprint)` commit for this summary)

## Verification

Run from the worktree root:

```bash
cargo check -p ddb-server
cargo check -p ddb-server --features mlua-runtime
cargo test  -p ddb-server --features mlua-runtime --lib functions::mlua::
cargo clippy -p ddb-server --features mlua-runtime --lib
```

Local results:

- `cargo check -p ddb-server` — green, 1m 32s.
- `cargo check -p ddb-server --features mlua-runtime` — green, 48.94s.
- `cargo test -p ddb-server --features mlua-runtime --lib functions::mlua::`
  — `test result: ok. 8 passed; 0 failed; 0 ignored`.
- `cargo clippy -p ddb-server --features mlua-runtime --lib` — 4 pre-
  existing warnings in unrelated files (e.g. `main.rs` collapsible_if).
  Zero warnings in `functions/mlua.rs`.

## Cross-agent notes

- **`packages/server/Cargo.toml` collision risk with Agent 1 (sqlite-store)**:
  my additions are a single `mlua = { ... }` block under
  `[dependencies]` appended after the `rusqlite` line, and a single
  `mlua-runtime = ["dep:mlua"]` block under `[features]` appended after
  the `sqlite-store` line. Every line is tagged with a trailing
  `# TODO(v0.3.2-sprint-merge):` comment. If Agent 1 appends their own
  lines in the same region, resolution is to keep both sets of
  additions side by side — no semantic conflict.
- **Workspace `Cargo.toml`**: untouched.
- **`packages/server/src/main.rs`**: untouched.
- **No files under `store/**`, `query/**`, or `migrations/**` were
  touched** — those are Agent 1 / Agent 2 territory.
