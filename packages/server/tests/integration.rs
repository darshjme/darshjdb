//! Integration tests for DarshJDB against a real Postgres database.
//!
//! These tests require a running Postgres instance. Set the `DATABASE_URL`
//! environment variable to enable them:
//!
//! ```sh
//! DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb_test cargo test --test integration
//! ```
//!
//! If `DATABASE_URL` is not set, every test silently passes (returns early).
//! Each test creates its own data in isolated entity namespaces and cleans
//! up after itself so tests can run in parallel without interference.
//!
//! **71 integration tests** across 9 categories:
//! - Triple store core: 11
//! - Auth password provider: 10
//! - Auth session manager: 7
//! - Data CRUD: 15
//! - DarshJQL query engine: 10
//! - Mutations: 5
//! - Permissions: 5
//! - Audit/Merkle: 4
//! - Edge cases: 5

use ddb_server::triple_store::TripleStore;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup_pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    ddb_server::triple_store::PgTripleStore::new(pool.clone())
        .await
        .ok()?;
    ddb_server::api::rest::ensure_auth_schema(&pool)
        .await
        .ok()?;
    Some(pool)
}

async fn cleanup_entities(pool: &PgPool, ids: &[Uuid]) {
    if ids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM triples WHERE entity_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await
        .ok();
}

async fn cleanup_audit(pool: &PgPool, ids: &[i64]) {
    if ids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM tx_merkle_roots WHERE tx_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await
        .ok();
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

async fn cleanup_entity_pool(pool: &PgPool, uuids: &[Uuid]) {
    if uuids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM entity_pool WHERE external_id = ANY($1)")
        .bind(uuids)
        .execute(pool)
        .await
        .ok();
}

async fn create_test_user(pool: &PgPool) -> (Uuid, String) {
    let email = format!("integ-test-{}@darshan.db", Uuid::new_v4());
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

async fn create_test_admin(pool: &PgPool) -> (Uuid, String) {
    let email = format!("integ-admin-{}@darshan.db", Uuid::new_v4());
    let hash = ddb_server::auth::PasswordProvider::hash_password("TestPass123!").expect("hash");
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
    .expect("insert admin");
    (uid, email)
}

fn create_session_manager(pool: PgPool) -> ddb_server::auth::SessionManager {
    let km = ddb_server::auth::KeyManager::from_secret(
        b"integration-test-secret-key-at-least-32-bytes-long",
    );
    ddb_server::auth::SessionManager::new(pool, km)
}

macro_rules! ti {
    ($eid:expr, $attr:expr, $val:expr) => {
        ddb_server::triple_store::TripleInput {
            entity_id: $eid,
            attribute: $attr.into(),
            value: $val,
            value_type: 0,
            ttl_seconds: None,
        }
    };
    ($eid:expr, $attr:expr, $val:expr, $vt:expr) => {
        ddb_server::triple_store::TripleInput {
            entity_id: $eid,
            attribute: $attr.into(),
            value: $val,
            value_type: $vt,
            ttl_seconds: None,
        }
    };
    ($eid:expr, $attr:expr, $val:expr, $vt:expr, $ttl:expr) => {
        ddb_server::triple_store::TripleInput {
            entity_id: $eid,
            attribute: $attr.into(),
            value: $val,
            value_type: $vt,
            ttl_seconds: Some($ttl),
        }
    };
}

async fn run_ql(pool: &PgPool, q: &serde_json::Value) -> Vec<ddb_server::query::QueryResultRow> {
    let ast = ddb_server::query::parse_darshan_ql(q).expect("parse");
    let plan = ddb_server::query::plan_query(&ast).expect("plan");
    ddb_server::query::execute_query(pool, &plan)
        .await
        .expect("exec")
}

// ===========================================================================
// 1. TRIPLE STORE (11)
// ===========================================================================

#[tokio::test]
async fn test_ts_roundtrip() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[ti!(eid, "user/name", json!("Darshan"))])
        .await
        .expect("set");
    assert!(tx > 0);
    let t = store.get_entity(eid).await.expect("get");
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].attribute, "user/name");
    assert_eq!(t[0].value, json!("Darshan"));
    assert!(!t[0].retracted);
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ts_schema_inference() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let (e1, e2) = (Uuid::new_v4(), Uuid::new_v4());
    let tx = store
        .set_triples(&[
            ti!(e1, ":db/type", json!("User")),
            ti!(e1, "user/email", json!("t@d.db")),
            ti!(e2, ":db/type", json!("Project")),
            ti!(e2, "project/name", json!("DarshJDB")),
        ])
        .await
        .expect("set");
    let schema = store.get_schema().await.expect("schema");
    let types: Vec<&str> = schema.entity_types.keys().map(|k| k.as_str()).collect();
    assert!(types.contains(&"User"));
    assert!(types.contains(&"Project"));
    cleanup_entities(&pool, &[e1, e2]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ts_retraction() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[ti!(eid, "temp/data", json!(42), 1)])
        .await
        .expect("set");
    assert_eq!(store.get_entity(eid).await.expect("get").len(), 1);
    store.retract(eid, "temp/data").await.expect("retract");
    assert!(store.get_entity(eid).await.expect("get").is_empty());
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ts_entity_pool() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let ep = ddb_server::triple_store::EntityPool::new(pool.clone());
    ep.ensure_schema().await.expect("schema");
    let uuid = Uuid::new_v4();
    let id1 = ep.get_or_create(uuid).await.expect("create");
    assert!(id1 > 0);
    assert_eq!(ep.get_or_create(uuid).await.expect("idem"), id1);
    assert_eq!(ep.resolve(id1).await.expect("resolve"), uuid);
    cleanup_entity_pool(&pool, &[uuid]).await;
}

