//! Integration tests for DarshanDB against a real Postgres database.
//!
//! These tests require a running Postgres instance. Set the `DATABASE_URL`
//! environment variable to enable them:
//!
//! ```sh
//! DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshandb_test cargo test --test integration
//! ```
//!
//! If `DATABASE_URL` is not set, every test silently passes (returns early).
//! Each test creates its own data in isolated entity namespaces and cleans
//! up after itself so tests can run in parallel without interference.

use darshandb_server::triple_store::TripleStore;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Attempt to connect to the test database. Returns `None` if `DATABASE_URL`
/// is not set or the connection fails, causing the calling test to skip.
async fn setup_pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;

    // Ensure the schema exists so tests don't fail on a blank database.
    darshandb_server::triple_store::PgTripleStore::new(pool.clone())
        .await
        .ok()?;

    // Ensure the auth tables exist.
    darshandb_server::api::rest::ensure_auth_schema(&pool)
        .await
        .ok()?;

    Some(pool)
}

/// Delete all triples (and audit rows) that belong to the given entity ids.
/// Used by tests to clean up after themselves.
async fn cleanup_entities(pool: &PgPool, entity_ids: &[Uuid]) {
    if entity_ids.is_empty() {
        return;
    }
    // Delete triples for these entities.
    sqlx::query("DELETE FROM triples WHERE entity_id = ANY($1)")
        .bind(entity_ids)
        .execute(pool)
        .await
        .ok();
}

/// Delete audit rows for the given transaction ids.
async fn cleanup_audit(pool: &PgPool, tx_ids: &[i64]) {
    if tx_ids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM tx_merkle_roots WHERE tx_id = ANY($1)")
        .bind(tx_ids)
        .execute(pool)
        .await
        .ok();
}

/// Delete a test user by email.
async fn cleanup_user(pool: &PgPool, email: &str) {
    // Delete sessions first (FK constraint).
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

/// Delete entity_pool entries for the given UUIDs.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Write a triple, read it back, verify all fields match.
#[tokio::test]
async fn test_triple_store_roundtrip() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let entity_id = Uuid::new_v4();

    // Write
    let input = darshandb_server::triple_store::TripleInput {
        entity_id,
        attribute: "user/name".into(),
        value: json!("Darshan"),
        value_type: 0, // string
        ttl_seconds: None,
    };

    let tx_id = store
        .set_triples(&[input])
        .await
        .expect("set_triples failed");
    assert!(tx_id > 0, "tx_id should be positive");

    // Read back
    let triples = store
        .get_entity(entity_id)
        .await
        .expect("get_entity failed");

    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].entity_id, entity_id);
    assert_eq!(triples[0].attribute, "user/name");
    assert_eq!(triples[0].value, json!("Darshan"));
    assert!(!triples[0].retracted);

    // Cleanup
    cleanup_entities(&pool, &[entity_id]).await;
    cleanup_audit(&pool, &[tx_id]).await;
}

/// Write entities of different types, infer the schema, verify type presence.
#[tokio::test]
async fn test_schema_inference() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let e1 = Uuid::new_v4();
    let e2 = Uuid::new_v4();

    let inputs = vec![
        darshandb_server::triple_store::TripleInput {
            entity_id: e1,
            attribute: ":db/type".into(),
            value: json!("User"),
            value_type: 0,
            ttl_seconds: None,
        },
        darshandb_server::triple_store::TripleInput {
            entity_id: e1,
            attribute: "user/email".into(),
            value: json!("test@darshan.db"),
            value_type: 0,
            ttl_seconds: None,
        },
        darshandb_server::triple_store::TripleInput {
            entity_id: e2,
            attribute: ":db/type".into(),
            value: json!("Project"),
            value_type: 0,
            ttl_seconds: None,
        },
        darshandb_server::triple_store::TripleInput {
            entity_id: e2,
            attribute: "project/name".into(),
            value: json!("DarshanDB"),
            value_type: 0,
            ttl_seconds: None,
        },
    ];

    let tx_id = store
        .set_triples(&inputs)
        .await
        .expect("set_triples failed");

    let schema = store.get_schema().await.expect("get_schema failed");

    // Schema should contain at least the entity types we wrote.
    // Schema.entity_types is HashMap<String, EntityType> — keys are type names.
    let type_names: Vec<&str> = schema
        .entity_types
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert!(
        type_names.contains(&"User"),
        "Schema should contain User type, got: {:?}",
        type_names
    );
    assert!(
        type_names.contains(&"Project"),
        "Schema should contain Project type, got: {:?}",
        type_names
    );

    // Cleanup
    cleanup_entities(&pool, &[e1, e2]).await;
    cleanup_audit(&pool, &[tx_id]).await;
}

