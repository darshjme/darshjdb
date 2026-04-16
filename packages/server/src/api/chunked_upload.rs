//! Chunked / resumable file upload — VYASA Phase 7.1.
//!
//! Created by Darshankumar Joshi (github.com/darshjme).
//!
//! This module implements a three-phase resumable upload protocol on top
//! of the existing [`crate::storage::StorageEngine`]:
//!
//! 1. **Init** (`POST /api/storage/upload/init`)
//!    Client declares `path`, `content_type`, `total_chunks`, and optional
//!    `file_size` / `entity_id`. Server validates the path, inserts a row
//!    into `chunked_uploads`, and returns an `upload_id` plus a URL
//!    template the client can fill with chunk indices.
//!
//! 2. **Chunk** (`PUT /api/storage/upload/:upload_id/chunk/:index`)
//!    Client streams raw chunk bytes. The server writes each chunk to
//!    `/tmp/darshjdb-uploads/{upload_id}/{index}.part` using an atomic
//!    `write-then-rename` dance, then records the index in
//!    `received_chunks` using `array_append` guarded by `NOT ANY($1)` so
//!    duplicate retries are idempotent. When the last chunk lands, the
//!    server assembles the file in index order, streams it through
//!    [`StorageEngine::upload`], wipes the tmp directory, and marks the
//!    row `completed`.
//!
//! 3. **Status** (`GET /api/storage/upload/:upload_id/status`)
//!    Returns `{status, received_chunks_count, total_chunks, pct}` so
//!    clients can poll for resume decisions.
//!
//! A background cleanup task sweeps abandoned uploads (>24h old,
//! `status='in_progress'`) every 5 minutes and purges their tmp dirs.
//!
//! # Security
//!
//! All client-supplied paths run through [`sanitize_storage_path`] which
//! rejects `..` segments, absolute paths, NUL bytes, empty strings, and
//! paths >= 1024 bytes. The same sanitizer is reused to plug the Phase 0
//! path-traversal hole in the legacy single-shot upload handler.

use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use super::error::ApiError;
use super::rest::{AppState, negotiate_response_pub};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum accepted path length in bytes. Matches common filesystem limits
/// and keeps the Postgres `TEXT` column bounded in practice.
pub const MAX_STORAGE_PATH_LEN: usize = 1024;

/// Hard cap on `total_chunks` — 1 GiB at 1 MiB chunks is 1024, so 4096
/// gives plenty of headroom while preventing integer-overflow silliness.
pub const MAX_TOTAL_CHUNKS: i32 = 4096;

/// Maximum size of a single chunk the server will accept (64 MiB).
pub const MAX_CHUNK_BYTES: usize = 64 * 1024 * 1024;

/// Root tmp directory where in-flight chunks are staged on disk.
pub const TMP_UPLOAD_ROOT: &str = "/tmp/darshjdb-uploads";

/// Age threshold for `cleanup_stale_uploads` (24h).
const STALE_UPLOAD_AFTER: chrono::Duration = chrono::Duration::hours(24);

/// Interval at which the background cleanup task runs (5 minutes).
pub const CLEANUP_INTERVAL: Duration = Duration::from_secs(5 * 60);

// ---------------------------------------------------------------------------
// Schema bootstrap
// ---------------------------------------------------------------------------

/// Create the `chunked_uploads` table if it does not already exist.
/// Mirrors `migrations/20260414002423_chunked_uploads.sql` so deployments
/// that don't run the SQL migrator still get a working table on startup.
pub async fn ensure_chunked_uploads_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS chunked_uploads (
            upload_id        UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            entity_id        UUID,
            path             TEXT        NOT NULL,
            total_chunks     INTEGER     NOT NULL,
            received_chunks  INTEGER[]   NOT NULL DEFAULT '{}',
            content_type     TEXT        NOT NULL,
            file_size        BIGINT,
            status           TEXT        NOT NULL DEFAULT 'in_progress',
            created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
            completed_at     TIMESTAMPTZ,
            CONSTRAINT chunked_uploads_total_chunks_positive CHECK (total_chunks > 0)
        );
        CREATE INDEX IF NOT EXISTS idx_chunked_uploads_status
            ON chunked_uploads (status, created_at);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Path sanitization — shared with the legacy single-shot upload handler