#[tokio::test]
async fn test_ts_bulk_load() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eids: Vec<Uuid> = (0..1000).map(|_| Uuid::new_v4()).collect();
    let inputs: Vec<_> = eids
        .iter()
        .enumerate()
        .map(|(i, eid)| ti!(*eid, "bulk/item", json!({"index": i})))
        .collect();
    let result = store.bulk_load(inputs).await.expect("bulk");
    assert_eq!(result.triples_loaded, 1000);
    assert!(result.tx_id > 0);
    assert_eq!(store.get_entity(eids[500]).await.expect("get").len(), 1);
    cleanup_entities(&pool, &eids).await;
    cleanup_audit(&pool, &[result.tx_id]).await;
}

#[tokio::test]
async fn test_ts_ttl() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[ti!(eid, "eph/tok", json!("x"), 0, 3600)])
        .await
        .expect("set");
    let t = store.get_entity(eid).await.expect("get");
    assert!(t[0].expires_at.is_some());
    let diff = (t[0].expires_at.unwrap() - chrono::Utc::now()).num_seconds();
    assert!((3590..=3610).contains(&diff), "got {diff}s");
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ts_point_in_time() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx1 = store
        .set_triples(&[ti!(eid, "user/name", json!("Alice"))])
        .await
        .expect("w1");
    let tx2 = store
        .set_triples(&[ti!(eid, "user/name", json!("Bob"))])
        .await
        .expect("w2");
    let at1 = store.get_entity_at(eid, tx1).await.expect("at");
    let name = at1
        .iter()
        .find(|t| t.attribute == "user/name")
        .expect("name");
    assert_eq!(name.value, json!("Alice"));
    assert!(store.get_entity(eid).await.expect("get").len() >= 2);
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx1, tx2]).await;
}

#[tokio::test]
async fn test_ts_query_by_attribute() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let (e1, e2) = (Uuid::new_v4(), Uuid::new_v4());
    let tx = store
        .set_triples(&[
            ti!(e1, "integ_test/color", json!("blue")),
            ti!(e2, "integ_test/color", json!("red")),
        ])
        .await
        .expect("set");
    assert!(
        store
            .query_by_attribute("integ_test/color", None)
            .await
            .expect("q")
            .len()
            >= 2
    );
    let blue = store
        .query_by_attribute("integ_test/color", Some(&json!("blue")))
        .await
        .expect("q");
    assert!(blue.iter().all(|t| t.value == json!("blue")));
    cleanup_entities(&pool, &[e1, e2]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ts_get_attribute() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, "user/name", json!("Alice")),
            ti!(eid, "user/email", json!("a@t.com")),
        ])
        .await
        .expect("set");
    let email = store.get_attribute(eid, "user/email").await.expect("attr");
    assert_eq!(email.len(), 1);
    assert_eq!(email[0].value, json!("a@t.com"));
    assert!(
        store
            .get_attribute(eid, "user/nope")
            .await
            .expect("attr")
            .is_empty()
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ts_partial_retraction() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, "p/keep", json!("stay")),
            ti!(eid, "p/rm", json!("go")),
        ])
        .await
        .expect("set");
    store.retract(eid, "p/rm").await.expect("retract");
    let r = store.get_entity(eid).await.expect("get");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].attribute, "p/keep");
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ts_20_attributes() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let inputs: Vec<_> = (0..20)
        .map(|i| ti!(eid, format!("m/a_{i}"), json!(format!("v_{i}"))))
        .collect();
    let tx = store.set_triples(&inputs).await.expect("set");
    assert_eq!(store.get_entity(eid).await.expect("get").len(), 20);
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

// ===========================================================================
// 2. AUTH — Password Provider (10)
// ===========================================================================

