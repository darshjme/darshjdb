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

## v0.3.2 Post-Review Security + Correctness Fixes (2026-04-15)

Second pass on `feat/v0.3.2-mlua-runtime` closing all 5 Critical +
3 Major + 4 Minor findings from the post-sprint security / code
review audit. Branch tip advanced from `8f41338` through 8 fix
commits; no force-push, no rebase, no merge from main.

### Fix commits (ordered)

1. `029a787` — **security(mlua): comprehensively strip sandbox-escape paths**
   — CR-01 / CR-02 / CR-03. Nuke `debug` entirely (was only stripping
   `sethook`, so `debug.getregistry()._LOADED.io.popen` escaped).
   Nil `require` (separate global in 5.4, not reachable via `package`
   alone). Nil `string.dump`, `collectgarbage`, `rawget`, `rawset`,
   `rawequal`, `rawlen`. The `os` whitelist now builds a fresh
   table; the original `os` is not retained anywhere.

2. `512f472` — **security(mlua): require ChunkMode::Text on every load to reject bytecode**
   — F5. mlua auto-detected bytecode in `lua.load` and executed it,
   bypassing source-level validation. Every load now pins
   `ChunkMode::Text`. Adds `sandbox_rejects_bytecode_chunk` which
   dumps a real Lua function via a scratch VM and proves the
   production path refuses it.

3. `ced814c` — **security(mlua): per-invocation environment isolation**
   — F4. Each call now runs under a fresh env table whose library
   entries (`string`, `table`, `math`, `os`, `ddb`) are wrapped in
   per-call proxy tables whose `__index` falls through to a frozen
   `safe_globals` snapshot held in the Lua registry. Top-level
   `string.sub = function() end` in user A's chunk lands on the
   proxy and is dropped when the call returns; user B still sees
   the pristine `string.sub`. Adds
   `per_invocation_env_does_not_leak_globals` regression.

4. `413bc58` — **security(mlua): wall-clock timeout via tokio::time::timeout + call_async**
   — MJ-01 + F2. Switch from `func.call` (synchronous) to
   `func.call_async`, wrap in `tokio::time::timeout` sourcing the
   cap from `ResourceLimits::cpu_time_ms`. Yielding user code is
   bounded cleanly; CPU-bound interruption of non-yielding loops
   still needs mlua 0.10's `set_interrupt` API and is tracked as
   v0.3.3 via an `#[ignore]`'d `cpu_bound_loop_is_bounded` stub.
   Adds `lua_call_respects_wall_clock_cap` with a
   `coroutine.yield` loop and a 50ms cap.

5. `bcd23c6` — **fix(mlua): drop redundant semaphore — single Mutex<Lua> already serializes**
   — MJ-02. The old semaphore admitted N permits but every admitted
   task then locked the single `Mutex<Lua>`, so effective
   concurrency was always 1. Removed the field and the
   `acquire_owned` call; `_max_concurrency` kept as an ignored
   parameter for call-site compatibility. A `Pool<Lua>` is tracked
   for v0.4.

6. `2baddc6` — **security(mlua): structured tracing fields + 64KB cap on user log input**
   — MJ-03 + MN-01. `ddb.log.*` now passes user text as a
   structured `message = %msg` field (not as a captured format
   identifier), so embedded newlines land in a tagged field the
   formatter escapes. 64 KiB UTF-8-safe truncation with an
   explicit `…[truncated]` marker prevents `string.rep("x", 100M)`
   from OOM-ing the log pipeline.

7. `688dab4` — **security(mlua): canonicalize function_path and enforce containment**
   — F6 + MN-04 + MN-03. `function_def.file_path` was naively
   `join`'d onto `functions_dir` with no canonicalization —
   `"../../etc/passwd"` traversed out. `execute` now canonicalizes
   via `tokio::fs::canonicalize` and asserts the result
   `starts_with(functions_dir_canonical)`. Also fixes MN-04: the
   sync `std::fs::read_to_string` was previously held across the
   `Mutex<Lua>` guard; read now happens before the lock is
   acquired via `tokio::fs`. Also fixes MN-03: `MluaRuntime::new`
   validates + canonicalizes `functions_dir` at construction time
   so misconfiguration fails fast at boot. Adds
   `function_path_traversal_rejected` and
   `new_rejects_missing_functions_dir`.

