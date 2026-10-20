//! Real HTTP route authorization regression against PostgreSQL + pgvector.
//! Set DATABASE_URL to execute; an absent URL skips the database test.
//! Connection/schema errors with a configured URL fail instead of silently skipping.
//! Covers idle query and raw-event stream revocation without data changes.

#![cfg(test)]

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use ddb_server::api::rest::{self, AppState, build_router};
use ddb_server::auth::{KeyManager, RateLimiter, SessionManager};
use ddb_server::storage::{LocalFsBackend, StorageEngine};
use ddb_server::triple_store::PgTripleStore;
use futures::StreamExt;
use serde_json::json;
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
    let pool = PgPool::connect(&url).await.expect("database connection");
    PgTripleStore::new(pool.clone())
        .await
        .expect("triple schema");
    rest::ensure_auth_schema(&pool).await.expect("auth schema");

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

/// No change notification is emitted after revocation: only the idle timer can end these streams.
#[tokio::test]
async fn idle_query_and_raw_streams_report_revoked_session() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let sm = Arc::new(SessionManager::new(
        pool.clone(),
        KeyManager::from_secret(b"idle-stream-test-secret-at-least-32"),
    ));
    let (uid, email) = insert_user(&pool, vec!["admin"]).await;
    let token = issue_token(&sm, uid, vec!["admin"]).await;
    let state = make_app_state(pool.clone(), sm);
    let router = axum::Router::new().nest("/api", build_router(state));
    let q = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("q", &json!({"type":"idle_recheck"}).to_string())
        .finish();
    let mut streams = Vec::new();
    for path in [
        format!("/api/subscribe?{q}"),
        "/api/events?channel=entity:*".into(),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::get(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        streams.push(response.into_body().into_data_stream());
    }
    sqlx::query("UPDATE sessions SET revoked = true, revoked_at = now() WHERE user_id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();
    for mut stream in streams {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let bytes = stream
                    .next()
                    .await
                    .expect("stream remains open until auth error")
                    .unwrap();
                if String::from_utf8_lossy(&bytes).contains("event: auth-error") {
                    break;
                }
            }
        })
        .await
        .expect("idle stream must revalidate without data changes");
    }
    cleanup_user(&pool, &email).await;
}
