//! Real HTTP route authorization regression against PostgreSQL + pgvector.
//! Set DATABASE_URL to execute; an absent URL skips the database test.
//! Connection/schema errors with a configured URL fail instead of silently skipping.
//! Covers ownership before search limits, denied-field inference, embedding
//! writes/metadata, and admin-only DarshQL containment.

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
    let pool = PgPool::connect(&url).await.expect("database connection");
    PgTripleStore::new(pool.clone())
        .await
        .expect("triple schema");
    rest::ensure_auth_schema(&pool).await.expect("auth schema");
    rest::ensure_search_schema(&pool)
        .await
        .expect("search schema");
    sqlx::query("SELECT 1 FROM embeddings LIMIT 1")
        .execute(&pool)
        .await
        .expect("pgvector required");
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

async fn post(router: axum::Router, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let response = router
        .oneshot(
            Request::post(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn search_and_embedding_routes_enforce_owner_and_fields() {
    use ddb_server::auth::permissions::CompositeOperator;
    use ddb_server::auth::{Operation, PermissionRule};
    use ddb_server::triple_store::{TripleInput, TripleStore};
    let Some(pool) = setup_pool().await else {
        return;
    };
    let sm = Arc::new(SessionManager::new(
        pool.clone(),
        KeyManager::from_secret(b"search-authorization-test-secret-at-least-32"),
    ));
    let (alice, ae) = insert_user(&pool, vec!["user"]).await;
    let (bob, be) = insert_user(&pool, vec!["user"]).await;
    let at = issue_token(&sm, alice, vec!["user"]).await;
    let bt = issue_token(&sm, bob, vec!["user"]).await;
    let mut state = make_app_state(pool.clone(), sm.clone());
    let kind = format!("review_{}", Uuid::new_v4().simple());
    let own = Uuid::new_v4();
    let foreign = Uuid::new_v4();
    let store = PgTripleStore::new_lazy(pool.clone());
    for (id, owner, text) in [
        (own, bob, "needle"),
        (foreign, alice, "needle needle needle needle"),
    ] {
        let triples = [
            (":db/type".to_string(), json!(kind)),
            (format!("{kind}/owner_id"), json!(owner)),
            (format!("{kind}/body"), json!(text)),
            (format!("{kind}/secret"), json!("classified")),
        ]
        .into_iter()
        .map(|(attribute, value)| TripleInput {
            entity_id: id,
            attribute,
            value,
            value_type: 0,
            ttl_seconds: None,
        })
        .collect::<Vec<_>>();
        store.set_triples(&triples).await.unwrap();
    }
    let router = axum::Router::new().nest("/api", build_router(state.clone()));
    assert_eq!(
        get_with_token(
            router.clone(),
            &format!("/api/search/text?q=needle&entity_type={kind}"),
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, body) = get_with_token(
        router.clone(),
        &format!("/api/search/text?q=needle&entity_type={kind}&limit=1"),
        Some(&bt),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"][0]["entity_id"], json!(own));
    let vector = vec![1.0f32; 1536];
    let embedding = json!({"entity_id":foreign,"embedding":vector,"attribute":"body"});
    assert_eq!(
        post(router.clone(), "/api/embeddings", &bt, embedding.clone())
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = post(router.clone(), "/api/embeddings", &at, embedding).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        get_with_token(
            router.clone(),
            &format!("/api/embeddings/{foreign}"),
            Some(&bt)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            router.clone(),
            "/api/embeddings",
            &bt,
            json!({"entity_id":own,"embedding":vector,"attribute":"body"})
        )
        .await
        .0,
        StatusCode::CREATED
    );
    sqlx::query("UPDATE users SET roles = $1 WHERE id = $2")
        .bind(json!(["admin"]))
        .bind(alice)
        .execute(&pool)
        .await
        .unwrap();
    let admin = issue_token(&sm, alice, vec!["admin"]).await;
    for path in ["/api/darshql", "/api/sql/darshql"] {
        assert_eq!(
            post(
                router.clone(),
                path,
                &bt,
                json!({"query":format!("DELETE {kind};")})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            post(
                router.clone(),
                path,
                &admin,
                json!({"query":format!("SELECT * FROM {kind};")})
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let surviving: i64 =
        sqlx::query_scalar("SELECT count(*) FROM triples WHERE entity_id = $1 AND NOT retracted")
            .bind(foreign)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(surviving, 4, "denied DELETE must leave foreign rows intact");
    let (status, body) = post(
        router.clone(),
        "/api/search/semantic",
        &bt,
        json!({"entity_type":kind,"vector":vector,"limit":1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"][0]["entity_id"], json!(own));
    let (status, body) = post(
        router.clone(),
        "/api/search/hybrid",
        &bt,
        json!({"entity_type":kind,"query_text":"needle","vector":vector,"limit":1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"][0]["id"], json!(own));
    let mut permissions = ddb_server::auth::default_permissions::build_default_engine();
    permissions.add_rule(
        &kind,
        Operation::Read,
        PermissionRule::Composite {
            operator: CompositeOperator::And,
            rules: vec![
                PermissionRule::WhereClause {
                    sql: "owner_id = $user_id".into(),
                },
                PermissionRule::FieldRestriction {
                    allowed_fields: vec![],
                    denied_fields: vec!["secret".into(), "body".into()],
                },
            ],
        },
    );
    state.permissions = Arc::new(permissions);
    let router = axum::Router::new().nest("/api", build_router(state.clone()));
    let (status, body) = get_with_token(
        router.clone(),
        &format!("/api/search/text?q=classified&entity_type={kind}"),
        Some(&bt),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"], json!([]));
    let (status, body) = post(
        router.clone(),
        "/api/search/semantic",
        &bt,
        json!({"entity_type":kind,"vector":vector}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"], json!([]));
    // An allowlist keeps permitted lexical recall while withholding all vectors.
    let mut permissions = ddb_server::auth::default_permissions::build_default_engine();
    permissions.add_rule(
        &kind,
        Operation::Read,
        PermissionRule::Composite {
            operator: CompositeOperator::And,
            rules: vec![
                PermissionRule::WhereClause {
                    sql: "owner_id = $user_id".into(),
                },
                PermissionRule::FieldRestriction {
                    allowed_fields: vec!["body".into()],
                    denied_fields: vec![],
                },
            ],
        },
    );
    state.permissions = Arc::new(permissions);
    let router = axum::Router::new().nest("/api", build_router(state));
    let (status, body) = post(
        router.clone(),
        "/api/search/hybrid",
        &bt,
        json!({"entity_type":kind,"query_text":"needle","vector":vector,"limit":1}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"].as_array().unwrap().len(), 1);
    assert_eq!(body["data"][0]["id"], json!(own));
    assert!(
        body["data"][0]["semantic_score"].is_null(),
        "restricted hybrid must not use vector scores"
    );
    assert!(body["data"][0]["text_score"].as_f64().unwrap() > 0.0);
    let (status, body) = get_with_token(
        router.clone(),
        &format!("/api/search/text?q=classified&entity_type={kind}"),
        Some(&bt),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"],
        json!([]),
        "non-allowlisted fields must not match"
    );
    // A deleted entity's retained embedding cannot be replaced or resurrected.
    sqlx::query("UPDATE triples SET retracted = true WHERE entity_id = $1")
        .bind(own)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = post(
        router.clone(),
        "/api/embeddings",
        &bt,
        json!({"entity_id":own,"embedding":vector,"attribute":"body","model":"must-not-write"}),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let model: String = sqlx::query_scalar(
        "SELECT model FROM embeddings WHERE entity_id = $1 AND attribute = 'body'",
    )
    .bind(own)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_ne!(model, "must-not-write");
    sqlx::query("DELETE FROM embeddings WHERE entity_id = ANY($1)")
        .bind(vec![own, foreign])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM triples WHERE entity_id = ANY($1)")
        .bind(vec![own, foreign])
        .execute(&pool)
        .await
        .unwrap();
    cleanup_user(&pool, &ae).await;
    cleanup_user(&pool, &be).await;
}
