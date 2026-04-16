# DarshJDB v0.3.1.1 — Hotfix summary

**Commit range**: `32e9b18..65eb1b1` (9 commits, fast-forwarded onto `main`)
**Tag**: `v0.3.1.1`
**Release**: https://github.com/darshjme/darshjdb/releases/tag/v0.3.1.1
**Branch**: `hotfix/v0.3.1.1` (merged fast-forward into `main`)

## Commits (oldest first)

| # | SHA | Subject |
|---|-----|---------|
| 1 | `de11dd9` | `security(config): wrap DatabaseConfig.url in Secret<String>` — F1 + IN-03 stale URL docstring (bundled because both touched the same lines) |
| 2 | `2355850` | `fix(main): honor cfg.server.bind_addr instead of shadowing with DDB_BIND_ADDR` — WR-02 |
| 3 | `9a81f61` | `fix(main): log actual pool max_lifetime_sec from typed config` — WR-01 |
| 4 | `7a21420` | `fix(config,main): correct misleading 'single-threaded startup' safety comments` — WR-03 |
| 5 | `c82f575` | `chore(clippy): resolve 4 pre-existing warnings in config/mod.rs` — M-4 (derivable_impls + 3x collapsible_if) |
| 6 | `2f4ade2` | `docs(changelog,storage): correct Store trait method names and error variant` — IN-01 + IN-02 |
| 7 | `05bbec1` | `docs(changelog): correct DARSH_CACHE_PASSWORD and /cluster/status claims` — FALSE #3 + OVERSTATED #1 |
| 8 | `f4383ef` | `fix(compose): require POSTGRES_PASSWORD instead of weak darshan fallback` — F9 |
| 9 | `65eb1b1` | `chore(store): symmetrize StoreTx commit/rollback across Pg and Sqlite` — IN-04 |

Originally scoped as 11 commits; commits 7+8 of the plan were pre-planned to combine, and commit 5 (IN-03 stale URL) was folded into commit 1 because both edits touched the same `DatabaseConfig::url` docstring line — net 9 commits, all 11 concerns resolved.

## Verification output

```
$ cargo check --workspace
    Finished `dev` profile [unoptimized + debuginfo] target(s)  (clean)

$ cargo clippy -p ddb-server --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)  (clean)

$ cargo clippy -p ddb-server --all-targets --features sqlite-store -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)  (clean)

$ cargo test -p ddb-server --lib -- config::
running 9 tests
test config::tests::secret_expose_returns_inner_value ... ok
test config::tests::debug_impl_redacts_secrets ... ok
test config::tests::database_url_does_not_leak_password_via_debug ... ok  # NEW — guards F1
test config::tests::env_var_overrides_tls_paths_via_legacy_shim ... ok
test config::tests::darsh_prefix_maps_to_anchor ... ok
test config::tests::defaults_produce_a_valid_config ... ok
test config::tests::legacy_flat_env_var_still_works ... ok
test config::tests::new_env_var_overrides_server_port ... ok
test config::tests::new_prefix_wins_over_legacy_flat ... ok
test result: ok. 9 passed; 0 failed
```

## Findings deferred

None. Every finding from the hotfix plan landed.

## Ship actions

- [x] Fast-forwarded `main` to `hotfix/v0.3.1.1` (`32e9b18..65eb1b1`)
- [x] Pushed `main` to `origin`
- [x] Created annotated tag `v0.3.1.1` and pushed
- [x] Created GitHub release https://github.com/darshjme/darshjdb/releases/tag/v0.3.1.1