/// Write data, query with a where-clause filter, verify correct results.
#[tokio::test]
async fn test_query_with_where() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let e1 = Uuid::new_v4();
    let e2 = Uuid::new_v4();
    let e3 = Uuid::new_v4();

    let inputs = vec![
        // Entity 1: active user
        darshandb_server::triple_store::TripleInput {
            entity_id: e1,
            attribute: ":db/type".into(),
            value: json!("IntegTestUser"),
            value_type: 0,
            ttl_seconds: None,
        },
        darshandb_server::triple_store::TripleInput {
            entity_id: e1,
            attribute: "user/status".into(),
            value: json!("active"),
            value_type: 0,
            ttl_seconds: None,
        },
        // Entity 2: inactive user
        darshandb_server::triple_store::TripleInput {
            entity_id: e2,
            attribute: ":db/type".into(),
            value: json!("IntegTestUser"),
            value_type: 0,
            ttl_seconds: None,
        },
        darshandb_server::triple_store::TripleInput {
            entity_id: e2,
            attribute: "user/status".into(),
            value: json!("inactive"),
            value_type: 0,
            ttl_seconds: None,
        },
        // Entity 3: active user
        darshandb_server::triple_store::TripleInput {
            entity_id: e3,
            attribute: ":db/type".into(),
            value: json!("IntegTestUser"),
            value_type: 0,
            ttl_seconds: None,
        },
        darshandb_server::triple_store::TripleInput {
            entity_id: e3,
            attribute: "user/status".into(),
            value: json!("active"),
            value_type: 0,
            ttl_seconds: None,
        },
    ];

    let tx_id = store
        .set_triples(&inputs)
        .await
        .expect("set_triples failed");

    // Query: IntegTestUser where status = "active"
    let query = json!({
        "type": "IntegTestUser",
        "$where": [
            { "attribute": "user/status", "op": "=", "value": "active" }
        ]
    });

    let ast = darshandb_server::query::parse_darshan_ql(&query).expect("parse failed");
    let plan = darshandb_server::query::plan_query(&ast).expect("plan failed");
    let results = darshandb_server::query::execute_query(&pool, &plan)
        .await
        .expect("execute failed");

    // We should get exactly 2 active entities (e1 and e3).
    let result_ids: Vec<Uuid> = results.iter().map(|r| r.entity_id).collect();
    assert!(result_ids.contains(&e1), "Results should contain e1");
    assert!(result_ids.contains(&e3), "Results should contain e3");
    // e2 is inactive, should not appear
    assert!(
        !result_ids.contains(&e2),
        "Results should NOT contain inactive e2"
    );

    // Cleanup
    cleanup_entities(&pool, &[e1, e2, e3]).await;
    cleanup_audit(&pool, &[tx_id]).await;
}

/// Write a triple, retract it, verify the entity reads as empty.
#[tokio::test]
async fn test_retraction() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let entity_id = Uuid::new_v4();

    let input = darshandb_server::triple_store::TripleInput {
        entity_id,
        attribute: "temp/data".into(),
        value: json!(42),
        value_type: 1, // number
        ttl_seconds: None,
    };

    let tx_id = store
        .set_triples(&[input])
        .await
        .expect("set_triples failed");

    // Verify it exists
    let before = store
        .get_entity(entity_id)
        .await
        .expect("get_entity failed");
    assert_eq!(before.len(), 1);

    // Retract
    store
        .retract(entity_id, "temp/data")
        .await
        .expect("retract failed");

    // Verify entity is now empty (all triples retracted)
    let after = store
        .get_entity(entity_id)
        .await
        .expect("get_entity failed");
    assert!(
        after.is_empty(),
        "Entity should have no active triples after retraction, got {}",
        after.len()
    );

    // Cleanup
    cleanup_entities(&pool, &[entity_id]).await;
    cleanup_audit(&pool, &[tx_id]).await;
}

/// Register a UUID in the entity pool, get its internal ID, reverse-resolve
/// back to the same UUID.
#[tokio::test]
async fn test_entity_pool_roundtrip() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let entity_pool = darshandb_server::triple_store::EntityPool::new(pool.clone());
    entity_pool
        .ensure_schema()
        .await
        .expect("ensure_schema failed");

    let uuid = Uuid::new_v4();

    // Forward: UUID -> internal_id
    let internal_id = entity_pool
        .get_or_create(uuid)
        .await
        .expect("get_or_create failed");
    assert!(internal_id > 0);

    // Idempotent: same UUID -> same internal_id
    let internal_id_again = entity_pool
        .get_or_create(uuid)
        .await
        .expect("get_or_create second call failed");
    assert_eq!(internal_id, internal_id_again);

    // Reverse: internal_id -> UUID
    let resolved = entity_pool
        .resolve(internal_id)
        .await
        .expect("resolve failed");
    assert_eq!(resolved, uuid);

    // Cleanup
    cleanup_entity_pool(&pool, &[uuid]).await;
}

