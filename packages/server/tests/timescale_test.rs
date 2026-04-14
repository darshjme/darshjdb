//! Time-series REST + migration integration tests — Phase 5.1.
//!
//! Author: Darshankumar Joshi
//!
//! Exercises the new `/api/ts/*` routes end-to-end. The tests connect
//! to the real database using `DATABASE_URL` and silently no-op when it
//! is unset so CI stays green on environments without Postgres (same
//! pattern as `admin_role_test.rs`).
//!
//! TimescaleDB-specific assertions (hypertable present, bucket grouping
//! via `time_bucket`) are skipped automatically when the extension is
//! absent — we probe `pg_extension` first, then dispatch.

#![cfg(test)]

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use ddb_server::api::rest::{self, build_router, AppState};
use ddb_server::auth::{KeyManager, RateLimiter, SessionManager};
use ddb_server::storage::{LocalFsBackend, StorageEngine};
use ddb_server::triple_store::PgTripleStore;
use serde_json::{json, Value};
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
    ensure_time_series_schema(&pool).await.ok()?;
    Some(pool)
}

/// Idempotently apply the Phase 5.1 migration so the test suite can run
/// without `sqlx migrate` being wired into CI. Mirrors the migration at
/// `migrations/20260414090000_timescale.sql`.
async fn ensure_time_series_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS time_series (
            time         TIMESTAMPTZ      NOT NULL,
            entity_id    UUID             NOT NULL,
            entity_type  TEXT             NOT NULL,
            attribute    TEXT             NOT NULL,
            value_num    DOUBLE PRECISION,
            value_text   TEXT,
            value_json   JSONB,
            tags         JSONB            NOT NULL DEFAULT '{}'::jsonb,
            PRIMARY KEY (entity_type, entity_id, attribute, time)
        );
        CREATE INDEX IF NOT EXISTS idx_time_series_entity_time
            ON time_series (entity_type, entity_id, time DESC);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn has_timescale(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'timescaledb')",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

async fn insert_user(pool: &PgPool) -> (Uuid, String) {
    let email = format!("ts-test-{}@darshan.db", Uuid::new_v4());
    let hash =
        ddb_server::auth::PasswordProvider::hash_password("TestPass123!").expect("hash password");
    let uid = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(uid)
    .bind(&email)
    .bind(&hash)
    .bind(json!(["admin", "user"]))
    .execute(pool)
    .await
    .expect("insert user");
    (uid, email)
}

