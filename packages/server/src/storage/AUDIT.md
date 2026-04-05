# Storage Engine Security Audit

**Date:** 2026-04-05
**Auditor:** PhD Storage Engineer (Claude Opus 4.6)
**File:** `packages/server/src/storage/mod.rs`
**Status:** Issues found and remediated

---

## Executive Summary

The DarshJDB storage engine provides a well-structured pluggable backend architecture with local filesystem, S3, R2, and MinIO support. The audit identified **9 security and correctness issues**, of which **6 were fixed** in this pass. The remaining 3 are documented as recommendations for future work.

---

## Issues Found

### CRITICAL

#### 1. Null Byte Injection in Paths
- **Severity:** CRITICAL
- **Status:** FIXED
- **Location:** `LocalFsBackend::resolve_path()`
- **Description:** The path resolver did not reject null bytes (`\0`). On certain operating systems and filesystem layers, null bytes cause string truncation, allowing an attacker to craft a path like `file.txt\0.jpg` that resolves to `file.txt`, bypassing extension-based checks.
- **Fix:** Added explicit null byte check at the top of `resolve_path()`. Also added empty path rejection.
- **Test:** `path_traversal_null_byte`, `path_traversal_empty_path`

### HIGH

#### 2. No Content-Type Validation
- **Severity:** HIGH
- **Status:** FIXED
- **Location:** `StorageEngine::upload()`
- **Description:** The upload method accepted any content-type string, including empty strings, malformed values (missing `/` separator), and dangerous executable types (`application/x-executable`, `application/x-msdownload`, etc.). This enables stored XSS via content-type spoofing and uploading of executable payloads.
- **Fix:** Added `validate_content_type()` method that enforces: (a) non-empty, (b) valid `type/subtype` format, (c) blocklist of dangerous executable MIME types. Applied before hooks run.
- **Test:** `upload_rejects_empty_content_type`, `upload_rejects_invalid_content_type`, `upload_rejects_blocked_content_types`, `upload_accepts_valid_content_types`

#### 3. No Upload Size Limits
- **Severity:** HIGH
- **Status:** FIXED
- **Location:** `StorageEngine::upload()`
- **Description:** The upload method accepted arbitrarily large payloads with no size enforcement. A malicious client could exhaust server memory or disk by uploading multi-gigabyte files.
- **Fix:** Added `max_upload_size` field (default 100 MiB) with `set_max_upload_size()` setter. Size check runs before any hook or backend I/O.
- **Test:** `upload_rejects_oversized_payload`, `upload_accepts_within_size_limit`

#### 4. S3 Backend Path Traversal
- **Severity:** HIGH
- **Status:** FIXED
- **Location:** `S3Backend::effective_key()`
- **Description:** The `effective_key()` method did no path validation -- it simply prepended an optional prefix to the raw user-supplied path. An attacker could supply `../../../sensitive-bucket-key` to access or overwrite objects outside the intended prefix.
- **Fix:** Added null byte, empty path, absolute path, and `..` traversal checks to `effective_key()`. Method now returns `Result<String, StorageError>` instead of bare `String`.
- **Test:** `s3_effective_key_rejects_traversal`

### MEDIUM

#### 5. Timing Attack on Signed URL Verification
- **Severity:** MEDIUM
- **Status:** FIXED
- **Location:** `StorageEngine::verify_signed_url()`
- **Description:** The signature comparison used `!=` (standard string equality), which is vulnerable to timing side-channel attacks. An attacker could progressively guess the correct signature by measuring response times.
- **Fix:** Replaced string comparison with constant-time comparison using `hmac::digest::CtOutput`. The signature is now decoded from base64 and compared byte-by-byte in constant time.
- **Test:** `signed_url_roundtrip`, `signed_url_tampered_signature_fails`

#### 6. Unbounded Image Transform Dimensions
- **Severity:** MEDIUM
- **Status:** FIXED
- **Location:** `ImageTransform::from_query()`
- **Description:** Width and height accepted any `u32` value (up to 4,294,967,295 pixels). Quality accepted any `u8` value including 0. An attacker could request `w=4294967295,h=4294967295` to cause DoS in downstream image processors.
- **Fix:** Dimensions clamped to `MAX_IMAGE_DIMENSION` (16384) and zero-values rejected. Quality clamped to 1-100 range.
- **Test:** `image_transform_dimension_clamped`, `image_transform_zero_dimension_rejected`, `image_transform_quality_clamped`

### LOW (Not Fixed -- Recommendations)

#### 7. `head_object` Reads Full File for ETag
- **Severity:** LOW (performance)
- **Location:** `LocalFsBackend::head_object()`
- **Description:** The `head_object` implementation reads the entire file content into memory just to compute the SHA-256 ETag. For a 10 GB file, this allocates 10 GB of RAM for a metadata-only operation.
- **Recommendation:** Store the ETag in the sidecar `.meta.json` file during `put_object` and read it from there in `head_object`. Fall back to computing from content only if the sidecar is missing.

#### 8. Resumable Uploads Never Expire
- **Severity:** LOW (resource leak)
- **Location:** `StorageEngine::resumable_uploads` (DashMap)
- **Description:** The `resumable_uploads` DashMap grows unboundedly. Abandoned uploads are never cleaned up, causing a slow memory leak proportional to the number of initiated-but-never-completed uploads.
- **Recommendation:** Add an `expires_at` field to `ResumableUpload` and run a periodic cleanup task (e.g., every 60 seconds) that removes uploads older than a configurable TTL (e.g., 24 hours).

#### 9. No SSRF Mitigation for Signed URLs
- **Severity:** LOW (informational)
- **Location:** `StorageEngine::signed_url()`
- **Description:** The `base_url` parameter is passed directly from the caller with no validation. If an attacker controls the `base_url` (e.g., via a host header injection), they could generate signed URLs pointing to internal services.
- **Recommendation:** Validate `base_url` against an allowlist of known public origins during engine initialization, or hardcode the base URL in config.

---

## Test Coverage Added

| Category | Tests | Count |
|---|---|---|
| Path traversal prevention | parent_dir, nested_parent, absolute_path, null_byte, empty_path, safe_paths | 6 |
| S3 path validation | s3_effective_key_rejects_traversal | 1 |
| Signed URL security | roundtrip, wrong_path, expired, tampered_signature | 4 |
| Image transform parsing | full_parsing, empty_query, quality_clamped, dimension_clamped, zero_dimension, all_fits, all_formats, unknown_keys | 8 |
| Local FS CRUD | put_get_delete, head_object, list_objects, get_nonexistent, delete_nonexistent, metadata_preserved | 6 |
| Content-type validation | empty, invalid, blocked, valid | 4 |
| Upload size limits | oversized_rejected, within_limit | 2 |
| Resumable uploads | create_status, sequential_chunks, wrong_offset, nonexistent_id, cancel, open_ended | 6 |
| Hook integration | upload_hook_can_reject | 1 |
| **Total** | | **38** |

All 38 tests pass. `cargo check --lib` passes clean.

---

## Files Modified

| File | Changes |
|---|---|
| `packages/server/src/storage/mod.rs` | Security fixes + 38 tests (see above) |
| `packages/server/src/auth/session.rs` | Fixed pre-existing `include_bytes!` path (unrelated) |

---

## Pre-existing Issues (Out of Scope)

- `packages/server/src/main.rs` has 5 compilation errors related to `KeyManager` API changes -- these predate this audit.
- `packages/server/src/auth/session.rs` had broken `include_bytes!` paths for test fixtures -- fixed as a prerequisite to compile tests.