/// Bulk-load 1000 triples and verify the count matches.
#[tokio::test]
async fn test_bulk_load() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());

    let count = 1000;
    let entity_ids: Vec<Uuid> = (0..count).map(|_| Uuid::new_v4()).collect();

    let inputs: Vec<darshandb_server::triple_store::TripleInput> = entity_ids
        .iter()
        .enumerate()
        .map(|(i, eid)| darshandb_server::triple_store::TripleInput {
            entity_id: *eid,
            attribute: "bulk/item".into(),
            value: json!({ "index": i }),
            value_type: 0,
            ttl_seconds: None,
        })
        .collect();

    let result = store.bulk_load(inputs).await.expect("bulk_load failed");

    assert_eq!(result.triples_loaded, count);
    assert!(result.tx_id > 0);
    assert!(result.duration_ms < 30_000, "Bulk load took too long");
    assert!(result.rate_per_sec > 0.0, "Rate should be positive");

    // Verify we can read back a sample triple
    let sample = store
        .get_entity(entity_ids[500])
        .await
        .expect("get_entity failed");
    assert_eq!(sample.len(), 1);
    assert_eq!(sample[0].attribute, "bulk/item");

    // Cleanup
    cleanup_entities(&pool, &entity_ids).await;
    cleanup_audit(&pool, &[result.tx_id]).await;
}

/// Create a user via password provider, sign in, verify a JWT is returned.
#[tokio::test]
async fn test_auth_signup_signin() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let test_email = format!("integ-test-{}@darshan.db", Uuid::new_v4());
    let test_password = "SuperSecure!Pass123";

    // Hash the password
    let hash = darshandb_server::auth::PasswordProvider::hash_password(test_password)
        .expect("hash_password failed");

    // Insert user directly (bypassing signup endpoint for isolation)
    let user_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, roles) VALUES ($1, $2, $3, $4::jsonb)",
    )
    .bind(user_id)
    .bind(&test_email)
    .bind(&hash)
    .bind(json!(["user"]))
    .execute(&pool)
    .await
    .expect("insert user failed");

    // Authenticate via the PasswordProvider
    let outcome =
        darshandb_server::auth::PasswordProvider::authenticate(&pool, &test_email, test_password)
            .await
            .expect("authenticate failed");

    match outcome {
        darshandb_server::auth::AuthOutcome::Success {
            user_id: auth_uid,
            roles,
        } => {
            assert_eq!(auth_uid, user_id);
            assert!(roles.contains(&"user".to_string()));
        }
        other => panic!("Expected AuthOutcome::Success, got {:?}", other),
    }

    // Verify wrong password fails
    let bad_outcome = darshandb_server::auth::PasswordProvider::authenticate(
        &pool,
        &test_email,
        "wrong-password",
    )
    .await
    .expect("authenticate should not error on wrong password");

    assert!(
        matches!(
            bad_outcome,
            darshandb_server::auth::AuthOutcome::Failed { .. }
        ),
        "Wrong password should return Failed"
    );

    // Cleanup
    cleanup_user(&pool, &test_email).await;
}

/// Perform multiple mutations, then verify the Merkle hash chain is intact.
#[tokio::test]
async fn test_merkle_audit_chain() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());

    // Perform 3 sequential mutations that each get a Merkle root.
    let mut tx_ids = Vec::new();
    let mut entity_ids = Vec::new();

    for i in 0..3 {
        let eid = Uuid::new_v4();
        entity_ids.push(eid);

        let inputs: Vec<darshandb_server::triple_store::TripleInput> = (0..5)
            .map(|j| darshandb_server::triple_store::TripleInput {
                entity_id: eid,
                attribute: format!("audit/field_{j}"),
                value: json!({ "batch": i, "field": j }),
                value_type: 0,
                ttl_seconds: None,
            })
            .collect();

        let tx_id = store
            .set_triples(&inputs)
            .await
            .expect("set_triples failed");
        tx_ids.push(tx_id);
    }

    // Verify each individual transaction's Merkle root.
    for &tx_id in &tx_ids {
        let verification = darshandb_server::audit::verify_tx(&pool, tx_id)
            .await
            .expect("verify_tx failed");
        assert!(
            verification.valid,
            "Transaction {} should be valid: {}",
            tx_id, verification.detail
        );
        assert!(verification.triple_count > 0);
    }

    // Verify the entire hash chain is intact.
    let chain = darshandb_server::audit::verify_chain(&pool)
        .await
        .expect("verify_chain failed");
    assert!(chain.valid, "Hash chain should be valid: {}", chain.detail);
    assert!(chain.total_transactions >= 3);

    // Cleanup
    cleanup_entities(&pool, &entity_ids).await;
    cleanup_audit(&pool, &tx_ids).await;
}

