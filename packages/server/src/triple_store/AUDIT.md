# Triple Store Audit Log

**Date:** 2026-04-05
**Scope:** `packages/server/src/triple_store/{mod.rs, schema.rs}`
**Test count before:** 2 | **Test count after:** 41

---

## Findings and Fixes

### ISSUE 1: No input validation on TripleInput [SEVERITY: HIGH]
**File:** `mod.rs`
**Problem:** `TripleInput` accepted empty attribute names, arbitrarily long attribute strings, and invalid `value_type` discriminators (e.g., -1, 99). These would silently persist corrupt data into Postgres.
**Fix:** Added `TripleInput::validate()` method that checks:
- Attribute is non-empty
- Attribute does not exceed 512 bytes
- `value_type` is a known `ValueType` discriminator
Called `validate()` in `PgTripleStore::set_triples` before any database writes.

### ISSUE 2: Non-deterministic MigrationGenerator::diff output [SEVERITY: MEDIUM]
**File:** `schema.rs`
**Problem:** The doc comment claimed "additions before removals" ordering, but the implementation iterated over `HashMap` keys which have random order in Rust. Consumers relying on the documented ordering would see unpredictable behavior.
**Fix:** Rewrote `diff()` to sort all key iterations alphabetically. Strict phase ordering: (1) new entity types + their attributes, (2) attribute additions/alterations on common types, (3) attribute removals, (4) removed entity types. Added `diff_deterministic_ordering` test that runs 20 iterations.

### ISSUE 3: Missing Display and TryFrom on ValueType [SEVERITY: LOW]
**File:** `schema.rs`
**Problem:** `ValueType` had `label()` but no `Display` impl, making formatted error messages awkward. No idiomatic `TryFrom<i16>` conversion.
**Fix:** Added `impl Display for ValueType` (delegates to `label()`) and `impl TryFrom<i16> for ValueType` (returns `Err(raw_value)` for unknown discriminators). Added `max_discriminator()` helper.

### ISSUE 4: Missing PartialEq on all schema data types [SEVERITY: LOW]
**File:** `schema.rs`
**Problem:** `AttributeInfo`, `ReferenceInfo`, `EntityType`, `Schema`, and `MigrationAction` all lacked `PartialEq`, making equality assertions in tests impossible and preventing downstream comparison logic.
**Fix:** Added `PartialEq` derive to all five types.

### ISSUE 5: EntityTypeBuilder silently drops unknown value types [SEVERITY: LOW]
**File:** `mod.rs` (line ~458)
**Problem:** `filter_map(|vt| ValueType::from_i16(*vt))` silently discards unknown discriminators during schema inference. If corrupt data exists, the schema would show an attribute with an empty `value_types` vec and no warning.
**Status:** Not fixed (by design). The builder operates on live data that may contain legacy discriminators. Silent filtering is acceptable here. Added test `builder_unknown_value_type_filtered` to document this behavior.

### ISSUE 6: get_entity_at uses DISTINCT ON for point-in-time reads [SEVERITY: INFORMATIONAL]
**File:** `mod.rs` (line ~364)
**Problem:** `DISTINCT ON (attribute)` returns only one triple per attribute at a given tx. This is correct for cardinality-one attributes but loses data for cardinality-many attributes (e.g., multiple tags on an entity).
**Status:** Documented, not fixed. This is an architectural decision -- cardinality-many support would require changing the query strategy. The current design treats all attributes as cardinality-one at the storage layer.

### ISSUE 7: Pre-existing compilation errors in main.rs [SEVERITY: N/A]
**File:** `main.rs` (lines 93, 95, 151)
**Problem:** `KeyManager::generate()` and `KeyManager::from_secret()` no longer exist; auth middleware type mismatch. These are pre-existing and unrelated to the triple store.
**Status:** Out of scope. Library target (`--lib`) compiles cleanly.

---

## Test Coverage Summary

### mod.rs tests (17 tests)
- Triple JSON serialization roundtrip
- Triple clone independence
- TripleInput validation: valid input, empty attribute, overlong attribute, invalid value_type, negative value_type, all valid types, boundary length
- EntityTypeBuilder: single entity, required detection, polymorphic types, unknown type filtering, reference detection, empty builder, deduplication, cardinality counting

### schema.rs tests (24 tests)
- ValueType: roundtrip all variants, invalid values, TryFrom errors, Display/label match, lowercase labels, contiguous discriminators, serde roundtrip
- AttributeInfo equality
- Schema: default is empty, equality, inequality on tx
- ReferenceInfo equality
- EntityType with no attributes
- MigrationGenerator::diff: empty-to-empty, empty-to-new, remove type, add attribute, remove attribute, alter type, type widening, no-change, multi-type add+remove, deterministic ordering, complex scenario

---

## SQL Injection Risk Assessment
**Result: SAFE.** All SQL queries use parameterized bindings (`$1`, `$2`, etc.) via `sqlx::query` and `sqlx::query_as`. No string interpolation into SQL. The `sqlx` crate enforces compile-time query safety where possible.

## Panic Risk Assessment
**Result: LOW.** No `unwrap()` on user-controlled data paths. The only `unwrap_or` is on `MAX(tx_id)` which returns `None` for empty tables (correctly defaults to 0). `EntityTypeBuilder::build()` uses safe `HashMap` access patterns. The `as u64` casts on `len()` are safe since `usize` always fits in `u64` on 64-bit targets.