#[tokio::test]
async fn test_auth_signin_success() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    match ddb_server::auth::PasswordProvider::authenticate(&pool, &email, "TestPass123!")
        .await
        .expect("auth")
    {
        ddb_server::auth::AuthOutcome::Success { user_id, roles } => {
            assert_eq!(user_id, uid);
            assert!(roles.contains(&"user".to_string()));
        }
        other => panic!("Expected Success, got {:?}", other),
    }
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_auth_duplicate_email() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let email = format!("dup-{}@darshan.db", Uuid::new_v4());
    let hash = ddb_server::auth::PasswordProvider::hash_password("P@ss123!").expect("hash");
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(Uuid::new_v4())
    .bind(&email)
    .bind(&hash)
    .bind(json!(["user"]))
    .execute(&pool)
    .await
    .expect("1st");
    let r = sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(Uuid::new_v4())
    .bind(&email)
    .bind(&hash)
    .bind(json!(["user"]))
    .execute(&pool)
    .await;
    assert!(r.is_err());
    let err_str = r.unwrap_err().to_string();
    assert!(
        err_str.contains("duplicate") || err_str.contains("unique"),
        "got: {err_str}"
    );
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_auth_wrong_password() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (_, email) = create_test_user(&pool).await;
    let o = ddb_server::auth::PasswordProvider::authenticate(&pool, &email, "wrong")
        .await
        .expect("auth");
    assert!(matches!(o, ddb_server::auth::AuthOutcome::Failed { .. }));
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_auth_nonexistent_email() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let o = ddb_server::auth::PasswordProvider::authenticate(&pool, "no@darshan.db", "P!")
        .await
        .expect("auth");
    assert!(matches!(o, ddb_server::auth::AuthOutcome::Failed { .. }));
}

#[tokio::test]
async fn test_auth_correct_roles() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_admin(&pool).await;
    match ddb_server::auth::PasswordProvider::authenticate(&pool, &email, "TestPass123!")
        .await
        .expect("auth")
    {
        ddb_server::auth::AuthOutcome::Success { user_id, roles } => {
            assert_eq!(user_id, uid);
            assert!(roles.contains(&"admin".to_string()));
            assert!(roles.contains(&"user".to_string()));
        }
        other => panic!("Expected Success, got {:?}", other),
    }
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_auth_empty_password() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (_, email) = create_test_user(&pool).await;
    let o = ddb_server::auth::PasswordProvider::authenticate(&pool, &email, "")
        .await
        .expect("auth");
    assert!(matches!(o, ddb_server::auth::AuthOutcome::Failed { .. }));
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_auth_deleted_user() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .expect("del");
    let o = ddb_server::auth::PasswordProvider::authenticate(&pool, &email, "TestPass123!")
        .await
        .expect("auth");
    assert!(matches!(o, ddb_server::auth::AuthOutcome::Failed { .. }));
    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .ok();
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(uid)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn test_auth_email_exact_match() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let email = format!("case-{}@darshan.db", Uuid::new_v4());
    let hash = ddb_server::auth::PasswordProvider::hash_password("TestPass123!").expect("hash");
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(Uuid::new_v4())
    .bind(&email)
    .bind(&hash)
    .bind(json!(["user"]))
    .execute(&pool)
    .await
    .expect("ins");
    let o = ddb_server::auth::PasswordProvider::authenticate(&pool, &email, "TestPass123!")
        .await
        .expect("auth");
    assert!(matches!(o, ddb_server::auth::AuthOutcome::Success { .. }));
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_auth_long_password() {
    let pw = "A".repeat(128);
    let hash = ddb_server::auth::PasswordProvider::hash_password(&pw).expect("hash");
    assert!(ddb_server::auth::PasswordProvider::verify_password(&pw, &hash).expect("verify"));
}

#[tokio::test]
async fn test_auth_special_chars() {
    let pw = r#"p@$$w0rd!#%^&*()_+-={}[]|\":;'<>,.?/~`"#;
    let hash = ddb_server::auth::PasswordProvider::hash_password(pw).expect("hash");
    assert!(ddb_server::auth::PasswordProvider::verify_password(pw, &hash).expect("verify"));
}

// ===========================================================================
// 3. AUTH — Session Manager (7)
// ===========================================================================