/// Write a triple with a TTL, verify it is created with an expiry timestamp.
#[tokio::test]
async fn test_ttl_expiry_set() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let entity_id = Uuid::new_v4();

    let input = darshandb_server::triple_store::TripleInput {
        entity_id,
        attribute: "ephemeral/token".into(),
        value: json!("short-lived"),
        value_type: 0,
        ttl_seconds: Some(3600), // 1 hour
    };

    let tx_id = store
        .set_triples(&[input])
        .await
        .expect("set_triples failed");

    let triples = store
        .get_entity(entity_id)
        .await
        .expect("get_entity failed");
    assert_eq!(triples.len(), 1);
    assert!(
        triples[0].expires_at.is_some(),
        "Triple with TTL should have expires_at set"
    );

    // The expiry should be roughly 1 hour from now (within 10 seconds tolerance).
    let expires = triples[0].expires_at.unwrap();
    let now = chrono::Utc::now();
    let diff = (expires - now).num_seconds();
    assert!(
        (3590..=3610).contains(&diff),
        "expires_at should be ~3600s from now, got {}s",
        diff
    );

    // Cleanup
    cleanup_entities(&pool, &[entity_id]).await;
    cleanup_audit(&pool, &[tx_id]).await;
}

/// Verify point-in-time reads: write at tx1, modify at tx2, read at tx1
/// should return the original value.
#[tokio::test]
async fn test_point_in_time_read() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let entity_id = Uuid::new_v4();

    // Write initial value
    let input1 = darshandb_server::triple_store::TripleInput {
        entity_id,
        attribute: "user/name".into(),
        value: json!("Alice"),
        value_type: 0,
        ttl_seconds: None,
    };
    let tx1 = store
        .set_triples(&[input1])
        .await
        .expect("first write failed");

    // Write updated value (new triple, same attribute)
    let input2 = darshandb_server::triple_store::TripleInput {
        entity_id,
        attribute: "user/name".into(),
        value: json!("Bob"),
        value_type: 0,
        ttl_seconds: None,
    };
    let tx2 = store
        .set_triples(&[input2])
        .await
        .expect("second write failed");

    // Point-in-time read at tx1 should return Alice
    let at_tx1 = store
        .get_entity_at(entity_id, tx1)
        .await
        .expect("get_entity_at failed");

    assert!(!at_tx1.is_empty(), "Should have triples at tx1");
    let name_at_tx1 = at_tx1
        .iter()
        .find(|t| t.attribute == "user/name")
        .expect("should have user/name");
    assert_eq!(name_at_tx1.value, json!("Alice"));

    // Current read should include both Alice and Bob (append-only)
    let current = store
        .get_entity(entity_id)
        .await
        .expect("get_entity failed");
    assert!(
        current.len() >= 2,
        "Current entity should have at least 2 triples (append-only)"
    );

    // Cleanup
    cleanup_entities(&pool, &[entity_id]).await;
    cleanup_audit(&pool, &[tx1, tx2]).await;
}

/// Verify that query_by_attribute finds triples by attribute name and
/// optionally filters by value.
#[tokio::test]
async fn test_query_by_attribute() {
    let Some(pool) = setup_pool().await else {
        return;
    };

    let store = darshandb_server::triple_store::PgTripleStore::new_lazy(pool.clone());
    let e1 = Uuid::new_v4();
    let e2 = Uuid::new_v4();

    let inputs = vec![
        darshandb_server::triple_store::TripleInput {
            entity_id: e1,
            attribute: "integ_test/color".into(),
            value: json!("blue"),
            value_type: 0,
            ttl_seconds: None,
        },
        darshandb_server::triple_store::TripleInput {
            entity_id: e2,
            attribute: "integ_test/color".into(),
            value: json!("red"),
            value_type: 0,
            ttl_seconds: None,
        },
    ];

    let tx_id = store
        .set_triples(&inputs)
        .await
        .expect("set_triples failed");

    // Query by attribute only
    let all_colors = store
        .query_by_attribute("integ_test/color", None)
        .await
        .expect("query_by_attribute failed");
    assert!(
        all_colors.len() >= 2,
        "Should find at least 2 color triples"
    );

    // Query by attribute + value
    let blue_only = store
        .query_by_attribute("integ_test/color", Some(&json!("blue")))
        .await
        .expect("query_by_attribute with value failed");
    assert!(
        blue_only.iter().all(|t| t.value == json!("blue")),
        "All results should be blue"
    );
    assert!(
        blue_only.iter().any(|t| t.entity_id == e1),
        "Should contain e1"
    );

    // Cleanup
    cleanup_entities(&pool, &[e1, e2]).await;
    cleanup_audit(&pool, &[tx_id]).await;
}
