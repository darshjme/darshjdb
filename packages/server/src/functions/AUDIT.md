# Functions Module Audit

**Date:** 2026-04-05
**Scope:** `packages/server/src/functions/` (mod.rs, runtime.rs, registry.rs, validator.rs, scheduler.rs)
**Tests before:** 28 passing
**Tests after:** 113 passing (+85 new tests)

---

## Issues Found & Fixed

### 1. [CRITICAL] Subprocess not killed on timeout (runtime.rs)

**Severity:** Critical -- resource leak / zombie processes
**Lines:** ~327-340 (original)

The timeout handler mapped the error but never actually killed the child process. When a function exceeded its CPU time limit, the subprocess continued running indefinitely, consuming CPU, memory, and file descriptors.

**Fix:** Replaced the `.map_err()` chain with an explicit `match` that calls `child.kill().await` before returning `CpuTimeout`. The child is now forcefully terminated on timeout.

### 2. [HIGH] stdin pipe not closed after writing context (runtime.rs)

**Severity:** High -- potential deadlock
**Lines:** ~314-324 (original)

The `BufWriter` wrapping stdin was dropped without flushing or shutting down. In async context, drop does not guarantee a flush. The child process could hang forever waiting for EOF on stdin.

**Fix:** Added explicit `shutdown()` call on the `BufWriter` after `write_all()`, ensuring the pipe is flushed and closed so the child sees EOF.

### 3. [MEDIUM] ResourceLimits had no validation (runtime.rs)

**Severity:** Medium -- silent misconfiguration
**Lines:** 94-102 (original)

`ResourceLimits` accepted any values including:
- `cpu_time_ms: 0` -- instant timeout, no function could ever succeed
- `memory_mb: 0` -- subprocess launched with 0MB memory limit
- `max_concurrency: 0` -- creates a Semaphore(0) that permanently blocks all executions

**Fix:** Added `ResourceLimits::validate()` method that enforces:
- `cpu_time_ms`: 1..=300,000 (max 5 minutes)
- `memory_mb`: 1..=4,096
- `max_concurrency`: >= 1

### 4. [MEDIUM] Scheduler off-by-one in disable threshold (scheduler.rs)

**Severity:** Medium -- job disabled one tick late
**Lines:** ~419-430 (original)

The retry loop runs `max_retries + 1` times (initial attempt + retries). After all attempts fail, `consecutive_failures` is incremented. The check `consecutive_failures > max_retries` means the job only gets disabled on the *next* scheduler tick after already exhausting retries, causing one extra execution cycle.

**Fix:** Changed `>` to `>=` so the job is disabled immediately after exhausting its retry budget.

### 5. [LOW] Number validation silent bypass with NaN/Infinity (validator.rs)

**Severity:** Low (JSON parsers typically reject these, but defense-in-depth matters)
**Lines:** 152-177 (original)

`f64::NAN` comparisons always return `false`, so a NaN value would silently pass both min and max checks. Similarly, `f64::INFINITY` would pass min checks.

**Fix:** Added explicit `is_nan()` / `is_infinite()` guard before range comparisons.

---

## Issues Identified But Not Fixed (By Design)

### A. Export parser is line-based (registry.rs)

`parse_exports()` scans line-by-line and will miss multi-line export patterns like:
```ts
export const getUser =
  query({
    handler: async (ctx) => {},
  });
```

**Rationale:** This is a documented design choice. A proper fix requires a JS/TS AST parser (e.g., `swc` or `tree-sitter`), which is an architectural change (Rule 4). The line-based approach handles the vast majority of real-world patterns.

### B. Object validator allows extra fields (validator.rs)

The `Object` schema validates declared fields but ignores extra keys in the input. This is intentional for forward compatibility but could be a security concern if strict validation is desired.

**Recommendation:** Consider adding an optional `strict: bool` flag to `ArgSchema::Object` in a future iteration.

### C. `register_job` uses `block_in_place` (scheduler.rs)

`tokio::task::block_in_place` panics on a single-threaded runtime. Since DarshJDB uses `tokio::main` with the multi-threaded runtime, this works in practice. However, it would break unit tests using `#[tokio::test]` with `flavor = "current_thread"`.

**Recommendation:** Convert `register_job` to `async fn` in a future refactor.

### D. Hot reload scan is non-atomic with request handling (registry.rs)

During `scan_directory`, new files might be added or removed. The scan result is swapped atomically via `RwLock`, but the scan itself is not snapshot-isolated. This is acceptable for a development-only feature with debouncing.

---

## Test Coverage Added

### validator.rs (+35 tests)

| Category | Tests | Coverage |
|----------|-------|----------|
| String edge cases | 6 | Empty string, min=1 rejection, exact boundary, unicode char counting, null rejection, number rejection |
| Number edge cases | 8 | Exact min/max boundaries, zero, negative ranges, type mismatches, no-constraint extremes |
| Bool edge cases | 4 | false value, null rejection, integer 0/1 rejection (no truthy coercion) |
| ID edge cases | 4 | Null/number rejection, prefixed ID, plain string |
| Array edge cases | 5 | Empty array, non-array rejection, null rejection, nested arrays, error path indexing |
| Object edge cases | 5 | Empty schema, extra fields, non-object rejection, nested validation, optional fields |
| Optional edge cases | 1 | Wrong inner type |
| Type name coverage | 1 | All 5 JSON type names in error messages |
| Complex schemas | 1 | Deeply nested Object > Array > Object with mixed required/optional |

### runtime.rs (+17 tests, new module)

| Category | Tests | Coverage |
|----------|-------|----------|
| ResourceLimits validation | 7 | Zero/excessive cpu_time, zero/excessive memory, zero concurrency, boundary values, defaults |
| ProcessKind | 1 | Binary name mapping |
| build_command | 2 | Deno flags (--allow-net, --v8-flags, harness path), Node flags (--max-old-space-size) |
| Backend name | 1 | "deno-subprocess" / "node-subprocess" |
| ExecutionContext serde | 2 | Roundtrip with/without auth_token |
| ExecutionResult serde | 1 | Roundtrip with logs and peak_memory |
| Error display | 1 | All error variant messages contain expected substrings |

### registry.rs (+17 tests)

| Category | Tests | Coverage |
|----------|-------|----------|
| FunctionKind | 1 | Unknown/empty/case-sensitive wrapper name rejection |
| parse_exports | 7 | Scheduled, internalFn, space-before-paren, empty file, comments, non-function constants, mixed exports |
| is_function_file | 5 | .mjs/.mts acceptance, .spec exclusion, underscore exclusion, non-JS extensions, no extension |
| module_name_from_path | 2 | Nested paths, backslash normalization |
| has_function_extension | 3 | Mixed paths, no matches, empty list |

### scheduler.rs (+16 tests)

| Category | Tests | Coverage |
|----------|-------|----------|
| Cron parsing | 7 | Every-second, complex weekday, lists, empty string, too-few-fields, invalid range, error preservation |
| Advisory lock key | 3 | Empty string, similar strings diverge, long string determinism |
| JobStatus serde | 2 | Roundtrip all variants, camelCase output verification |
| ScheduledJob serde | 1 | Full roundtrip with DateTime |
| SchedulerError display | 1 | All error variant messages |

---

## Summary

- **5 issues fixed** (1 critical, 1 high, 2 medium, 1 low)
- **4 issues documented** as known limitations / future work
- **85 new tests** added across all 4 implementation files
- **113 total tests** passing in the functions module
- All changes are backward-compatible; no API signatures changed