async fn cleanup(pool: &PgPool, email: &str, entity_type: &str) {
    sqlx::query("DELETE FROM time_series WHERE entity_type = $1")
        .bind(entity_type)
        .execute(pool)
        .await
        .ok();
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

fn make_app_state(pool: PgPool, sm: Arc<SessionManager>) -> AppState {
    let triple_store = Arc::new(PgTripleStore::new_lazy(pool.clone()));
    let (change_tx, _) = broadcast::channel(64);
    let rate_limiter = Arc::new(RateLimiter::new());
    let storage_backend = Arc::new(
        LocalFsBackend::new("/tmp/darshjdb-ts-test-storage").expect("create storage backend"),
    );
    let storage_engine = Arc::new(StorageEngine::new(
        storage_backend,
        b"ts-test-signing-key".to_vec(),
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
            vec!["admin".into(), "user".into()],
            "127.0.0.1",
            "ts-test",
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

async fn get_path(router: axum::Router, path: &str, token: &str) -> (StatusCode, Value) {
    let req = Request::get(path)
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
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ts_insert_and_range_scan_roundtrip() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let entity_type = format!("sensor_{}", Uuid::new_v4().simple());
    let entity_id = Uuid::new_v4();

    let km = KeyManager::from_secret(b"ts-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let (uid, email) = insert_user(&pool).await;
    let token = issue_token(&sm, uid).await;

    let state = make_app_state(pool.clone(), sm);

    // Insert 3 points.
    for (i, v) in [21.5f64, 22.0, 22.7].iter().enumerate() {
        let app = axum::Router::new().nest("/api", build_router(state.clone()));
        let (status, body) = post_json(
            app,
            &format!("/api/ts/{entity_type}"),
            &token,
            json!({
                "entity_id": entity_id,
                "attribute": "temperature",
                "value_num": v,
                "tags": { "unit": "celsius", "seq": i },
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "insert failed: {body}");
        assert_eq!(body.get("ok"), Some(&Value::Bool(true)));
    }

    // Range scan should return the 3 points for this entity_type.
    let app = axum::Router::new().nest("/api", build_router(state.clone()));
    let (status, body) = get_path(
        app,
        &format!("/api/ts/{entity_type}?attribute=temperature"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "range failed: {body}");
    let count = body.get("count").and_then(Value::as_u64).unwrap_or(0);
    assert_eq!(count, 3, "expected 3 points, got {count}: {body}");

    // Latest endpoint must return exactly one row (the newest).
    let app = axum::Router::new().nest("/api", build_router(state.clone()));
    let (status, body) = get_path(
        app,
        &format!("/api/ts/{entity_type}/latest?attribute=temperature"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "latest failed: {body}");
    let count = body.get("count").and_then(Value::as_u64).unwrap_or(0);
    assert_eq!(count, 1, "expected 1 latest, got {count}: {body}");

    cleanup(&pool, &email, &entity_type).await;
}

#[tokio::test]
async fn ts_aggregate_bucket_timescale_or_fallback() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let entity_type = format!("metric_{}", Uuid::new_v4().simple());
    let entity_id = Uuid::new_v4();

    let km = KeyManager::from_secret(b"ts-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let (uid, email) = insert_user(&pool).await;
    let token = issue_token(&sm, uid).await;
    let state = make_app_state(pool.clone(), sm);

    // Seed 5 points.
    for i in 0..5 {
        let app = axum::Router::new().nest("/api", build_router(state.clone()));
        let (status, _) = post_json(
            app,
            &format!("/api/ts/{entity_type}"),
            &token,
            json!({
                "entity_id": entity_id,
                "attribute": "cpu",
                "value_num": i as f64 * 10.0,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
    }

    // `1h` bucket maps to `date_trunc('hour', ...)` on vanilla Postgres,
    // so the aggregation endpoint must succeed in either mode.
    let app = axum::Router::new().nest("/api", build_router(state.clone()));
    let (status, body) = get_path(
        app,
        &format!("/api/ts/{entity_type}/agg?fn=avg&bucket=1h&attribute=cpu"),
        &token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "agg failed: {body}");
    let count = body.get("count").and_then(Value::as_u64).unwrap_or(0);
    assert!(count >= 1, "expected >=1 bucket row, got {count}: {body}");

    // TimescaleDB-only: confirm the hypertable is registered.
    if has_timescale(&pool).await {
        let rows: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM timescaledb_information.hypertables WHERE hypertable_name = 'time_series'",
        )
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
        assert!(
            rows >= 1,
            "time_series should be a hypertable when timescaledb is present"
        );
    }

    cleanup(&pool, &email, &entity_type).await;
}

#[tokio::test]
async fn ts_rejects_missing_value() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let entity_type = format!("sensor_{}", Uuid::new_v4().simple());

    let km = KeyManager::from_secret(b"ts-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let (uid, email) = insert_user(&pool).await;
    let token = issue_token(&sm, uid).await;
    let state = make_app_state(pool.clone(), sm);

    let app = axum::Router::new().nest("/api", build_router(state));
    let (status, _body) = post_json(
        app,
        &format!("/api/ts/{entity_type}"),
        &token,
        json!({
            "entity_id": Uuid::new_v4(),
            "attribute": "temp",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    cleanup(&pool, &email, &entity_type).await;
}

#[tokio::test]
async fn ts_rejects_unauthenticated() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"ts-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));
    let state = make_app_state(pool.clone(), sm);
    let app = axum::Router::new().nest("/api", build_router(state));
    let req = Request::get("/api/ts/sensor/latest")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.expect("oneshot");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