// ---------------------------------------------------------------------------

/// Reject any client-supplied storage path that could escape the backend's
/// root, embed NUL bytes, or blow past filesystem limits.
///
/// This is the **single source of truth** for storage path validation —
/// both the chunked upload handlers and the legacy multipart handler in
/// `api::rest::storage_upload` (Phase 0 vuln fix) call into it.
///
/// The returned `String` is the normalized path that should be persisted.
/// At the moment we do not rewrite slashes, but callers should always
/// use the return value rather than the original input so future
/// normalization lands everywhere.
pub fn sanitize_storage_path(raw: &str) -> Result<String, ApiError> {
    if raw.is_empty() {
        return Err(ApiError::bad_request("storage path must not be empty"));
    }
    if raw.len() >= MAX_STORAGE_PATH_LEN {
        return Err(ApiError::bad_request(format!(
            "storage path exceeds maximum length of {MAX_STORAGE_PATH_LEN} bytes"
        )));
    }
    if raw.contains('\0') {
        return Err(ApiError::bad_request(
            "storage path must not contain NUL bytes",
        ));
    }
    // Reject absolute paths — both Unix `/foo` and Windows `C:\foo`.
    if raw.starts_with('/') || raw.starts_with('\\') {
        return Err(ApiError::bad_request("storage path must not be absolute"));
    }
    if raw.len() >= 2 && raw.as_bytes()[1] == b':' {
        return Err(ApiError::bad_request(
            "storage path must not be a drive-qualified absolute path",
        ));
    }
    // Reject any `..` segment. We split on both `/` and `\` so a mixed-
    // separator payload like `foo\..\bar` cannot sneak through.
    for segment in raw.split(['/', '\\']) {
        if segment == ".." {
            return Err(ApiError::bad_request(
                "path traversal (`..`) is not allowed in storage paths",
            ));
        }
    }
    // Control characters are not a legitimate part of any filename the
    // server will hand out. Reject them up front so nothing downstream
    // has to worry about them.
    if raw.chars().any(|c| c.is_control()) {
        return Err(ApiError::bad_request(
            "storage path must not contain control characters",
        ));
    }
    Ok(raw.to_string())
}

// ---------------------------------------------------------------------------
// Request / response DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct InitUploadRequest {
    pub path: String,
    pub content_type: String,
    pub total_chunks: i32,
    #[serde(default)]
    pub file_size: Option<i64>,
    #[serde(default)]
    pub entity_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct InitUploadResponse {
    pub upload_id: Uuid,
    pub chunk_upload_url: String,
    pub status_url: String,
    pub total_chunks: i32,
}

#[derive(Debug, Serialize)]
pub struct UploadStatusResponse {
    pub upload_id: Uuid,
    pub status: String,
    pub path: String,
    pub received_chunks_count: usize,
    pub total_chunks: i32,
    pub pct: u8,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Handler: POST /api/storage/upload/init
// ---------------------------------------------------------------------------

pub async fn init_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(req): axum::Json<InitUploadRequest>,
) -> Result<Response, ApiError> {
    let _token = crate::api::rest::extract_bearer_token_pub(&headers)?;

    // --- validate -----------------------------------------------------
    let path = sanitize_storage_path(&req.path)?;
    if req.content_type.trim().is_empty() {
        return Err(ApiError::bad_request("content_type must not be empty"));
    }
    if req.total_chunks <= 0 {
        return Err(ApiError::bad_request(
            "total_chunks must be a positive integer",
        ));
    }
    if req.total_chunks > MAX_TOTAL_CHUNKS {
        return Err(ApiError::bad_request(format!(
            "total_chunks {} exceeds maximum of {MAX_TOTAL_CHUNKS}",
            req.total_chunks
        )));
    }
    if let Some(size) = req.file_size
        && size < 0
    {
        return Err(ApiError::bad_request("file_size must not be negative"));
    }

    // --- insert -------------------------------------------------------
    let upload_id: Uuid = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO chunked_uploads
            (entity_id, path, total_chunks, content_type, file_size, status)
        VALUES ($1, $2, $3, $4, $5, 'in_progress')
        RETURNING upload_id
        "#,
    )
    .bind(req.entity_id)
    .bind(&path)
    .bind(req.total_chunks)
    .bind(&req.content_type)
    .bind(req.file_size)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("failed to create chunked upload: {e}")))?;

    let body = InitUploadResponse {
        upload_id,
        chunk_upload_url: format!("/api/storage/upload/{upload_id}/chunk/{{index}}"),
        status_url: format!("/api/storage/upload/{upload_id}/status"),
        total_chunks: req.total_chunks,
    };
    Ok((StatusCode::CREATED, axum::Json(body)).into_response())
}

