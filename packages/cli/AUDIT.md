# DarshJDB CLI Security & Quality Audit

**Date:** 2026-04-05
**Scope:** `packages/cli/src/main.rs`, `packages/cli/src/config.rs`
**Tooling:** `cargo check`, `cargo clippy -- -D warnings` (both pass clean)

---

## Issues Found & Fixed

### 1. Missing authentication guard on remote commands (HIGH)

**Before:** Every command that contacts the server (`push`, `pull`, `seed`, `migrate`, `logs`, `auth`, `backup`, `restore`, `status`) silently sent an empty `Bearer` token when no token was configured. The server would reject the request with a generic HTTP error, giving users no actionable feedback.

**Fix:** Added `Config::require_token()` method in `config.rs` that returns a clear error message with resolution steps. Called at the top of every remote-server command handler. Local-only commands (`dev`, `init`) are unaffected.

**Files:** `config.rs` (new method), `main.rs` (9 call sites added)

---

### 2. URL parameter injection in `cmd_logs` (MEDIUM)

**Before:** The `--level` argument was interpolated directly into the URL query string without validation:
```rust
url.push_str(&format!("&level={l}"));
```
A crafted value like `--level "error&admin=true"` could inject additional query parameters.

**Fix:** Added an allowlist of valid log levels (`debug`, `info`, `warn`, `error`) as `Config::VALID_LOG_LEVELS`. The `cmd_logs` handler now validates the `--level` value against this list before constructing the URL, bailing with a clear error for invalid values.

**Files:** `config.rs` (const added), `main.rs` (validation block in `cmd_logs`)

---

### 3. Bare `unwrap()` on `ProgressStyle::with_template` (LOW)

**Before:** Two helper functions used `.unwrap()` on template construction:
```rust
ProgressStyle::with_template("...").unwrap()
```
While these are compile-time-known string literals that cannot fail in practice, bare `unwrap()` violates the project's error-handling discipline and obscures intent.

**Fix:** Replaced with `.expect("hard-coded ... template must be valid")` to document the invariant. If the indicatif API ever changes template syntax, the panic message will be actionable.

**Files:** `main.rs` (`spinner()` and `progress_bar()` helpers)

---

### 4. Silent error swallowing in JSON formatting (LOW)

**Before:** Two call sites used `serde_json::to_string_pretty(&body).unwrap_or_default()`, which would silently print an empty string if serialization failed (e.g., on non-UTF-8 sequences in server responses).

**Fix:** Replaced with proper `?` propagation using `.context()` for actionable error messages.

**Files:** `main.rs` (migration status display, user list display)

---

## Items Reviewed & Confirmed Safe

| Area | Status | Notes |
|------|--------|-------|
| Shell command injection | **Safe** | All external process calls (`docker`, `cargo`) use `tokio::process::Command` with explicit arg arrays -- never `sh -c` or string interpolation into shell commands |
| `file_stem().unwrap_or_default()` | **Safe** | Only reached after filtering for `.ts`/`.js` extensions, so `file_stem()` always returns `Some` |
| Config file parsing | **Safe** | Uses `toml::from_str` with `anyhow::Context` wrapping; malformed TOML produces clear errors |
| Directory traversal in `find_config_file` | **Safe** | Walks upward from CWD, only reads `ddb.toml` -- no user-controlled path components |
| Clap argument definitions | **Complete** | Every command and subcommand has `///` doc-comment help text; all arguments have help text via doc-comments; `--version` propagates correctly |
| Error types | **Adequate** | `anyhow::Result` with `.context()` throughout; no need for custom error enums at this CLI layer |
| `unwrap_or(false)` on Docker check | **Safe** | Intentional fallback -- Docker being unavailable is a valid state, not an error |

---

## Build Verification

```
cargo check -p ddb-cli        -> OK (0 errors, 0 warnings)
cargo clippy -p ddb-cli -- -D warnings -> OK (0 errors, 0 warnings)
```