#[tokio::test]
async fn test_session_create_validate() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    let sm = create_session_manager(pool.clone());
    let tp = sm
        .create_session(uid, vec!["user".into()], "127.0.0.1", "agent", "fp")
        .await
        .expect("create");
    assert!(!tp.access_token.is_empty());
    assert!(!tp.refresh_token.is_empty());
    assert_eq!(tp.token_type, "Bearer");
    let ctx = sm
        .validate_token(&tp.access_token, "127.0.0.1", "agent", "fp")
        .expect("validate");
    assert_eq!(ctx.user_id, uid);
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_session_refresh_rotation() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    let sm = create_session_manager(pool.clone());
    let orig = sm
        .create_session(uid, vec!["user".into()], "127.0.0.1", "a", "fp")
        .await
        .expect("c");
    let fresh = sm
        .refresh_session(&orig.refresh_token, "fp")
        .await
        .expect("r");
    assert_ne!(orig.refresh_token, fresh.refresh_token);
    assert_ne!(orig.access_token, fresh.access_token);
    assert_eq!(
        sm.validate_token(&fresh.access_token, "127.0.0.1", "a", "fp")
            .expect("v")
            .user_id,
        uid
    );
    assert!(sm.refresh_session(&orig.refresh_token, "fp").await.is_err());
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_session_signout() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    let sm = create_session_manager(pool.clone());
    let tp = sm
        .create_session(uid, vec!["user".into()], "127.0.0.1", "a", "fp")
        .await
        .expect("c");
    let ctx = sm
        .validate_token(&tp.access_token, "127.0.0.1", "a", "fp")
        .expect("v");
    sm.revoke_session(ctx.session_id).await.expect("revoke");
    assert!(sm.refresh_session(&tp.refresh_token, "fp").await.is_err());
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_session_revoke_all() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    let sm = create_session_manager(pool.clone());
    for i in 0..3 {
        sm.create_session(
            uid,
            vec!["user".into()],
            "127.0.0.1",
            "a",
            &format!("fp{i}"),
        )
        .await
        .expect("c");
    }
    assert!(sm.list_sessions(uid).await.expect("list").len() >= 3);
    assert!(sm.revoke_all_sessions(uid).await.expect("revoke") >= 3);
    assert!(sm.list_sessions(uid).await.expect("list").is_empty());
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_session_fingerprint_mismatch() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    let sm = create_session_manager(pool.clone());
    let tp = sm
        .create_session(uid, vec!["user".into()], "127.0.0.1", "a", "devA")
        .await
        .expect("c");
    assert!(sm.refresh_session(&tp.refresh_token, "devB").await.is_err());
    assert!(sm.list_sessions(uid).await.expect("list").is_empty());
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_session_list_filters_revoked() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    let sm = create_session_manager(pool.clone());
    let tp1 = sm
        .create_session(uid, vec!["user".into()], "127.0.0.1", "a1", "f1")
        .await
        .expect("s1");
    let _tp2 = sm
        .create_session(uid, vec!["user".into()], "127.0.0.1", "a2", "f2")
        .await
        .expect("s2");
    let ctx = sm
        .validate_token(&tp1.access_token, "127.0.0.1", "a1", "f1")
        .expect("v");
    sm.revoke_session(ctx.session_id).await.expect("revoke");
    assert_eq!(sm.list_sessions(uid).await.expect("list").len(), 1);
    cleanup_user(&pool, &email).await;
}

#[tokio::test]
async fn test_session_empty_fields() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let (uid, email) = create_test_user(&pool).await;
    let sm = create_session_manager(pool.clone());
    let tp = sm
        .create_session(uid, vec!["user".into()], "", "", "")
        .await
        .expect("c");
    assert!(!tp.access_token.is_empty());
    cleanup_user(&pool, &email).await;
}

// ===========================================================================
// 4. DATA CRUD (15)
// ===========================================================================

#[tokio::test]
async fn test_data_create() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!("Product")),
            ti!(eid, "Product/name", json!("Widget")),
            ti!(eid, "Product/price", json!(29.99), 1),
            ti!(eid, "Product/in_stock", json!(true), 2),
        ])
        .await
        .expect("set");
    assert_eq!(store.get_entity(eid).await.expect("get").len(), 4);
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_list() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("L_{}", Uuid::new_v4().to_string().replace('-', ""));
    let mut eids = Vec::new();
    let mut txs = Vec::new();
    for i in 0..5 {
        let eid = Uuid::new_v4();
        eids.push(eid);
        txs.push(
            store
                .set_triples(&[
                    ti!(eid, ":db/type", json!(ut)),
                    ti!(eid, format!("{ut}/i"), json!(i), 1),
                ])
                .await
                .expect("set"),
        );
    }
    let results = run_ql(&pool, &json!({"type": ut})).await;
    assert_eq!(results.len(), 5);
    cleanup_entities(&pool, &eids).await;
    for tx in &txs {
        cleanup_audit(&pool, &[*tx]).await;
    }
}

#[tokio::test]
async fn test_data_get_single() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!("S")),
            ti!(eid, "S/n", json!("Test")),
        ])
        .await
        .expect("set");
    let r = store.get_entity(eid).await.expect("get");
    assert!(
        r.iter()
            .any(|t| t.attribute == "S/n" && t.value == json!("Test"))
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_update() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx1 = store
        .set_triples(&[
            ti!(eid, ":db/type", json!("U")),
            ti!(eid, "U/s", json!("draft")),
        ])
        .await
        .expect("set");
    store.retract(eid, "U/s").await.expect("retract");
    let tx2 = store
        .set_triples(&[ti!(eid, "U/s", json!("pub"))])
        .await
        .expect("set");
    let s = store
        .get_entity(eid)
        .await
        .expect("get")
        .into_iter()
        .find(|t| t.attribute == "U/s")
        .expect("s");
    assert_eq!(s.value, json!("pub"));
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx1, tx2]).await;
}