// ---------------------------------------------------------------------------
// Handler: PUT /api/storage/upload/:upload_id/chunk/:index
// ---------------------------------------------------------------------------

pub async fn put_chunk(
    State(state): State<AppState>,
    Path((upload_id, index)): Path<(Uuid, i32)>,
    headers: HeaderMap,
    request: axum::http::Request<Body>,
) -> Result<Response, ApiError> {
    let _token = crate::api::rest::extract_bearer_token_pub(&headers)?;

    if index < 0 {
        return Err(ApiError::bad_request("chunk index must not be negative"));
    }

    // --- look up upload row ------------------------------------------
    let row: Option<(String, i32, String, String, Vec<i32>)> = sqlx::query_as(
        r#"
        SELECT path, total_chunks, content_type, status, received_chunks
          FROM chunked_uploads
         WHERE upload_id = $1
        "#,
    )
    .bind(upload_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("failed to load upload row: {e}")))?;

    let (path, total_chunks, content_type, status, _received) =
        row.ok_or_else(|| ApiError::not_found(format!("upload {upload_id} not found")))?;

    if status != "in_progress" {
        return Err(ApiError::bad_request(format!(
            "upload {upload_id} is not in progress (status={status})"
        )));
    }
    if index >= total_chunks {
        return Err(ApiError::bad_request(format!(
            "chunk index {index} out of range (total_chunks={total_chunks})"
        )));
    }

    // --- read raw body (bounded) -------------------------------------
    let body_bytes = axum::body::to_bytes(request.into_body(), MAX_CHUNK_BYTES)
        .await
        .map_err(|e| ApiError::bad_request(format!("failed to read chunk body: {e}")))?;
    if body_bytes.is_empty() {
        return Err(ApiError::bad_request("chunk body must not be empty"));
    }

    // --- stage to /tmp atomically ------------------------------------
    let upload_dir = PathBuf::from(TMP_UPLOAD_ROOT).join(upload_id.to_string());
    fs::create_dir_all(&upload_dir)
        .await
        .map_err(|e| ApiError::internal(format!("failed to create upload tmp dir: {e}")))?;

    let final_path = upload_dir.join(format!("{index}.part"));
    let tmp_path = upload_dir.join(format!("{index}.part.tmp"));
    {
        let mut f = fs::File::create(&tmp_path)
            .await
            .map_err(|e| ApiError::internal(format!("failed to open chunk tmp file: {e}")))?;
        f.write_all(&body_bytes)
            .await
            .map_err(|e| ApiError::internal(format!("failed to write chunk: {e}")))?;
        f.flush()
            .await
            .map_err(|e| ApiError::internal(format!("failed to flush chunk: {e}")))?;
    }
    fs::rename(&tmp_path, &final_path)
        .await
        .map_err(|e| ApiError::internal(format!("failed to commit chunk: {e}")))?;

    // --- idempotently record the chunk index -------------------------
    // The `NOT ($1 = ANY(received_chunks))` guard makes duplicate PUTs a
    // no-op at the SQL layer.
    sqlx::query(
        r#"
        UPDATE chunked_uploads
           SET received_chunks = array_append(received_chunks, $1)
         WHERE upload_id = $2
           AND NOT ($1 = ANY(received_chunks))
        "#,
    )
    .bind(index)
    .bind(upload_id)
    .execute(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("failed to record chunk: {e}")))?;

    // --- re-read to see if we're done --------------------------------
    let received: Vec<i32> =
        sqlx::query_scalar("SELECT received_chunks FROM chunked_uploads WHERE upload_id = $1")
            .bind(upload_id)
            .fetch_one(&state.pool)
            .await
            .map_err(|e| ApiError::internal(format!("failed to re-read chunks: {e}")))?;

    let mut assembled: Option<Vec<u8>> = None;
    if received.len() as i32 >= total_chunks {
        // All chunks present — assemble in index order.
        let mut sorted: Vec<i32> = (0..total_chunks).collect();
        sorted.sort_unstable();
        let mut buf: Vec<u8> = Vec::new();
        for idx in sorted {
            let chunk_path = upload_dir.join(format!("{idx}.part"));
            let bytes = fs::read(&chunk_path).await.map_err(|e| {
                ApiError::internal(format!("failed to read chunk {idx} for assembly: {e}"))
            })?;
            buf.extend_from_slice(&bytes);
        }
        assembled = Some(buf);
    }

    if let Some(buf) = assembled {
        // --- push to the storage backend ----------------------------
        state
            .storage_engine
            .upload(&path, &buf, &content_type, std::collections::HashMap::new())
            .await
            .map_err(|e| ApiError::internal(format!("failed to upload assembled file: {e}")))?;

        // --- best-effort tmp cleanup --------------------------------
        if let Err(e) = fs::remove_dir_all(&upload_dir).await {
            tracing::warn!(
                error = %e,
                dir = %upload_dir.display(),
                "failed to remove upload tmp dir after completion"
            );
        }

        // --- mark complete ------------------------------------------
        sqlx::query(
            r#"
            UPDATE chunked_uploads
               SET status = 'completed',
                   completed_at = now()
             WHERE upload_id = $1
            "#,
        )
        .bind(upload_id)
        .execute(&state.pool)
        .await
        .map_err(|e| ApiError::internal(format!("failed to mark upload complete: {e}")))?;

        let body = json!({
            "upload_id": upload_id,
            "status": "completed",
            "received_chunks_count": total_chunks,
            "total_chunks": total_chunks,
            "pct": 100,
            "path": path,
        });
        return Ok(negotiate_response_pub(&headers, &body));
    }

    let received_count = received.len();
    let pct = pct_from(received_count, total_chunks);
    let body = json!({
        "upload_id": upload_id,
        "status": "in_progress",
        "received_chunks_count": received_count,
        "total_chunks": total_chunks,
        "pct": pct,
        "path": path,
    });
    Ok(negotiate_response_pub(&headers, &body))
}

