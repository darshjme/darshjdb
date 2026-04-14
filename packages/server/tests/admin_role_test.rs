//! Admin role enforcement integration tests.
//!
//! Phase 0.1 — exercises the real `require_admin_auth` extractor wired
//! onto every `/api/admin/*` route in `build_router`. These tests use
//! the real `SessionManager` so JWT signatures are cryptographically
//! verified rather than merely decoded, proving the gate can't be
//! bypassed with an unsigned payload.
//!
//! Requires `DATABASE_URL` to be set. When unset, every test silently
//! passes (returning early) so they stay compatible with the rest of
//! the integration suite.

#![cfg(test)]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
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
// Harness helpers
// ---------------------------------------------------------------------------

/// Connect to the integration database and ensure the auth schema exists.
///
/// Returns `None` when `DATABASE_URL` is not set so the test can no-op.
async fn setup_pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    PgTripleStore::new(pool.clone()).await.ok()?;
    rest::ensure_auth_schema(&pool).await.ok()?;
    Some(pool)
}

/// Insert a user with the given roles and return its id and email.
async fn insert_user(pool: &PgPool, roles: Vec<&str>) -> (Uuid, String) {
    let email = format!("admin-role-{}@darshan.db", Uuid::new_v4());
    let hash = ddb_server::auth::PasswordProvider::hash_password("TestPass123!").expect("hash");
    let uid = Uuid::new_v4();
    let roles_json: Vec<String> = roles.into_iter().map(String::from).collect();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(uid)
    .bind(&email)
    .bind(&hash)
    .bind(json!(roles_json))
    .execute(pool)
    .await
    .expect("insert user");
    (uid, email)
}

/// Drop the test user and its sessions.
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

/// Build a fully wired `AppState` backed by the given pool and session
/// manager so the admin routes share the same keys used to sign test
/// tokens.
fn make_app_state(pool: PgPool, sm: Arc<SessionManager>) -> AppState {
    let triple_store = Arc::new(PgTripleStore::new_lazy(pool.clone()));
    let (change_tx, _) = broadcast::channel(64);
    let rate_limiter = Arc::new(RateLimiter::new());
    let storage_backend = Arc::new(
        LocalFsBackend::new("/tmp/darshjdb-admin-role-test-storage")
            .expect("create test storage backend"),
    );
    let storage_engine = Arc::new(StorageEngine::new(
        storage_backend,
        b"admin-role-test-signing-key".to_vec(),
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

/// Issue an access token for `uid` with the given `roles` using `sm`.
async fn issue_token(sm: &SessionManager, uid: Uuid, roles: Vec<&str>) -> String {
    let roles: Vec<String> = roles.into_iter().map(String::from).collect();
    let pair = sm
        .create_session(uid, roles, "127.0.0.1", "admin-role-test", "fp")
        .await
        .expect("create session");
    pair.access_token
}

/// Send a GET to `path` on the router with an optional bearer token.
async fn get_with_token(
    router: axum::Router,
    path: &str,
    token: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::get(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = router
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .expect("router oneshot");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.expect("bytes");
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A user whose JWT lacks the "admin" role must receive 403 on every
/// admin endpoint, including when the token is otherwise valid.
#[tokio::test]
async fn test_non_admin_cannot_access_admin_routes() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"admin-role-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));

    let (uid, email) = insert_user(&pool, vec!["user"]).await;
    let token = issue_token(&sm, uid, vec!["user"]).await;

    let state = make_app_state(pool.clone(), sm);
    let app = axum::Router::new().nest("/api", build_router(state));

    let (status, body) = get_with_token(app, "/api/admin/schema", Some(&token)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-admin must be rejected, got body={body}"
    );

    cleanup_user(&pool, &email).await;
}

/// A user whose JWT carries the "admin" role must receive 200 and a
/// schema payload back from `GET /api/admin/schema`.
#[tokio::test]
async fn test_admin_can_access_admin_routes() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"admin-role-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));

    let (uid, email) = insert_user(&pool, vec!["admin", "user"]).await;
    let token = issue_token(&sm, uid, vec!["admin", "user"]).await;

    let state = make_app_state(pool.clone(), sm);
    let app = axum::Router::new().nest("/api", build_router(state));

    let (status, _body) = get_with_token(app, "/api/admin/schema", Some(&token)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "admin must be allowed through, got {status}"
    );

    cleanup_user(&pool, &email).await;
}

/// Requests without a bearer token must be rejected with 401 before
/// role checking runs.
#[tokio::test]
async fn test_admin_routes_reject_missing_token() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"admin-role-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));

    let state = make_app_state(pool.clone(), sm);
    let app = axum::Router::new().nest("/api", build_router(state));

    let (status, _body) = get_with_token(app, "/api/admin/schema", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// An unsigned / fake JWT must be rejected with 401, not silently
/// accepted because its payload happens to contain `roles: ["admin"]`.
/// This is the regression guard for the original stub that decoded
/// claims without signature verification.
#[tokio::test]
async fn test_admin_routes_reject_unsigned_jwt() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let km = KeyManager::from_secret(b"admin-role-test-secret-at-least-32-bytes-long");
    let sm = Arc::new(SessionManager::new(pool.clone(), km));

    // Hand-craft a JWT whose payload claims admin but whose signature
    // is bogus. A real `SessionManager::validate_token` must reject it.
    let header_b64 = data_encoding::BASE64URL_NOPAD.encode(b"{\"alg\":\"HS256\",\"typ\":\"JWT\"}");
    let payload_b64 = data_encoding::BASE64URL_NOPAD.encode(
        serde_json::to_string(&json!({
            "sub": Uuid::new_v4().to_string(),
            "sid": Uuid::new_v4().to_string(),
            "roles": ["admin"],
            "iat": 0,
            "exp": 9999999999i64,
            "iss": "darshjdb",
            "aud": "darshjdb",
        }))
        .unwrap()
        .as_bytes(),
    );
    let sig_b64 = data_encoding::BASE64URL_NOPAD.encode(b"forged-signature");
    let forged = format!("{header_b64}.{payload_b64}.{sig_b64}");

    let state = make_app_state(pool.clone(), sm);
    let app = axum::Router::new().nest("/api", build_router(state));

    let (status, _body) = get_with_token(app, "/api/admin/schema", Some(&forged)).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "forged JWT must be rejected"
    );
}