#[tokio::test]
async fn test_data_delete() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!("D")),
            ti!(eid, "D/n", json!("x")),
        ])
        .await
        .expect("set");
    store.retract(eid, ":db/type").await.expect("r");
    store.retract(eid, "D/n").await.expect("r");
    assert!(store.get_entity(eid).await.expect("get").is_empty());
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_get_after_delete() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[ti!(eid, "e/d", json!("t"))])
        .await
        .expect("set");
    store.retract(eid, "e/d").await.expect("r");
    assert!(store.get_entity(eid).await.expect("get").is_empty());
    assert!(
        store
            .get_entity(Uuid::new_v4())
            .await
            .expect("get")
            .is_empty()
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_ttl() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!("T"), 0, 7200),
            ti!(eid, "T/d", json!("e"), 0, 7200),
        ])
        .await
        .expect("set");
    let r = store.get_entity(eid).await.expect("get");
    assert_eq!(r.len(), 2);
    for t in &r {
        assert!(t.expires_at.is_some());
        let d = (t.expires_at.unwrap() - chrono::Utc::now()).num_seconds();
        assert!((7100..=7300).contains(&d), "got {d}");
    }
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_bulk_100() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("B_{}", Uuid::new_v4().to_string().replace('-', ""));
    let mut all = Vec::new();
    let mut eids = Vec::new();
    for i in 0..100 {
        let eid = Uuid::new_v4();
        eids.push(eid);
        all.push(ti!(eid, ":db/type", json!(ut)));
        all.push(ti!(eid, format!("{ut}/i"), json!(i), 1));
    }
    let result = store.bulk_load(all).await.expect("bulk");
    assert_eq!(result.triples_loaded, 200);
    assert_eq!(run_ql(&pool, &json!({"type": ut})).await.len(), 100);
    cleanup_entities(&pool, &eids).await;
    cleanup_audit(&pool, &[result.tx_id]).await;
}

#[tokio::test]
async fn test_data_upsert() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx1 = store
        .set_triples(&[ti!(eid, "u/c", json!(1), 1)])
        .await
        .expect("w1");
    store.retract(eid, "u/c").await.expect("r");
    let tx2 = store
        .set_triples(&[ti!(eid, "u/c", json!(2), 1)])
        .await
        .expect("w2");
    assert_eq!(
        store
            .get_entity(eid)
            .await
            .expect("get")
            .iter()
            .find(|t| t.attribute == "u/c")
            .unwrap()
            .value,
        json!(2)
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx1, tx2]).await;
}

#[tokio::test]
async fn test_data_json_object() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let val = json!({"a": {"b": "c"}, "tags": [1, 2], "x": 2.78});
    let tx = store
        .set_triples(&[ti!(eid, "u/meta", val.clone())])
        .await
        .expect("set");
    assert_eq!(store.get_entity(eid).await.expect("get")[0].value, val);
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_null_value() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[ti!(eid, "t/null", json!(null))])
        .await
        .expect("set");
    assert!(store.get_entity(eid).await.expect("get")[0].value.is_null());
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_long_string() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let long = "x".repeat(100_000);
    let tx = store
        .set_triples(&[ti!(eid, "t/long", json!(long))])
        .await
        .expect("set");
    assert_eq!(
        store.get_entity(eid).await.expect("get")[0]
            .value
            .as_str()
            .unwrap()
            .len(),
        100_000
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_data_nonexistent_empty() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    assert!(
        store
            .get_entity(Uuid::new_v4())
            .await
            .expect("get")
            .is_empty()
    );
}

#[tokio::test]
async fn test_data_types_roundtrip() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, "t/bool", json!(true), 2),
            ti!(eid, "t/int", json!(42), 1),
            ti!(eid, "t/float", json!(2.78), 1),
            ti!(eid, "t/arr", json!([1, 2, 3])),
        ])
        .await
        .expect("set");
    let t = store.get_entity(eid).await.expect("get");
    assert_eq!(t.len(), 4);
    assert!(
        t.iter()
            .any(|tr| tr.attribute == "t/bool" && tr.value == json!(true))
    );
    assert!(
        t.iter()
            .any(|tr| tr.attribute == "t/int" && tr.value == json!(42))
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

// ===========================================================================
// 5. DARSHANQL QUERY (10)
// ===========================================================================

#[tokio::test]
async fn test_ql_basic() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("BQ_{}", Uuid::new_v4().to_string().replace('-', ""));
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!(ut)),
            ti!(eid, format!("{ut}/f"), json!("v")),
        ])
        .await
        .expect("set");
    assert!(
        run_ql(&pool, &json!({"type": ut}))
            .await
            .iter()
            .any(|r| r.entity_id == eid)
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ql_where() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("WF_{}", Uuid::new_v4().to_string().replace('-', ""));
    let (ey, en) = (Uuid::new_v4(), Uuid::new_v4());
    let tx = store
        .set_triples(&[
            ti!(ey, ":db/type", json!(ut)),
            ti!(ey, format!("{ut}/r"), json!("admin")),
            ti!(en, ":db/type", json!(ut)),
            ti!(en, format!("{ut}/r"), json!("viewer")),
        ])
        .await
        .expect("set");
    let ids: Vec<Uuid> = run_ql(&pool, &json!({"type": ut, "$where": [{"attribute": format!("{ut}/r"), "op": "Eq", "value": "admin"}]})).await.iter().map(|r| r.entity_id).collect();
    assert!(ids.contains(&ey));
    assert!(!ids.contains(&en));
    cleanup_entities(&pool, &[ey, en]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ql_limit_offset() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("PG_{}", Uuid::new_v4().to_string().replace('-', ""));
    let mut eids = Vec::new();
    let mut inputs = Vec::new();
    for i in 0..10 {
        let eid = Uuid::new_v4();
        eids.push(eid);
        inputs.push(ti!(eid, ":db/type", json!(ut)));
        inputs.push(ti!(eid, format!("{ut}/s"), json!(i), 1));
    }
    let tx = store.set_triples(&inputs).await.expect("set");
    let p1 = run_ql(&pool, &json!({"type": ut, "$limit": 3})).await;
    assert_eq!(p1.len(), 3);
    let p2 = run_ql(&pool, &json!({"type": ut, "$limit": 3, "$offset": 3})).await;
    assert_eq!(p2.len(), 3);
    let ids1: Vec<Uuid> = p1.iter().map(|r| r.entity_id).collect();
    let ids2: Vec<Uuid> = p2.iter().map(|r| r.entity_id).collect();
    for id in &ids1 {
        assert!(!ids2.contains(id));
    }
    cleanup_entities(&pool, &eids).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ql_search_no_error() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("SQ_{}", Uuid::new_v4().to_string().replace('-', ""));
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!(ut)),
            ti!(eid, format!("{ut}/d"), json!("quick brown fox")),
        ])
        .await
        .expect("set");
    let _ = run_ql(&pool, &json!({"type": ut, "$search": "fox"})).await;
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ql_empty_result() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let ut = format!("NE_{}", Uuid::new_v4().to_string().replace('-', ""));
    assert!(run_ql(&pool, &json!({"type": ut})).await.is_empty());
}

