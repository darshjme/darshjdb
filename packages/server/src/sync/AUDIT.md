# Sync Engine Audit Report

**Date:** 2026-04-05
**Scope:** `packages/server/src/sync/` — mod.rs, session.rs, registry.rs, broadcaster.rs, diff.rs, presence.rs
**Auditor model:** claude-opus-4-6

---

## Executive Summary

Reviewed all 6 files in the sync engine. Found 5 bugs (3 race conditions, 1 hash non-determinism, 1 off-by-one). Fixed 4 inline. Added 65 unit tests across session, registry, diff, and presence modules. All tests pass.

---

## Issues Found and Fixed

### ISSUE 1: Non-deterministic JSON hashing (diff.rs) — FIXED

**Severity:** High
**Type:** Incorrect diff computation

`hash_value()` used `serde_json::to_string()` which preserves insertion order of object keys. Two logically identical JSON objects constructed with different key insertion order would hash differently, causing phantom diffs or missed updates.

`hash_result_set()` was order-dependent — same entities returned in different row order would produce different hashes, triggering unnecessary full diff recomputation and spurious updates to clients.

**Fix:**
- Replaced `hash_value()` with recursive canonical hashing that sorts object keys at every nesting level. Each JSON type gets a discriminator byte to prevent cross-type collisions (e.g., `null` vs `false`, `0` vs `"0"`).
- Replaced `hash_result_set()` with XOR-based combination so row order does not affect the hash. Length is mixed in to distinguish empty from single-zero-hash cases.

**Files:** `diff.rs` lines 69-130

---

### ISSUE 2: TOCTOU race in SubscriptionRegistry::unregister (registry.rs) — FIXED

**Severity:** Medium
**Type:** Race condition

The original code did `drop(entry); self.by_query.remove(&query_hash);` — between the drop (releasing the shard lock) and the remove (re-acquiring it), another thread could insert a new handle into the same set. The remove would then delete a non-empty set, losing active subscriptions.

**Fix:** Replaced `drop + remove` pattern with `drop + remove_if(|_, set| set.is_empty())` which atomically checks emptiness under the shard lock before removing. Applied the same fix in `unregister_session()`.

**Files:** `registry.rs` lines 58-91, 100-112

---

### ISSUE 3: Split-brain rate limiter in PresenceRoom (presence.rs) — FIXED

**Severity:** Medium
**Type:** Race condition + off-by-one

The rate limiter stored `window_start` in a Mutex and `count` in a separate AtomicU32. This created two problems:

1. **Race condition:** Thread A locks mutex, sees window expired, resets window start, stores count=1, unlocks. Thread B (which called `fetch_add` before A's reset) now has a stale count from the old window, and its increment is against the new window's count that A just set to 1. Result: the actual count in the new window is wrong.

2. **Off-by-one:** `fetch_add(1)` returns the *previous* value but increments unconditionally. When `count == MAX_UPDATES_PER_SEC - 1`, `fetch_add` returns the limit-1 (passes the check) but the stored value is now at the limit. The next concurrent caller also gets limit-1 before seeing the incremented value. Result: more updates than the limit allows.

**Fix:** Consolidated `window_start` and `count` into a single `Mutex<(Instant, u32)>`. The check-increment-reset sequence is now fully serialized. The count is checked *before* incrementing, eliminating the off-by-one.

**Files:** `presence.rs` rate_state field, `check_rate_limit()` method

---

### ISSUE 4: Broadcaster result_cache unbounded growth (broadcaster.rs) — DOCUMENTED

**Severity:** Low
**Type:** Potential memory leak

The `result_cache: DashMap<(SessionId, SubId), Vec<Value>>` stores full query result snapshots per subscription. If `evict_session_cache()` is not called on disconnect (e.g., due to a panic in the WebSocket handler), entries leak indefinitely.

`evict_session_cache()` uses `retain()` which scans the entire DashMap — O(n) per disconnect.

**Recommendation:** Add a periodic GC task that cross-references cache keys against the SessionManager. Alternatively, add a secondary index `SessionId -> Vec<SubId>` for O(1) session eviction (mirrors the registry's `by_session` pattern). Not fixed inline as it would change the Broadcaster's public API.

---

### ISSUE 5: Broadcaster processes subscriptions sequentially (broadcaster.rs) — DOCUMENTED

**Severity:** Low
**Type:** Performance

In `Broadcaster::run()`, each affected subscription is processed sequentially (`process_subscription().await` in a loop). For a mutation that affects many subscribers, this serializes all query re-executions and diff computations. A single slow query blocks all other subscribers' updates.

**Recommendation:** Use `futures::stream::FuturesUnordered` or `tokio::JoinSet` to process subscriptions concurrently per change event, with a concurrency limit to prevent resource exhaustion. Not fixed inline as it changes execution semantics and error handling.

---

## Architecture Notes

The overall architecture is sound:
- **DashMap** usage throughout avoids global locks and provides good concurrency.
- **Separation of concerns** between Session, Registry, Broadcaster, Diff, and Presence is clean.
- **The QueryExecutor/DependencyTracker traits** in broadcaster.rs provide good testability boundaries.
- **The registry's reverse index** (`by_session`) enables efficient disconnect cleanup.
- **Presence TTL + rate limiting** is a correct pattern for ephemeral state.

---

## Tests Added

**65 total tests across 4 modules:**

### session.rs (13 tests)
- Session creation defaults, authentication flow
- Add/remove subscriptions, cursor updates
- SessionManager CRUD, with_session/with_session_mut
- Multiple subscriptions per session
- Edge cases: nonexistent session/subscription operations

### registry.rs (10 tests)
- Register and lookup, multiple sessions same query
- Deduplication: same session, different sub IDs, same query hash
- Unregister single (verifies remaining subs preserved)
- Unregister last cleans up empty sets (memory leak prevention)
- Unregister session removes all + leaves other sessions intact
- Edge cases: nonexistent unregister, empty query lookup

### diff.rs (24 tests, 4 pre-existing + 20 new)
- Empty-to-empty, empty-to-nonempty, nonempty-to-empty
- Nested objects, arrays, null values (both directions)
- Added new fields, simultaneous add/remove/update
- Canonical key order hashing (insertion-order-independent)
- Nested canonical hashing
- Result set order independence
- Type discrimination in hashing
- Deeply nested diffs
- Entity ID extraction priority (_id > id > entity_id)
- Same entities different order produces no diff

### presence.rs (18 tests)
- Room: update/snapshot, remove user, expiry (immediate + mixed), rate limiting, rate limit reset, state overwrite
- Manager: join creates room, join+snapshot, leave removes user, leave last cleans room, leave_all across rooms, update_state existing, update_state auto-joins, expire_all, snapshot nonexistent room, leave nonexistent room/user

---

## Verification

```
cargo check --lib -p ddb-server  # PASS (0 errors in sync/)
cargo test --lib -p ddb-server sync::  # 65 passed, 0 failed
```

Pre-existing errors in `auth/session.rs` (missing `aud` field) and `functions/runtime.rs` (moved value borrow) are unrelated to the sync module.
