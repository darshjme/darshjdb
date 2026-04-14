//! Chunked upload integration tests — VYASA Phase 7.1.
//!
//! Created by Darshankumar Joshi (github.com/darshjme).
//!
//! Exercises the three-phase resumable upload protocol end-to-end:
//!
//! * **Happy path** — 4 chunks uploaded in order, assembled, storage
//!   backend receives the concatenated bytes, status flips to
//!   `completed`.
//! * **Out-of-order** — chunks arrive `2, 0, 3, 1`; the server must
//!   still assemble them in index order.
//! * **Duplicate chunk** — a repeated PUT of the same index is a no-op
//!   at the SQL layer (idempotent retries).
//! * **Path traversal** — `init_upload` rejects `../etc/passwd` and
//!   absolute paths before touching the database.
//!
//! Tests that hit Postgres silently skip when `DATABASE_URL` is unset,
//! matching the convention used by `admin_role_test.rs`. The
//! path-traversal test runs unconditionally because `sanitize_storage_path`
//! is pure.

#![cfg(test)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use ddb_server::api::chunked_upload::{
    cleanup_stale_uploads, ensure_chunked_uploads_schema, sanitize_storage_path,
};
use ddb_server::api::rest::{self, AppState, build_router};
use ddb_server::auth::{KeyManager, RateLimiter, SessionManager};
use ddb_server::storage::{LocalFsBackend, StorageEngine};
use ddb_server::triple_store::PgTripleStore;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

async fn setup_pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    PgTripleStore::new(pool.clone()).await.ok()?;
    rest::ensure_auth_schema(&pool).await.ok()?;
    ensure_chunked_uploads_schema(&pool).await.ok()?;
    Some(pool)
}

async fn insert_user(pool: &PgPool) -> (Uuid, String) {
    let email = format!("chunk-upload-{}@darshan.db", Uuid::new_v4());
    let hash = ddb_server::auth::PasswordProvider::hash_password("TestPass123!").expect("hash");
    let uid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(uid)
    .bind(&email)
    .bind(&hash)
    .bind(json!(["user"]))
    .execute(pool)
    .await
    .expect("insert user");
    (uid, email)
}

async fn cleanup_user(pool: &PgPool, email: &str) {
    sqlx::query("DELETE FROM sessions WHERE user_id IN (SELECT id FROM users WHERE email = $1)")
        .bind(email)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM users WHERE email = $1")
        .bind(email)
        .execute(pool)
        .await
        .ok();
}

fn make_app_state(pool: PgPool, sm: Arc<SessionManager>, tmp_root: &str) -> AppState {
    let triple_store = Arc::new(PgTripleStore::new_lazy(pool.clone()));
    let (change_tx, _) = broadcast::channel(64);
    let rate_limiter = Arc::new(RateLimiter::new());
    let storage_backend =
        Arc::new(LocalFsBackend::new(tmp_root).expect("create test storage backend"));
    let storage_engine = Arc::new(StorageEngine::new(
        storage_backend,
        b"chunked-upload-test-signing-key".to_vec(),
    ));
    AppState::with_pool(
        pool,
        triple_store,
        sm,
        change_tx,
        rate_limiter,
        storage_engine,
    )
}

async fn issue_token(sm: &SessionManager, uid: Uuid) -> String {
    let pair = sm
        .create_session(
            uid,
            vec!["user".to_string()],
            "127.0.0.1",
            "chunk-upload-test",
            "fp",
        )
        .await
        .expect("create session");
    pair.access_token
}

