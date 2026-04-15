# DarshJDB v0.3.2.1 — mlua `ddb.kv.*` wiring

Last deferred item from the v0.3.2 mlua sprint. Branch
`feat/v0.3.2.1-mlua-kv`, rooted at tag `v0.3.2`.

## Background

v0.3.2 shipped the `MluaRuntime` with a hardened sandbox and three
wired host APIs (`ddb.query`, `ddb.triples.get`, `ddb.triples.put`),
plus `ddb.log.*` which is always live. The `ddb.kv.{get,set}` paths
were left stubbed as `NotYetImplemented` because the `DdbCache`
boundary had not been threaded into `MluaContext`. v0.3.2.1 closes
that gap.

## v0.3.2.1 Changes

### `MluaContext` (commit 1)

`packages/server/src/functions/mlua.rs`

Added a third field, `cache: Arc<DdbCache>`, alongside the existing
`store` and `dialect` handles. `Debug` impl reports the cache as an
opaque `"Arc<DdbCache>"` placeholder. Marked the new field with a
`// v0.3.2.1-mlua-kv` comment so the post-merge reconciliation with
the sibling executor-rewire branch is obvious.

`packages/server/src/main.rs`

Single-line update at the `MluaContext { ... }` literal to populate
the new field, plus a `TODO(v0.3.2.1-merge):` comment pointing at
the post-merge wiring task: today the runtime gets a fresh
`DdbCache::new()` instance, but production should share the
AppState-scoped `Arc<DdbCache>` so Lua writes are visible to the
REST `/api/cache/*` router and the RESP3 dispatcher tenants of the
same cache.

### `ddb.kv.{get,set,del}` wiring (commit 2)

`packages/server/src/functions/mlua.rs` — `install_ddb_api`

Replaced the three stubs with closures capturing `Arc<DdbCache>`:

- **`ddb.kv.get(key)` -> `string | nil`** — calls `DdbCache::get(&key)`.
  Returns a Lua string for UTF-8 cache values, `nil` for miss (key
  absent or expired), and a Lua `RuntimeError` for present-but-non-
  UTF-8 bytes. Binary blobs belong in object storage; the user-facing
  convention for `ddb.kv.*` is text-typed (matching the RESP3 GET
  command shape).
- **`ddb.kv.set(key, value [, ttl_seconds])` -> `nil`** — calls
  `DdbCache::set(key, value.into_bytes(), ttl)`. The optional
  trailing `ttl_seconds` argument is mapped to
  `Some(Duration::from_secs(secs))`; `0` is treated as "no expiry"
  so user code that computes a TTL dynamically can pass `0` without
  special-casing.
- **`ddb.kv.del(key)` -> `bool`** — calls `DdbCache::del(&key)`.
  Returns `true` if the key existed before deletion across ANY of
  the typed cache tiers (string/hash/list/zset/stream).

The closures are sync because `DdbCache::{get,set,del}` are sync
(DashMap-backed in-process). They use `lua.create_function` rather
than `lua.create_async_function`, which still works correctly under
the runtime's `call_async` execute path.

When the runtime is constructed without an `MluaContext` (test
default), all three calls keep raising `NotYetImplemented` so
hermetic unit tests don't need a cache.

### Tests (commits 2 + 3)

5 new integration tests under `#[cfg(all(test, feature = "mlua-runtime"))]`,
gated on `sqlite-store` so they can reuse the
`new_runtime_with_sqlite_context()` helper:

1. **`ddb_kv_get_returns_nil_for_missing_key`** — bare `ddb.kv.get`
   on an unset key returns Lua `nil`.
2. **`ddb_kv_set_then_get_roundtrip`** — `ddb.kv.set("hello", "world")`
   followed by `ddb.kv.get("hello")` returns `"world"`.
3. **`ddb_kv_set_with_ttl_expires`** (`#[tokio::test(flavor = "multi_thread")]`)
   — sets a key with `ttl_seconds = 1`, confirms immediate read,
   sleeps 1.2s, asserts the next read is `nil`.
4. **`ddb_kv_del_removes_value`** — `set` -> `del` (asserts `true`)
   -> `get` (asserts `nil`).
5. **`ddb_kv_get_non_utf8_errors`** — constructs a custom runtime so
   the test owns the `Arc<DdbCache>` directly, seeds `binkey` with
   `vec![0xff, 0xfe, 0xfd]` bypassing the Lua text-only `set`, then
   `pcall`s `ddb.kv.get("binkey")` from Lua and asserts the error
   message contains `ddb.kv.get` and `non-utf8`.

Adjusted the existing `ddb_stubs_all_raise_lua_error` test
(commit 3): removed `call_kv_get` and `call_kv_set` from the stub-
fallback assertion list since those paths now have live wiring.
The test still covers `query`, `triples.get`, `triples.put`.

Removed the obsolete `ddb_kv_stays_stubbed_with_context` test
(commit 2): its assertion ("with context, ddb.kv.get must surface
the v0.3.2.1 deferral message") is the exact post-condition that
v0.3.2.1 inverts.

## Verification

```
cargo check -p ddb-server                                          # green
cargo check -p ddb-server --features mlua-runtime                  # green
cargo check -p ddb-server --features "mlua-runtime sqlite-store"   # green
cargo test  -p ddb-server --features "mlua-runtime sqlite-store" \
    --lib functions::mlua::                                        # 30 passed, 1 ignored
cargo clippy -p ddb-server --features mlua-runtime --lib -- -D warnings
```

Test count: v0.3.2 shipped 23 mlua tests; v0.3.2.1 brings the total
to 30 passing (+5 new ddb.kv.* tests, +2 sqlite-gated triples/query
tests already shipped, -1 obsolete stub-error test, with the original
23 still green). One pre-existing test stays `#[ignore]`d pending the
v0.3.3 `mlua::Lua::set_interrupt` work for CPU-bound interruption.

## Cross-agent collision surface

The sibling agent works on `packages/server/src/query/**`,
`packages/server/src/store/**`, and `darshql/executor.rs`. This
branch only touches:

- `packages/server/src/functions/mlua.rs` — additive (new field,
  new closures, new tests, removed obsolete test).
- `packages/server/src/main.rs` — single-line struct-literal update
  on the `MluaContext { ... }` site at line 815, with a TODO
  comment for the orchestrator to thread the AppState-shared cache
  in post-merge.

The new `cache` field on `MluaContext` is appended at the end of
the struct and tagged `// v0.3.2.1-mlua-kv` so any merge conflict
on the struct definition is trivially resolvable.