8. `f4295a7` — **test(mlua): comprehensive sandbox + ddb.* stub coverage**
   — MN-02 + Nyquist R3.3-R3.7. Ten new tests covering every strip
   in `install_sandbox`, the `os` whitelist exactness, all four
   `ddb.log.*` level routing, and all five `ddb.*` stub error
   paths:
   - `sandbox_strips_debug_fully` (R3.3 + CR-01, includes
     direct-call probe of `debug.getregistry`)
   - `sandbox_strips_require` (R3.4 + CR-02)
   - `sandbox_strips_dofile_and_loadfile`
   - `sandbox_strips_load_and_loadstring`
   - `sandbox_strips_string_dump` (CR-03)
   - `sandbox_strips_raw_accessors`
   - `sandbox_strips_collectgarbage`
   - `sandbox_os_whitelist_is_exact` (R3.5, exact-match
     `{clock, date, time}`)
   - `ddb_stubs_all_raise_lua_error` (R3.6)
   - `ddb_log_all_levels_route_ok` (R3.7)

### Finding → commit map

| Finding | Severity | Commit |
| ------- | -------- | ------ |
| CR-01 (debug reachable) | Critical | `029a787` |
| CR-02 (require reachable) | Critical | `029a787` |
| CR-03 (string.dump reachable) | Critical | `029a787` |
| F4 (no per-call env isolation) | Critical | `ced814c` |
| F5 (bytecode chunks accepted) | Critical | `512f472` |
| F6 (function_path traversal) | Critical | `688dab4` |
| MJ-01 (no wall-clock cap) | Major | `413bc58` |
| MJ-02 (semaphore theatre) | Major | `bcd23c6` |
| MJ-03 (log-injection + no cap) | Major | `2baddc6` |
| MN-01 (no log length cap) | Minor | `2baddc6` |
| MN-02 (sandbox test gaps) | Minor | `f4295a7` |
| MN-03 (functions_dir not validated) | Minor | `688dab4` |
| MN-04 (sync read under Mutex) | Minor | `688dab4` |

### Test suite growth

- v0.3.2 sprint delivered: **8** unit tests in `functions::mlua::tests`
- Post-review fix pass delivered: **15** additional unit tests
  (+ 1 `#[ignore]`'d stub pointing at v0.3.3 `set_interrupt` work)
- New total: **23 passing + 1 ignored**

### Verification

```bash
cd /Users/darshjme/repos/darshandb/.claude/worktrees/v032-mlua-runtime
cargo check -p ddb-server                                  # default, ProcessRuntime path — green
cargo check -p ddb-server --features mlua-runtime          # mlua-runtime — green
cargo test  -p ddb-server --features mlua-runtime --lib functions::mlua::
  # 23 passed; 0 failed; 1 ignored
cargo clippy -p ddb-server --features mlua-runtime --lib
  # zero warnings in functions/mlua.rs
  # (pre-existing collapsible_if warnings in config/mod.rs are out of scope)
git log --oneline v0.3.1..HEAD
  # 8f41338 docs(sprint): v0.3.2 mlua runtime agent summary   (sprint tip)
  # 029a787 security(mlua): comprehensively strip sandbox-escape paths
  # 512f472 security(mlua): require ChunkMode::Text on every load to reject bytecode
  # ced814c security(mlua): per-invocation environment isolation
  # 413bc58 security(mlua): wall-clock timeout via tokio::time::timeout + call_async
  # bcd23c6 fix(mlua): drop redundant semaphore
  # 2baddc6 security(mlua): structured tracing fields + 64KB cap on user log input
  # 688dab4 security(mlua): canonicalize function_path and enforce containment
  # f4295a7 test(mlua): comprehensive sandbox + ddb.* stub coverage
```

### Not in scope (deferred to v0.3.3 / v0.4)

- **CPU-bound interruption** of non-yielding user code
  (`while true do end`) requires mlua 0.10's `Lua::set_interrupt`
  API. `lua_call_respects_wall_clock_cap` covers yielding code
  today; `cpu_bound_loop_is_bounded` is an `#[ignore]`'d stub
  tracking the v0.3.3 work.
- **Real concurrency** beyond the single `Mutex<Lua>` needs a
  `Pool<Lua>` (one VM per worker, checked out per call). Tracked
  for v0.4.
- **Memory caps** per invocation (`Lua::set_memory_limit`) — same
  v0.3.3 window as CPU interruption.

No push, no tag, no merge from main. Orchestrator integrates later.