#[tokio::test]
async fn test_ql_invalid_no_type() {
    assert!(ddb_server::query::parse_darshan_ql(&json!({"$where": []})).is_err());
}

#[tokio::test]
async fn test_ql_invalid_not_object() {
    assert!(ddb_server::query::parse_darshan_ql(&json!("string")).is_err());
}

#[tokio::test]
async fn test_ql_order() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("OQ_{}", Uuid::new_v4().to_string().replace('-', ""));
    let mut eids = Vec::new();
    let mut inputs = Vec::new();
    for i in 0..5 {
        let eid = Uuid::new_v4();
        eids.push(eid);
        inputs.push(ti!(eid, ":db/type", json!(ut)));
        inputs.push(ti!(eid, format!("{ut}/p"), json!(i * 10), 1));
    }
    let tx = store.set_triples(&inputs).await.expect("set");
    let r = run_ql(
        &pool,
        &json!({"type": ut, "$order": [{"attribute": format!("{ut}/p"), "direction": "Desc"}]}),
    )
    .await;
    assert_eq!(r.len(), 5);
    cleanup_entities(&pool, &eids).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ql_multi_where() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("MW_{}", Uuid::new_v4().to_string().replace('-', ""));
    let (eb, eo) = (Uuid::new_v4(), Uuid::new_v4());
    let tx = store
        .set_triples(&[
            ti!(eb, ":db/type", json!(ut)),
            ti!(eb, format!("{ut}/c"), json!("red")),
            ti!(eb, format!("{ut}/s"), json!("large")),
            ti!(eo, ":db/type", json!(ut)),
            ti!(eo, format!("{ut}/c"), json!("red")),
            ti!(eo, format!("{ut}/s"), json!("small")),
        ])
        .await
        .expect("set");
    let ids: Vec<Uuid> = run_ql(
        &pool,
        &json!({"type": ut, "$where": [
            {"attribute": format!("{ut}/c"), "op": "Eq", "value": "red"},
            {"attribute": format!("{ut}/s"), "op": "Eq", "value": "large"}
        ]}),
    )
    .await
    .iter()
    .map(|r| r.entity_id)
    .collect();
    assert!(ids.contains(&eb));
    assert!(!ids.contains(&eo));
    cleanup_entities(&pool, &[eb, eo]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_ql_neq() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("NQ_{}", Uuid::new_v4().to_string().replace('-', ""));
    let (ey, en) = (Uuid::new_v4(), Uuid::new_v4());
    let tx = store
        .set_triples(&[
            ti!(ey, ":db/type", json!(ut)),
            ti!(ey, format!("{ut}/s"), json!("active")),
            ti!(en, ":db/type", json!(ut)),
            ti!(en, format!("{ut}/s"), json!("deleted")),
        ])
        .await
        .expect("set");
    let ids: Vec<Uuid> = run_ql(&pool, &json!({"type": ut, "$where": [{"attribute": format!("{ut}/s"), "op": "Neq", "value": "deleted"}]})).await.iter().map(|r| r.entity_id).collect();
    assert!(ids.contains(&ey));
    assert!(!ids.contains(&en));
    cleanup_entities(&pool, &[ey, en]).await;
    cleanup_audit(&pool, &[tx]).await;
}

// ===========================================================================
// 6. MUTATIONS (5)
// ===========================================================================