// ---------------------------------------------------------------------------
// Handler: GET /api/storage/upload/:upload_id/status
// ---------------------------------------------------------------------------

pub async fn upload_status(
    State(state): State<AppState>,
    Path(upload_id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let _token = crate::api::rest::extract_bearer_token_pub(&headers)?;

    let row: Option<(
        String,
        i32,
        String,
        Vec<i32>,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        r#"
        SELECT path, total_chunks, status, received_chunks, created_at, completed_at
          FROM chunked_uploads
         WHERE upload_id = $1
        "#,
    )
    .bind(upload_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| ApiError::internal(format!("failed to load upload row: {e}")))?;

    let (path, total_chunks, status, received, created_at, completed_at) =
        row.ok_or_else(|| ApiError::not_found(format!("upload {upload_id} not found")))?;

    let resp = UploadStatusResponse {
        upload_id,
        status,
        path,
        received_chunks_count: received.len(),
        total_chunks,
        pct: pct_from(received.len(), total_chunks),
        created_at,
        completed_at,
    };
    Ok(negotiate_response_pub(&headers, &resp))
}

// ---------------------------------------------------------------------------
// Background cleanup task
// ---------------------------------------------------------------------------

/// Spawn the periodic cleanup task. Returns the `JoinHandle` for tests —
/// production wires this through `tokio::spawn` in `main.rs`.
pub fn spawn_cleanup_task(pool: PgPool) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CLEANUP_INTERVAL);
        // Skip the immediate fire — we don't want to run cleanup before
        // Postgres has finished any startup migrations.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            match cleanup_stale_uploads(&pool).await {
                Ok(n) if n > 0 => {
                    tracing::info!(
                        count = n,
                        "chunked upload cleanup: purged stale in-progress uploads"
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "chunked upload cleanup failed");
                }
            }
        }
    })
}

