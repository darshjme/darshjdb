# DarshJDB Workspace Audit -- Improvement Log

**Date:** 2026-04-05
**Rust toolchain:** Edition 2024, stable
**Result:** All 4 gates pass -- `cargo check`, `cargo clippy -D warnings`, `cargo fmt --check`, `cargo test`

---

## Summary

| Gate | Before | After |
|------|--------|-------|
| `cargo check` | PASS | PASS |
| `cargo clippy -D warnings` | 16 errors | 0 errors, 0 warnings |
| `cargo fmt --check` | ~30 formatting diffs | 0 diffs |
| `cargo test` | Would not compile (test targets) | 438 passed, 0 failed |

---

## Fixes Applied

### 1. Missing `KeyManager::generate()` and `KeyManager::from_secret()` (compilation error)

**File:** `packages/server/src/auth/session.rs`

The server binary (`main.rs`) called `KeyManager::generate()` and `KeyManager::from_secret()` which did not exist. Only `KeyManager::new()` (RSA PEM-based) and a partial `dev()` helper existed.

**Fix:** Added two public constructors:
- `from_secret(secret: &[u8]) -> Self` -- creates an HS256 (HMAC) key manager from a shared secret, suitable for single-node or dev deployments.
- `generate() -> Self` -- creates an ephemeral HMAC key manager with a random 64-byte secret (tokens do not survive restart).

Also made `sign_access_token` and `validate_access_token` use the `algorithm` field stored on the `KeyManager` struct instead of hardcoding `Algorithm::RS256`, enabling both RS256 (production RSA) and HS256 (dev/secret) modes.

### 2. Private `ErrorCode::status()` method (test compilation error)

**File:** `packages/server/src/api/error.rs`

The `error_code_status_mapping` test in `rest.rs` called `ErrorCode::status()`, but the method was `fn` (private).

**Fix:** Changed visibility to `pub(crate) fn status(self) -> StatusCode`.

### 3. Invalid `Next::new()` in test (compilation error)

**File:** `packages/server/src/api/rest.rs`

The `rate_limit_headers_injected` test attempted to construct an `axum::middleware::Next` via `Next::new()`, which does not exist as a public API in axum 0.8. The test body only validated header value formats and never actually invoked the middleware.

**Fix:** Removed the dead `Request` + `Next::new()` construction. Converted from `#[tokio::test] async fn` to a plain `#[test] fn` since no async work is performed.

### 4. Unused import `Broadcaster` (clippy: `unused-imports`)

**File:** `packages/server/src/main.rs`

`use ddb_server::sync::broadcaster::Broadcaster` was imported but never used.

**Fix:** Removed the import.

### 5. Unused import `EncodingKey, DecodingKey` in test helper (clippy: `unused-imports`)

**File:** `packages/server/src/auth/session.rs` (test module)

The `generate_rsa_keypair()` test helper imported `jsonwebtoken::{EncodingKey, DecodingKey}` but never used them -- it loads PEM fixtures from disk.

**Fix:** Removed the unused import and updated the comment.

### 6. Collapsible `if` statements (clippy: `collapsible-if`) -- 3 instances

**Files:**
- `packages/cli/src/main.rs` -- `if let Some(l) = level { if !contains(...) }` collapsed with `&&`
- `packages/server/src/api/rest.rs` -- `if let Some(first) = ... { if !is_ascii_alphabetic ... }` collapsed with `&&`
- `packages/server/src/sync/registry.rs` -- `if !still_has_session { if let Some(mut entry) = ... }` collapsed with `&&`

### 7. `clone()` on `Copy` type (clippy: `clone-on-copy`)

**File:** `packages/server/src/storage/mod.rs`

`received.clone()` on a `GenericArray<u8, ...>` which implements `Copy`.

**Fix:** Changed to `*received` (dereference).

### 8. Unnecessary `splitn` / manual `split_once` (clippy: `needless-splitn`, `manual-split-once`)

**File:** `packages/server/src/auth/providers.rs`

- `state1.splitn(2, '.').next()` -- `splitn` unnecessary when only taking first element; changed to `split('.').next()`.
- `state2.splitn(2, '.').nth(1)` -- manual reimplementation of `split_once`; changed to `state2.split_once('.').unwrap().1`.

### 9. `map_or(false, ...)` instead of `is_some_and(...)` (clippy: `unnecessary-map-or`)

**File:** `packages/server/src/query/mod.rs`

Changed `p.as_str().map_or(false, |s| s.contains("dangerous"))` to `p.as_str().is_some_and(|s| s.contains("dangerous"))`.

### 10. Formatting (rustfmt) -- 9 files

Ran `cargo fmt --all` to fix ~30 formatting inconsistencies across 9 files. Primary patterns: long `assert_eq!` macro calls, method chain formatting, struct literal formatting, block expression formatting.

---

## Test Results

```
test result: ok. 438 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
Doc-tests:   ok. 3 passed; 0 failed; 1 ignored
```