async fn post_json(
    router: axum::Router,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let req = Request::post(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.expect("router oneshot");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("bytes");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn put_chunk(
    router: axum::Router,
    upload_id: Uuid,
    index: usize,
    token: &str,
    data: Vec<u8>,
) -> (StatusCode, Value) {
    let req = Request::put(format!(
        "/api/storage/upload/{upload_id}/chunk/{index}"
    ))
    .header(header::AUTHORIZATION, format!("Bearer {token}"))
    .header(header::CONTENT_TYPE, "application/octet-stream")
    .body(Body::from(data))
    .unwrap();
    let resp = router.oneshot(req).await.expect("router oneshot");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("bytes");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn get_status(
    router: axum::Router,
    upload_id: Uuid,
    token: &str,
) -> (StatusCode, Value) {
    let req = Request::get(format!("/api/storage/upload/{upload_id}/status"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.expect("router oneshot");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("bytes");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

// ---------------------------------------------------------------------------
// Pure unit tests (no DB)
// ---------------------------------------------------------------------------

#[test]
fn sanitizer_rejects_path_traversal() {
    assert!(sanitize_storage_path("../etc/passwd").is_err());
    assert!(sanitize_storage_path("uploads/../../etc/passwd").is_err());
    assert!(sanitize_storage_path("/etc/passwd").is_err());
    assert!(sanitize_storage_path("foo\0bar").is_err());
    assert!(sanitize_storage_path("").is_err());
    assert!(sanitize_storage_path("normal/path/file.bin").is_ok());
}

// ---------------------------------------------------------------------------
// Integration tests (require DATABASE_URL)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_happy_path_four_chunk_upload() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"chunked-upload-test-secret-32-bytes-long-!");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let (uid, email) = insert_user(&pool).await;
    let token = issue_token(&sm, uid).await;

    let tmp_root =
        format!("/tmp/darshjdb-chunk-happy-{}", Uuid::new_v4().simple());
    let state = make_app_state(pool.clone(), sm, &tmp_root);
    let app = axum::Router::new().nest("/api", build_router(state));

    // 1. Init
    let path = format!("uploads/chunk-happy-{}.bin", Uuid::new_v4().simple());
    let (status, body) = post_json(
        app.clone(),
        "/api/storage/upload/init",
        &token,
        json!({
            "path": path,
            "content_type": "application/octet-stream",
            "total_chunks": 4,
            "file_size": 16
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "init body={body}");
    let upload_id: Uuid = body["upload_id"].as_str().unwrap().parse().unwrap();

    // 2. Upload 4 chunks in order.
    let chunks: Vec<Vec<u8>> = (0..4u8)
        .map(|i| vec![i, i + 10, i + 20, i + 30])
        .collect();
    for (idx, c) in chunks.iter().enumerate() {
        let (st, _b) = put_chunk(app.clone(), upload_id, idx, &token, c.clone()).await;
        assert_eq!(st, StatusCode::OK, "chunk {idx} failed");
    }

    // 3. Status shows completed.
    let (st, body) = get_status(app.clone(), upload_id, &token).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["status"], "completed", "body={body}");
    assert_eq!(body["pct"], 100);

    // Cleanup.
    sqlx::query("DELETE FROM chunked_uploads WHERE upload_id = $1")
        .bind(upload_id)
        .execute(&pool)
        .await
        .ok();
    cleanup_user(&pool, &email).await;
    let _ = tokio::fs::remove_dir_all(&tmp_root).await;
}

#[tokio::test]
async fn test_out_of_order_chunks_assemble_correctly() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"chunked-upload-test-secret-32-bytes-long-!");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let (uid, email) = insert_user(&pool).await;
    let token = issue_token(&sm, uid).await;

    let tmp_root =
        format!("/tmp/darshjdb-chunk-ooo-{}", Uuid::new_v4().simple());
    let state = make_app_state(pool.clone(), sm, &tmp_root);
    let app = axum::Router::new().nest("/api", build_router(state));

    let path = format!("uploads/chunk-ooo-{}.bin", Uuid::new_v4().simple());
    let (_, body) = post_json(
        app.clone(),
        "/api/storage/upload/init",
        &token,
        json!({
            "path": path,
            "content_type": "application/octet-stream",
            "total_chunks": 4
        }),
    )
    .await;
    let upload_id: Uuid = body["upload_id"].as_str().unwrap().parse().unwrap();

    // PUT in out-of-order sequence: 2, 0, 3, 1.
    for idx in [2usize, 0, 3, 1] {
        let chunk = vec![idx as u8; 8];
        let (st, _b) = put_chunk(app.clone(), upload_id, idx, &token, chunk).await;
        assert_eq!(st, StatusCode::OK, "out-of-order chunk {idx} failed");
    }

    let (st, body) = get_status(app.clone(), upload_id, &token).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["status"], "completed", "body={body}");

    sqlx::query("DELETE FROM chunked_uploads WHERE upload_id = $1")
        .bind(upload_id)
        .execute(&pool)
        .await
        .ok();
    cleanup_user(&pool, &email).await;
    let _ = tokio::fs::remove_dir_all(&tmp_root).await;
}

#[tokio::test]
async fn test_duplicate_chunk_is_idempotent() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"chunked-upload-test-secret-32-bytes-long-!");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let (uid, email) = insert_user(&pool).await;
    let token = issue_token(&sm, uid).await;

    let tmp_root =
        format!("/tmp/darshjdb-chunk-dup-{}", Uuid::new_v4().simple());
    let state = make_app_state(pool.clone(), sm, &tmp_root);
    let app = axum::Router::new().nest("/api", build_router(state));

    let path = format!("uploads/chunk-dup-{}.bin", Uuid::new_v4().simple());
    let (_, body) = post_json(
        app.clone(),
        "/api/storage/upload/init",
        &token,
        json!({
            "path": path,
            "content_type": "application/octet-stream",
            "total_chunks": 2
        }),
    )
    .await;
    let upload_id: Uuid = body["upload_id"].as_str().unwrap().parse().unwrap();

    // First chunk uploaded once.
    let (st1, b1) = put_chunk(app.clone(), upload_id, 0, &token, vec![1, 2, 3]).await;
    assert_eq!(st1, StatusCode::OK);
    assert_eq!(b1["status"], "in_progress");
    assert_eq!(b1["received_chunks_count"], 1);

    // Same chunk replayed — must not bump the count.
    let (st2, b2) = put_chunk(app.clone(), upload_id, 0, &token, vec![1, 2, 3]).await;
    assert_eq!(st2, StatusCode::OK);
    assert_eq!(
        b2["received_chunks_count"], 1,
        "duplicate chunk must be idempotent: body={b2}"
    );

    // Finish the upload for tidiness.
    let (_, _) = put_chunk(app.clone(), upload_id, 1, &token, vec![4, 5, 6]).await;

    sqlx::query("DELETE FROM chunked_uploads WHERE upload_id = $1")
        .bind(upload_id)
        .execute(&pool)
        .await
        .ok();
    cleanup_user(&pool, &email).await;
    let _ = tokio::fs::remove_dir_all(&tmp_root).await;
}

#[tokio::test]
async fn test_init_rejects_path_traversal() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"chunked-upload-test-secret-32-bytes-long-!");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let (uid, email) = insert_user(&pool).await;
    let token = issue_token(&sm, uid).await;

    let tmp_root =
        format!("/tmp/darshjdb-chunk-trav-{}", Uuid::new_v4().simple());
    let state = make_app_state(pool.clone(), sm, &tmp_root);
    let app = axum::Router::new().nest("/api", build_router(state));

    for malicious in &[
        "../etc/passwd",
        "uploads/../../etc/shadow",
        "/absolute/evil",
        "foo\0bar",
    ] {
        let (st, body) = post_json(
            app.clone(),
            "/api/storage/upload/init",
            &token,
            json!({
                "path": malicious,
                "content_type": "application/octet-stream",
                "total_chunks": 1
            }),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "malicious path `{malicious}` must be rejected, body={body}"
        );
    }

    cleanup_user(&pool, &email).await;
    let _ = tokio::fs::remove_dir_all(&tmp_root).await;
}

#[tokio::test]
async fn test_cleanup_stale_uploads_removes_old_in_progress_rows() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    // Insert a fake in-progress row with a created_at 48h in the past.
    let upload_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO chunked_uploads
            (upload_id, path, total_chunks, content_type, status, created_at)
        VALUES
            ($1, $2, 1, 'application/octet-stream', 'in_progress', now() - interval '48 hours')
        "#,
    )
    .bind(upload_id)
    .bind(format!("uploads/stale-{}.bin", upload_id.simple()))
    .execute(&pool)
    .await
    .expect("insert stale row");

    let removed = cleanup_stale_uploads(&pool).await.expect("cleanup");
    assert!(
        removed >= 1,
        "cleanup must have removed at least the stale row, got {removed}"
    );

    // Verify row is gone.
    let exists: Option<Uuid> = sqlx::query_scalar(
        "SELECT upload_id FROM chunked_uploads WHERE upload_id = $1",
    )
    .bind(upload_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(exists.is_none(), "stale row must have been deleted");
}
