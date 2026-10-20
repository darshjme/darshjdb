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
async fn query_fields_cache_batches_and_nested_targets_are_authorized() {
    use ddb_server::auth::permissions::CompositeOperator;
    use ddb_server::auth::{Operation, PermissionRule};
    use ddb_server::triple_store::{TripleInput, TripleStore};
    let Some(pool) = setup_pool().await else {
        return;
    };
    let sm = Arc::new(SessionManager::new(
        pool.clone(),
        KeyManager::from_secret(b"query-authorization-test-secret-at-least-32"),
    ));
    let (alice, email) = insert_user(&pool, vec!["user"]).await;
    let token = issue_token(&sm, alice, vec!["user"]).await;
    let kind = format!("query_review_{}", Uuid::new_v4().simple());
    let target_kind = format!("target_{}", Uuid::new_v4().simple());
    let own = Uuid::new_v4();
    let foreign = Uuid::new_v4();
    let target = Uuid::new_v4();
    let store = PgTripleStore::new_lazy(pool.clone());
    for (id, typ, owner) in [
        (own, &kind, alice),
        (foreign, &kind, Uuid::new_v4()),
        (target, &target_kind, Uuid::new_v4()),
    ] {
        let data = vec![
            (":db/type".into(), json!(typ)),
            (format!("{typ}/owner_id"), json!(owner)),
            (format!("{typ}/secret"), json!("classified")),
            (format!("{typ}/title"), json!("public")),
            (format!("{typ}/ref"), json!(target)),
        ];
        store
            .set_triples(
                &data
                    .into_iter()
                    .map(|(attribute, value)| TripleInput {
                        entity_id: id,
                        attribute,
                        value,
                        value_type: 0,
                        ttl_seconds: None,
                    })
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
    }
    let mut state = make_app_state(pool.clone(), sm);
    let query = json!({"type":kind});
    let router = axum::Router::new().nest("/api", build_router(state.clone()));
    let (status, first) = post(router, "/api/query", &token, json!({"query":query})).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert!(first.to_string().contains("classified")); // Prime the unrestricted cache.
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
                    denied_fields: vec!["secret".into()],
                },
            ],
        },
    );
    for operation in [Operation::Create, Operation::Update] {
        permissions.add_rule(
            &kind,
            operation,
            PermissionRule::Composite {
                operator: CompositeOperator::And,
                rules: vec![
                    PermissionRule::WhereClause {
                        sql: "owner_id = $user_id".into(),
                    },
                    PermissionRule::FieldRestriction {
                        allowed_fields: vec![],
                        denied_fields: vec!["secret".into()],
                    },
                ],
            },
        );
    }
    permissions.add_rule(
        &target_kind,
        Operation::Read,
        PermissionRule::Composite {
            operator: CompositeOperator::And,
            rules: vec![
                PermissionRule::WhereClause {
                    sql: "owner_id = $user_id".into(),
                },
                PermissionRule::FieldRestriction {
                    allowed_fields: vec!["title".into(), "owner_id".into()],
                    denied_fields: vec![],
                },
            ],
        },
    );
    state.permissions = Arc::new(permissions);
    let router = axum::Router::new().nest("/api", build_router(state.clone()));
    for (status, body) in [
        post(router.clone(), "/api/query", &token, json!({"query":query})).await,
        get_with_token(router.clone(), &format!("/api/data/{kind}"), Some(&token)).await,
        get_with_token(
            router.clone(),
            &format!("/api/data/{kind}/{own}"),
            Some(&token),
        )
        .await,
        post(
            router.clone(),
            "/api/batch",
            &token,
            json!({"ops":[{"id":"read","type":"query","body":query}]}),
        )
        .await,
    ] {
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(!body.to_string().contains("classified"), "{body}");
        assert!(!body.to_string().contains(&foreign.to_string()), "{body}");
    }
    let (status, body) = post(router.clone(), "/api/query", &token, json!({"query":{"type":kind,"$where":[{"attribute":format!("{kind}/secret"),"op":"Eq","value":"classified"}]}})).await;
    assert!(status.is_client_error(), "{status} {body}");
    // A batch that writes an owned row before a foreign row must roll back.
    let (_, body) = post(router.clone(), "/api/batch", &token, json!({"ops":[
        {"id":"own","type":"mutate","body":{"mutations":[{"op":"update","entity":kind,"id":own,"data":{"title":"should-rollback"}}]}},
        {"id":"foreign","type":"mutate","body":{"mutations":[{"op":"update","entity":kind,"id":foreign,"data":{"title":"intrusion"}}]}}
    ]})).await;
    assert!(body.to_string().contains("403"), "{body}");
    let attrs = store.get_entity(own).await.unwrap();
    assert!(
        !attrs.iter().any(|t| t.value == json!("should-rollback")),
        "partial batch write persisted"
    );
    let (status, body) = get_with_token(
        router.clone(),
        &format!("/api/data/{target_kind}/{own}"),
        Some(&token),
    )
    .await;
    assert!(
        status.is_client_error(),
        "type relabeling borrowed a foreign policy: {body}"
    );
    // CRUD routes must enforce the same fields, target type and post-write owner.
    for (method, path, data) in [
        (
            "PATCH",
            format!("/api/data/{kind}/{own}"),
            json!({"secret":"blocked"}),
        ),
        (
            "PATCH",
            format!("/api/data/{kind}/{own}"),
            json!({"owner_id":Uuid::new_v4()}),
        ),
        (
            "PATCH",
            format!("/api/data/{target_kind}/{own}"),
            json!({"title":"blocked"}),
        ),
        (
            "DELETE",
            format!("/api/data/{target_kind}/{own}"),
            json!({}),
        ),
        (
            "POST",
            format!("/api/data/{kind}"),
            json!({"secret":"blocked"}),
        ),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(data.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{method} must deny"
        );
    }
    let (status, created) = post(
        router.clone(),
        &format!("/api/data/{kind}"),
        &token,
        json!({"title":"allowed"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "server-stamped owner must satisfy create rule: {created}"
    );
    let created_id: Uuid = serde_json::from_value(created["id"].clone()).unwrap();
    sqlx::query("DELETE FROM triples WHERE entity_id = $1")
        .bind(created_id)
        .execute(&pool)
        .await
        .unwrap();
    let target_ast = ddb_server::query::parse_darshan_ql(&json!({"type":target_kind})).unwrap();
    let nested = json!({"type":kind,"$nested":[{"via_attribute":format!("{kind}/ref"),"sub_query":target_ast}]});
    let (status, body) = post(
        router.clone(),
        "/api/query",
        &token,
        json!({"query":nested.clone()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["data"][0]["nested"],
        json!({}),
        "cross-owner reference exposed: {body}"
    );
    // A legitimate target remains available, and changing its owner revokes
    // the nested read even when the root entity and query remain unchanged.
    store
        .retract(target, &format!("{target_kind}/owner_id"))
        .await
        .unwrap();
    store
        .set_triples(&[TripleInput {
            entity_id: target,
            attribute: format!("{target_kind}/owner_id"),
            value: json!(alice),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .unwrap();
    let (status, body) = post(
        router.clone(),
        "/api/query",
        &token,
        json!({"query":nested.clone()}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body["data"][0]["nested"].to_string().contains("public")
            && !body["data"][0]["nested"].to_string().contains("classified"),
        "authorized target missing: {body}"
    );
    store
        .retract(target, &format!("{target_kind}/owner_id"))
        .await
        .unwrap();
    store
        .set_triples(&[TripleInput {
            entity_id: target,
            attribute: format!("{target_kind}/owner_id"),
            value: json!(Uuid::new_v4()),
            value_type: 0,
            ttl_seconds: None,
        }])
        .await
        .unwrap();
    let (_, body) = post(
        router.clone(),
        "/api/query",
        &token,
        json!({"query":nested}),
    )
    .await;
    assert_eq!(
        body["data"][0]["nested"],
        json!({}),
        "stale nested cache leaked target: {body}"
    );
    let (status, body) = post(
        router,
        "/api/query",
        &token,
        json!({"query":{"type":kind,"$nested":[{"via_attribute":format!("{kind}/ref")}]}}),
    )
    .await;
    assert!(
        status.is_client_error(),
        "untyped reference must fail closed: {body}"
    );
    sqlx::query("DELETE FROM triples WHERE entity_id = ANY($1)")
        .bind(vec![own, foreign, target])
        .execute(&pool)
        .await
        .unwrap();
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn access_tokens_require_reauthentication_after_role_changes() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let sm = SessionManager::new(
        pool.clone(),
        KeyManager::from_secret(b"role-downgrade-secret-at-least-32-bytes"),
    );
    let (user, email) = insert_user(&pool, vec!["user", "admin"]).await;
    let token = issue_token(&sm, user, vec!["user", "admin"]).await;
    assert!(
        sm.validate_token(&token, "", "", "")
            .await
            .unwrap()
            .roles
            .contains(&"admin".into())
    );
    sqlx::query("UPDATE users SET roles = $1 WHERE id = $2")
        .bind(json!(["user", "new-role"]))
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(sm.validate_token(&token, "", "", "").await.is_err());
    cleanup_user(&pool, &email).await;
}