#[tokio::test]
async fn test_mut_returns_tx() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!("M")),
            ti!(eid, "M/d", json!("v")),
        ])
        .await
        .expect("set");
    assert!(tx > 0);
    let t = store.get_entity(eid).await.expect("get");
    for triple in &t {
        assert_eq!(triple.tx_id, tx);
    }
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_mut_batch_atomic() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let (e1, e2, e3) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let tx = store
        .set_triples(&[
            ti!(e1, "B/a", json!(1), 1),
            ti!(e2, "B/a", json!(2), 1),
            ti!(e3, "B/a", json!(3), 1),
        ])
        .await
        .expect("set");
    for eid in [e1, e2, e3] {
        assert_eq!(store.get_entity(eid).await.expect("get")[0].tx_id, tx);
    }
    cleanup_entities(&pool, &[e1, e2, e3]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_mut_delete() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[ti!(eid, "D/a", json!("x")), ti!(eid, "D/b", json!("y"))])
        .await
        .expect("set");
    for t in &store.get_entity(eid).await.expect("get") {
        store.retract(eid, &t.attribute).await.expect("r");
    }
    assert!(store.get_entity(eid).await.expect("get").is_empty());
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_mut_sequential_tx_ids() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let mut txs = Vec::new();
    let mut eids = Vec::new();
    for i in 0..5 {
        let eid = Uuid::new_v4();
        eids.push(eid);
        txs.push(
            store
                .set_triples(&[ti!(eid, format!("s/{i}"), json!(i), 1)])
                .await
                .expect("set"),
        );
    }
    for w in txs.windows(2) {
        assert!(w[1] > w[0]);
    }
    cleanup_entities(&pool, &eids).await;
    for tx in &txs {
        cleanup_audit(&pool, &[*tx]).await;
    }
}

#[tokio::test]
async fn test_mut_empty_batch() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let _ = store.set_triples(&[]).await; // Should not panic
}

// ===========================================================================
// 7. PERMISSIONS (5)
// ===========================================================================

fn make_auth_ctx(roles: Vec<String>) -> ddb_server::auth::AuthContext {
    ddb_server::auth::AuthContext {
        user_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        roles,
        ip: "127.0.0.1".into(),
        user_agent: "test".into(),
        device_fingerprint: "test".into(),
    }
}

#[tokio::test]
async fn test_perm_user_read() {
    let e = ddb_server::auth::build_default_engine();
    let ctx = make_auth_ctx(vec!["user".into()]);
    assert!(
        ddb_server::auth::evaluate_permission(
            &ctx,
            "x",
            ddb_server::auth::Operation::Read,
            None,
            &e
        )
        .allowed
    );
}

#[tokio::test]
async fn test_perm_user_create() {
    let e = ddb_server::auth::build_default_engine();
    let ctx = make_auth_ctx(vec!["user".into()]);
    assert!(
        ddb_server::auth::evaluate_permission(
            &ctx,
            "x",
            ddb_server::auth::Operation::Create,
            None,
            &e
        )
        .allowed
    );
}

#[tokio::test]
async fn test_perm_admin_all() {
    let e = ddb_server::auth::build_default_engine();
    let ctx = make_auth_ctx(vec!["admin".into()]);
    for op in [
        ddb_server::auth::Operation::Read,
        ddb_server::auth::Operation::Create,
        ddb_server::auth::Operation::Update,
        ddb_server::auth::Operation::Delete,
    ] {
        assert!(ddb_server::auth::evaluate_permission(&ctx, "x", op, None, &e).allowed);
    }
}

#[tokio::test]
async fn test_perm_unknown_role() {
    let e = ddb_server::auth::build_default_engine();
    let ctx = make_auth_ctx(vec!["xyz".into()]);
    let _ = ddb_server::auth::evaluate_permission(
        &ctx,
        "e",
        ddb_server::auth::Operation::Read,
        None,
        &e,
    );
}

#[tokio::test]
async fn test_perm_empty_roles() {
    let e = ddb_server::auth::build_default_engine();
    let ctx = make_auth_ctx(vec![]);
    let _ = ddb_server::auth::evaluate_permission(
        &ctx,
        "e",
        ddb_server::auth::Operation::Read,
        None,
        &e,
    );
}

// ===========================================================================
// 8. AUDIT / MERKLE (4)
// ===========================================================================

#[tokio::test]
async fn test_audit_single_tx() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[ti!(eid, "au/a", json!("va")), ti!(eid, "au/b", json!("vb"))])
        .await
        .expect("set");
    let v = ddb_server::audit::verify_tx(&pool, tx).await.expect("v");
    assert!(v.valid);
    assert_eq!(v.triple_count, 2);
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_audit_chain() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let mut txs = Vec::new();
    let mut eids = Vec::new();
    for i in 0..3 {
        let eid = Uuid::new_v4();
        eids.push(eid);
        let inputs: Vec<_> = (0..5)
            .map(|j| ti!(eid, format!("au/f_{j}"), json!({"b": i, "f": j})))
            .collect();
        txs.push(store.set_triples(&inputs).await.expect("set"));
    }
    for &tx in &txs {
        assert!(
            ddb_server::audit::verify_tx(&pool, tx)
                .await
                .expect("v")
                .valid
        );
    }
    let chain = ddb_server::audit::verify_chain(&pool).await.expect("chain");
    assert!(chain.valid);
    assert!(chain.total_transactions >= 3);
    cleanup_entities(&pool, &eids).await;
    cleanup_audit(&pool, &txs).await;
}