/// Delete `chunked_uploads` rows in `in_progress` older than 24h and
/// purge their on-disk tmp dirs. Returns the number of rows removed.
pub async fn cleanup_stale_uploads(pool: &PgPool) -> Result<usize, sqlx::Error> {
    let cutoff = Utc::now() - STALE_UPLOAD_AFTER;
    let stale: Vec<Uuid> = sqlx::query_scalar(
        r#"
        DELETE FROM chunked_uploads
         WHERE status = 'in_progress'
           AND created_at < $1
         RETURNING upload_id
        "#,
    )
    .bind(cutoff)
    .fetch_all(pool)
    .await?;

    for upload_id in &stale {
        let dir = PathBuf::from(TMP_UPLOAD_ROOT).join(upload_id.to_string());
        if let Err(e) = purge_tmp_dir(&dir).await {
            tracing::warn!(
                error = %e,
                dir = %dir.display(),
                "failed to purge stale upload tmp dir"
            );
        }
    }
    Ok(stale.len())
}

async fn purge_tmp_dir(dir: &StdPath) -> std::io::Result<()> {
    match fs::metadata(dir).await {
        Ok(_) => fs::remove_dir_all(dir).await,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pct_from(received: usize, total: i32) -> u8 {
    if total <= 0 {
        return 0;
    }
    let v = (received as f64 / total as f64) * 100.0;
    v.clamp(0.0, 100.0).round() as u8
}

/// Expose the pool for wiring the cleanup task from `main.rs`.
pub fn cleanup_handle(pool: PgPool) -> Arc<PgPool> {
    Arc::new(pool)
}

// ---------------------------------------------------------------------------
// Unit tests for the sanitizer (integration tests live under tests/)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_normal_paths() {
        assert_eq!(
            sanitize_storage_path("uploads/abc/file.png").unwrap(),
            "uploads/abc/file.png"
        );
        assert_eq!(sanitize_storage_path("a.bin").unwrap(), "a.bin");
    }

    #[test]
    fn sanitize_rejects_parent_segments() {
        assert!(sanitize_storage_path("../secret").is_err());
        assert!(sanitize_storage_path("foo/../../etc/passwd").is_err());
        assert!(sanitize_storage_path("foo\\..\\bar").is_err());
    }

    #[test]
    fn sanitize_rejects_absolute_paths() {
        assert!(sanitize_storage_path("/etc/passwd").is_err());
        assert!(sanitize_storage_path("\\windows\\system32").is_err());
        assert!(sanitize_storage_path("C:/evil").is_err());
    }

    #[test]
    fn sanitize_rejects_null_and_control_bytes() {
        assert!(sanitize_storage_path("foo\0bar").is_err());
        assert!(sanitize_storage_path("foo\nbar").is_err());
        assert!(sanitize_storage_path("foo\rbar").is_err());
    }

    #[test]
    fn sanitize_rejects_empty_and_oversized() {
        assert!(sanitize_storage_path("").is_err());
        let big = "a".repeat(MAX_STORAGE_PATH_LEN + 1);
        assert!(sanitize_storage_path(&big).is_err());
    }

    #[test]
    fn pct_from_is_bounded() {
        assert_eq!(pct_from(0, 4), 0);
        assert_eq!(pct_from(2, 4), 50);
        assert_eq!(pct_from(4, 4), 100);
        assert_eq!(pct_from(10, 4), 100);
        assert_eq!(pct_from(1, 0), 0);
    }
}
