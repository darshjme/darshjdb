# Query Engine Audit

**Date:** 2026-04-05
**Scope:** `packages/server/src/query/mod.rs` and `packages/server/src/query/reactive.rs`
**Auditor:** Query optimisation review (Claude Opus 4.6)

---

## Issues Found and Fixed

### 1. LIKE Wildcard Injection in `$search` (CRITICAL)

**File:** `mod.rs`, line ~255 (plan_query, search clause)
**Severity:** Critical
**Category:** SQL injection / query manipulation

**Problem:** The `$search` term was interpolated directly into a `%{term}%` ILIKE pattern without escaping the LIKE metacharacters `%` and `_`. A user searching for `%` would match every row; searching for `_` would match any single-character value. This enabled data exfiltration through controlled wildcard expansion.

**Fix:** Added proper escaping of `\`, `%`, and `_` in the search term before wrapping in outer `%...%` wildcards:
```rust
let escaped = term.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
params.push(serde_json::Value::String(format!("%{escaped}%")));
```

**Test:** `plan_search_escapes_wildcards` validates metacharacters are escaped.

---

### 2. `LIMIT` / `OFFSET` Inlined as Literals (MEDIUM)

**File:** `mod.rs`, lines 289-294 (plan_query, pagination)
**Severity:** Medium
**Category:** Plan cache correctness

**Problem:** `LIMIT` and `OFFSET` values were inlined as format string literals (`LIMIT 10`) rather than parameterised (`LIMIT $N`). Since the plan cache `shape_key` hashes whether limit/offset are present (Some vs None) but not their actual values, a cached SQL for `LIMIT 10` would be reused for a query requesting `LIMIT 50`. The first query's literal would win, returning wrong result counts.

**Fix:** Parameterised both `LIMIT` and `OFFSET` as bind parameters (`LIMIT $N`, `OFFSET $N`) with values pushed to the params vector. This ensures cached SQL shapes are universally correct regardless of concrete pagination values.

**Tests:** `plan_limit_offset_parameterised`, `shape_key_ignores_limit_offset_values`, `shape_key_differs_with_without_limit`.

---

### 3. Reactive `$search` Wildcard Dependency Never Matched (MEDIUM)

**File:** `reactive.rs`, `extract_dependencies` and `get_affected_queries`
**Severity:** Medium
**Category:** Dependency tracking accuracy

**Problem:** Search queries registered a wildcard dependency with `attribute: "*"`, but `get_affected_queries` only checked exact attribute and wildcard (attribute, None) matches. Since no real triple change has attribute `"*"`, search queries were never invalidated by data mutations, causing stale live query results.

**Fix:** Added explicit wildcard `"*"` lookup in `get_affected_queries` that matches the star dependency against every incoming change (scoped by entity type).

**Tests:** `search_query_affected_by_any_attribute_change`, `search_query_not_affected_by_wrong_entity_type`.

---

### 4. `$semantic` Operator Silently Ignored (LOW)

**File:** `mod.rs`, plan_query
**Severity:** Low
**Category:** Missing operator handling

**Problem:** The `$semantic` field was parsed into the AST but completely ignored during plan generation. No SQL was emitted and no warning was logged, making it invisible to callers that their semantic filter had no effect.

**Fix:** Added a `tracing::warn!` log when `$semantic` is present, documenting that the operator is acknowledged but not yet wired to an embedding backend. Also included `$semantic` presence in the plan cache `shape_key` hash to prevent future cache collisions when the operator is implemented.

**Test:** `shape_key_differs_with_without_semantic`.

---

## Test Coverage Added

### mod.rs Tests (39 total, ~25 new)

**Parsing -- every DarshanQL operator:**
- `parse_minimal_query` -- type-only query
- `parse_where_all_operators` -- Eq, Neq, Gt, Gte, Lt, Lte, Contains, Like
- `parse_order_asc_desc` -- multi-column ordering
- `parse_limit_offset` -- pagination fields
- `parse_search` -- full-text search
- `parse_semantic` -- vector search
- `parse_nested` -- multiple references
- `parse_full_query` -- all operators combined

**Nested query parsing:**
- `parse_nested_with_sub_query` -- 3-level nesting (Order -> Customer -> Address)
- `parse_multiple_nested_forward_and_backward` -- forward and backward refs
- `deeply_nested_query` -- 5 levels of nesting

**Edge cases:**
- `reject_non_object_query`, `reject_array_query` -- wrong root types
- `reject_missing_type`, `reject_null_type`, `reject_numeric_type` -- missing/bad type
- `reject_invalid_where_shape` -- $where as string
- `reject_unknown_operator_in_where` -- unknown op "Regex"
- `empty_where_is_ok`, `empty_order_is_ok`, `empty_nested_is_ok` -- empty arrays

**Plan generation:**
- `plan_basic_generates_valid_sql` -- baseline SQL structure
- `plan_with_where_creates_joins` -- join aliases tw0, tw1
- `plan_all_operators_produce_correct_sql_op` -- correct SQL operators for each WhereOp
- `plan_limit_offset_parameterised` -- LIMIT/OFFSET as bind params
- `plan_search_escapes_wildcards` -- wildcard injection prevention
- `plan_nested_creates_plans` -- nested plan generation
- `plan_order_by_generates_subqueries` -- ORDER BY subquery structure

**Plan cache:**
- `plan_cache_hit` -- basic hit
- `plan_cache_miss_on_different_entity_type` -- type discriminates
- `plan_cache_miss_on_different_operator` -- operator discriminates
- `plan_cache_miss_on_different_attribute` -- attribute discriminates
- `shape_key_ignores_values` -- same shape, different values
- `shape_key_ignores_limit_offset_values` -- concrete limit/offset values irrelevant
- `shape_key_differs_with_without_limit` -- presence vs absence
- `shape_key_differs_with_without_search` -- search presence
- `shape_key_differs_with_without_semantic` -- semantic presence
- `plan_cache_lru_eviction` -- LRU eviction ordering
- `plan_cache_zero_capacity_uses_default` -- fallback to 256

### reactive.rs Tests (27 total, ~13 new)

**Registration:**
- `register_and_deregister` -- lifecycle
- `register_assigns_unique_ids` -- monotonic IDs
- `deregister_nonexistent_is_noop` -- no panic
- `deregister_preserves_other_queries_deps` -- clean removal

**Dependency matching:**
- `exact_match_triggers_affected` -- Eq value match
- `different_value_no_match` -- Eq value mismatch
- `order_by_creates_wildcard_dep` -- any value triggers
- `range_operator_creates_wildcard_dep` -- Gt/Gte/Lt/Lte/Neq/Contains/Like all wildcard
- `unknown_entity_type_conservatively_matches` -- None entity type = match

**Entity type filtering:**
- `wrong_entity_type_no_match` -- Post change vs User query
- `db_type_change_triggers_matching_queries` -- :db/type insertion
- `db_type_change_different_type_no_match` -- wrong type

**Search wildcard:**
- `search_query_affected_by_any_attribute_change` -- star dependency works
- `search_query_not_affected_by_wrong_entity_type` -- scoped by type

**Nested and batch:**
- `nested_ref_change_triggers_query` -- reference attribute change
- `batch_changes_match_multiple_queries` -- multiple changes, multiple queries
- `empty_changes_returns_empty` -- no changes = no affected

**Dependency extraction:**
- `extract_dependencies_includes_db_type` -- always present
- `extract_dependencies_eq_produces_exact_constraint` -- Eq = precise
- `extract_dependencies_gt_produces_wildcard` -- range = wildcard
- `extract_dependencies_order_produces_wildcard` -- order = wildcard
- `extract_dependencies_search_produces_star` -- search = star
- `extract_dependencies_nested_produces_wildcard` -- ref = wildcard

---

## Pre-existing Issues Not Fixed (Out of Scope)

1. **`auth/session.rs` include_bytes path:** `include_bytes!("../../../tests/fixtures/...")` resolves to `packages/tests/fixtures/` instead of `packages/server/tests/fixtures/`. The PEM files exist at the correct location but the relative path is wrong. Created symlink copies to unblock test compilation.

2. **`main.rs` compilation errors:** `KeyManager::generate()` and `KeyManager::from_secret()` methods referenced in main.rs do not exist on the current `KeyManager` struct. Pre-existing, unrelated to query engine.

3. **Plan cache re-planning on hit:** On cache hit, `run_query` calls `plan_query` to get fresh params then overwrites SQL with cached SQL. This works correctly (shape equivalence guarantees matching param slots) but the fresh plan's SQL construction is wasted work. A future optimisation could store only the SQL string in cache and always compute params from the AST directly.

4. ~~**N+1 nested query execution:**~~ **RESOLVED.** `execute_query` now calls `batch_resolve_nested` which collects all referenced UUIDs per nested plan into a `HashSet`, batch-fetches them in a single `WHERE entity_id = ANY($1::uuid[])` query, groups results by entity_id, and recursively resolves sub-nested references up to `MAX_NESTING_DEPTH` (3). This turns N+1 into 1+P where P = number of nested plans. Tests: `nested_plan_sql_uses_any_batch`, `nested_plan_builds_sub_nested`, `nested_plan_respects_max_depth`.

---

## Verification

```
cargo check --package darshandb-server --lib   # PASS
cargo test --package darshandb-server --lib query   # 66 passed, 0 failed
```