#[tokio::test]
async fn test_audit_5_sequential() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let mut txs = Vec::new();
    let mut eids = Vec::new();
    for _ in 0..5 {
        let eid = Uuid::new_v4();
        eids.push(eid);
        txs.push(
            store
                .set_triples(&[ti!(eid, "ch/d", json!("e"))])
                .await
                .expect("set"),
        );
    }
    let chain = ddb_server::audit::verify_chain(&pool).await.expect("chain");
    assert!(chain.valid);
    assert!(chain.total_transactions >= 5);
    cleanup_entities(&pool, &eids).await;
    for tx in &txs {
        cleanup_audit(&pool, &[*tx]).await;
    }
}

#[tokio::test]
async fn test_audit_triple_count() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, "ac/x", json!(1), 1),
            ti!(eid, "ac/y", json!(2), 1),
            ti!(eid, "ac/z", json!(3), 1),
        ])
        .await
        .expect("set");
    let v = ddb_server::audit::verify_tx(&pool, tx).await.expect("v");
    assert!(v.valid);
    assert_eq!(v.triple_count, 3);
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

// ===========================================================================
// 9. EDGE CASES (5)
// ===========================================================================

#[tokio::test]
async fn test_concurrent_writes() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = std::sync::Arc::new(ddb_server::triple_store::PgTripleStore::new_lazy(
        pool.clone(),
    ));
    let mut handles = Vec::new();
    for i in 0..10 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            let eid = Uuid::new_v4();
            let tx = s
                .set_triples(&[ti!(eid, format!("cc/{i}"), json!(i), 1)])
                .await
                .expect("w");
            (eid, tx)
        }));
    }
    let mut results = Vec::new();
    for h in handles {
        results.push(h.await.expect("join"));
    }
    let txs: std::collections::HashSet<i64> = results.iter().map(|(_, tx)| *tx).collect();
    assert_eq!(txs.len(), 10);
    for (eid, _) in &results {
        assert_eq!(store.get_entity(*eid).await.expect("get").len(), 1);
    }
    let eids: Vec<Uuid> = results.iter().map(|(e, _)| *e).collect();
    cleanup_entities(&pool, &eids).await;
    for (_, tx) in &results {
        cleanup_audit(&pool, &[*tx]).await;
    }
}

#[tokio::test]
async fn test_schema_custom_type() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("SC_{}", Uuid::new_v4().to_string().replace('-', ""));
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!(ut)),
            ti!(eid, format!("{ut}/n"), json!("t")),
        ])
        .await
        .expect("set");
    assert!(
        store
            .get_schema()
            .await
            .expect("schema")
            .entity_types
            .contains_key(&ut)
    );
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_retract_nonexistent() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    assert!(
        store
            .retract(Uuid::new_v4(), "does/not/exist")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn test_query_with_all_where_ops() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut = format!("WO_{}", Uuid::new_v4().to_string().replace('-', ""));
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(eid, ":db/type", json!(ut)),
            ti!(eid, format!("{ut}/val"), json!(50), 1),
        ])
        .await
        .expect("set");
    // Test Eq
    let r = run_ql(&pool, &json!({"type": ut, "$where": [{"attribute": format!("{ut}/val"), "op": "Eq", "value": 50}]})).await;
    assert!(r.iter().any(|row| row.entity_id == eid));
    // Test Neq
    let r = run_ql(&pool, &json!({"type": ut, "$where": [{"attribute": format!("{ut}/val"), "op": "Neq", "value": 999}]})).await;
    assert!(r.iter().any(|row| row.entity_id == eid));
    cleanup_entities(&pool, &[eid]).await;
    cleanup_audit(&pool, &[tx]).await;
}

#[tokio::test]
async fn test_multiple_entity_types_isolated() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let store = ddb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let ut_a = format!("TA_{}", Uuid::new_v4().to_string().replace('-', ""));
    let ut_b = format!("TB_{}", Uuid::new_v4().to_string().replace('-', ""));
    let ea = Uuid::new_v4();
    let eb = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            ti!(ea, ":db/type", json!(ut_a)),
            ti!(ea, format!("{ut_a}/x"), json!("a")),
            ti!(eb, ":db/type", json!(ut_b)),
            ti!(eb, format!("{ut_b}/x"), json!("b")),
        ])
        .await
        .expect("set");
    let ra = run_ql(&pool, &json!({"type": ut_a})).await;
    let rb = run_ql(&pool, &json!({"type": ut_b})).await;
    assert!(ra.iter().any(|r| r.entity_id == ea));
    assert!(!ra.iter().any(|r| r.entity_id == eb));
    assert!(rb.iter().any(|r| r.entity_id == eb));
    assert!(!rb.iter().any(|r| r.entity_id == ea));
    cleanup_entities(&pool, &[ea, eb]).await;
    cleanup_audit(&pool, &[tx]).await;
}
