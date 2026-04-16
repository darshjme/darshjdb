//! Integration tests for the DarshJDB relational field system.
//!
//! Tests link creation, lookup resolution, rollup computation, and
//! cascade delete against a real Postgres triple store.
//!
//! ```sh
//! DATABASE_URL=postgres://darshan:darshan@localhost:5432/darshjdb_test \
//!     cargo test --test relations_test
//! ```

use ddb_server::relations::link::{self, LinkConfig, Relationship};
use ddb_server::relations::lookup::{self, LookupCache, LookupConfig};
use ddb_server::relations::rollup::{self, RollupConfig, RollupFn};
use ddb_server::triple_store::schema::ValueType;
use ddb_server::triple_store::{PgTripleStore, TripleInput, TripleStore};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup() -> Option<(PgPool, PgTripleStore)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    let store = PgTripleStore::new(pool.clone()).await.ok()?;
    Some((pool, store))
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

/// Create a project entity with a name.
async fn create_project(store: &PgTripleStore, name: &str) -> (Uuid, i64) {
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            TripleInput {
                entity_id: eid,
                attribute: ":db/type".into(),
                value: json!("Project"),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "project/name".into(),
                value: json!(name),
                value_type: 0,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("create project");
    (eid, tx)
}

/// Create a task entity with a title and a numeric score.
async fn create_task(store: &PgTripleStore, title: &str, score: i64) -> (Uuid, i64) {
    let eid = Uuid::new_v4();
    let tx = store
        .set_triples(&[
            TripleInput {
                entity_id: eid,
                attribute: ":db/type".into(),
                value: json!("Task"),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "task/title".into(),
                value: json!(title),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: eid,
                attribute: "task/score".into(),
                value: json!(score),
                value_type: ValueType::Integer as i16,
                ttl_seconds: None,
            },
        ])
        .await
        .expect("create task");
    (eid, tx)
}

// ===========================================================================
// 1. CREATE LINK BETWEEN RECORDS
// ===========================================================================

#[tokio::test]
async fn test_link_one_to_many() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "DarshJDB").await;
    let (task1_id, _) = create_task(&store, "Implement views", 10).await;
    let (task2_id, _) = create_task(&store, "Write tests", 20).await;

    // Create a OneToMany link: project -> tasks.
    let tx1 = link::add_link(
        &pool,
        project_id,
        task1_id,
        "project/tasks",
        Relationship::OneToMany,
        false,
        None,
    )
    .await
    .expect("add link 1");
    assert!(tx1 > 0);

    let tx2 = link::add_link(
        &pool,
        project_id,
        task2_id,
        "project/tasks",
        Relationship::OneToMany,
        false,
        None,
    )
    .await
    .expect("add link 2");
    assert!(tx2 > tx1);

    // Resolve linked entities.
    let linked = link::get_linked(&pool, project_id, "project/tasks")
        .await
        .expect("get linked");
    assert!(linked.contains(&task1_id));
    assert!(linked.contains(&task2_id));
    assert_eq!(linked.len(), 2);

    cleanup_entities(&pool, &[project_id, task1_id, task2_id]).await;
}

#[tokio::test]
async fn test_link_one_to_one_symmetric() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (user_id, _) = create_project(&store, "UserA").await;
    let (profile_id, _) = create_project(&store, "ProfileA").await;

    // OneToOne symmetric link: user <-> profile.
    link::add_link(
        &pool,
        user_id,
        profile_id,
        "user/profile",
        Relationship::OneToOne,
        true,
        Some("profile/user"),
    )
    .await
    .expect("add symmetric link");

    // Forward link.
    let forward = link::get_linked(&pool, user_id, "user/profile")
        .await
        .expect("forward");
    assert_eq!(forward, vec![profile_id]);

    // Backlink.
    let backward = link::get_linked(&pool, profile_id, "profile/user")
        .await
        .expect("backward");
    assert_eq!(backward, vec![user_id]);

    cleanup_entities(&pool, &[user_id, profile_id]).await;
}

#[tokio::test]
async fn test_link_many_to_many() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (tag1_id, _) = create_project(&store, "rust").await;
    let (tag2_id, _) = create_project(&store, "postgres").await;
    let (post_id, _) = create_task(&store, "DarshJDB launch", 100).await;

    // M:N: post <-> tags.
    link::add_link(
        &pool,
        post_id,
        tag1_id,
        "post/tags",
        Relationship::ManyToMany,
        true,
        Some("tag/posts"),
    )
    .await
    .expect("m2m link 1");

    link::add_link(
        &pool,
        post_id,
        tag2_id,
        "post/tags",
        Relationship::ManyToMany,
        true,
        Some("tag/posts"),
    )
    .await
    .expect("m2m link 2");

    let tags = link::get_linked(&pool, post_id, "post/tags")
        .await
        .expect("post tags");
    assert_eq!(tags.len(), 2);
    assert!(tags.contains(&tag1_id));
    assert!(tags.contains(&tag2_id));

    // Reverse: tag -> posts.
    let posts = link::get_linked(&pool, tag1_id, "tag/posts")
        .await
        .expect("tag posts");
    assert!(posts.contains(&post_id));

    // Clean up junction entities too.
    sqlx::query(
        "DELETE FROM triples WHERE attribute IN ('link/source', 'link/target', 'link/attribute', 'db/type') AND value_type = $1",
    )
    .bind(ValueType::Reference as i16)
    .execute(&pool)
    .await
    .ok();

    cleanup_entities(&pool, &[tag1_id, tag2_id, post_id]).await;
}

// ===========================================================================
// 2. REMOVE LINK
// ===========================================================================

#[tokio::test]
async fn test_link_remove() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Ephemeral").await;
    let (task_id, _) = create_task(&store, "Temp task", 5).await;

    link::add_link(
        &pool,
        project_id,
        task_id,
        "project/tasks",
        Relationship::OneToMany,
        false,
        None,
    )
    .await
    .expect("add");

    // Verify link exists.
    let linked = link::get_linked(&pool, project_id, "project/tasks")
        .await
        .expect("get");
    assert!(linked.contains(&task_id));

    // Remove the link.
    link::remove_link(
        &pool,
        project_id,
        task_id,
        "project/tasks",
        Relationship::OneToMany,
        false,
        None,
    )
    .await
    .expect("remove");

    // Verify link is gone.
    let linked_after = link::get_linked(&pool, project_id, "project/tasks")
        .await
        .expect("get after");
    assert!(!linked_after.contains(&task_id), "link should be removed");

    cleanup_entities(&pool, &[project_id, task_id]).await;
}

// ===========================================================================
// 3. RESOLVE LOOKUP VALUES
// ===========================================================================

#[tokio::test]
async fn test_lookup_resolve() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Alpha").await;
    let (task1_id, _) = create_task(&store, "Build API", 10).await;
    let (task2_id, _) = create_task(&store, "Write tests", 20).await;

    // Link project -> tasks.
    link::add_link(
        &pool,
        project_id,
        task1_id,
        "project/tasks",
        Relationship::OneToMany,
        false,
        None,
    )
    .await
    .expect("link 1");

    link::add_link(
        &pool,
        project_id,
        task2_id,
        "project/tasks",
        Relationship::OneToMany,
        false,
        None,
    )
    .await
    .expect("link 2");

    // Lookup: project -> tasks -> task/title.
    let config = LookupConfig {
        link_field: "project/tasks".into(),
        lookup_field: "task/title".into(),
    };

    let values = lookup::resolve_lookup(&pool, project_id, &config, None)
        .await
        .expect("resolve lookup");

    assert_eq!(values.len(), 2);
    let titles: Vec<&str> = values.iter().filter_map(|v| v.as_str()).collect();
    assert!(titles.contains(&"Build API"));
    assert!(titles.contains(&"Write tests"));

    cleanup_entities(&pool, &[project_id, task1_id, task2_id]).await;
}

#[tokio::test]
async fn test_lookup_with_cache() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Beta").await;
    let (task_id, _) = create_task(&store, "Cached task", 50).await;

    link::add_link(
        &pool,
        project_id,
        task_id,
        "project/tasks",
        Relationship::OneToMany,
        false,
        None,
    )
    .await
    .expect("link");

    let cache = LookupCache::default_ttl();
    let config = LookupConfig {
        link_field: "project/tasks".into(),
        lookup_field: "task/title".into(),
    };

    // First call populates cache.
    let v1 = lookup::resolve_lookup(&pool, project_id, &config, Some(&cache))
        .await
        .expect("resolve 1");
    assert_eq!(v1.len(), 1);
    assert_eq!(v1[0], json!("Cached task"));

    // Second call should hit cache.
    let v2 = lookup::resolve_lookup(&pool, project_id, &config, Some(&cache))
        .await
        .expect("resolve 2");
    assert_eq!(v1, v2);

    // Invalidate and verify fresh fetch.
    cache.invalidate_entity(project_id).await;
    let v3 = lookup::resolve_lookup(&pool, project_id, &config, Some(&cache))
        .await
        .expect("resolve 3");
    assert_eq!(v3.len(), 1);

    cleanup_entities(&pool, &[project_id, task_id]).await;
}

#[tokio::test]
async fn test_lookup_empty_link() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Empty").await;

    let config = LookupConfig {
        link_field: "project/tasks".into(),
        lookup_field: "task/title".into(),
    };

    let values = lookup::resolve_lookup(&pool, project_id, &config, None)
        .await
        .expect("resolve empty");
    assert!(values.is_empty());

    cleanup_entities(&pool, &[project_id]).await;
}

// ===========================================================================
// 4. COMPUTE ROLLUP (count, sum, avg)
// ===========================================================================

#[tokio::test]
async fn test_rollup_count() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Gamma").await;
    let (t1, _) = create_task(&store, "Task A", 10).await;
    let (t2, _) = create_task(&store, "Task B", 20).await;
    let (t3, _) = create_task(&store, "Task C", 30).await;

    for tid in [t1, t2, t3] {
        link::add_link(
            &pool,
            project_id,
            tid,
            "project/tasks",
            Relationship::OneToMany,
            false,
            None,
        )
        .await
        .expect("link");
    }

    // Count of linked records (regardless of field).
    let config = RollupConfig {
        link_field: "project/tasks".into(),
        rollup_field: "task/score".into(),
        function: RollupFn::CountAll,
    };
    let result = rollup::compute_rollup(&pool, project_id, &config)
        .await
        .expect("rollup count_all");
    assert_eq!(result, json!(3));

    // Count of records with task/score.
    let config = RollupConfig {
        link_field: "project/tasks".into(),
        rollup_field: "task/score".into(),
        function: RollupFn::Count,
    };
    let result = rollup::compute_rollup(&pool, project_id, &config)
        .await
        .expect("rollup count");
    assert_eq!(result, json!(3));

    cleanup_entities(&pool, &[project_id, t1, t2, t3]).await;
}

#[tokio::test]
async fn test_rollup_sum() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Delta").await;
    let (t1, _) = create_task(&store, "T1", 10).await;
    let (t2, _) = create_task(&store, "T2", 20).await;
    let (t3, _) = create_task(&store, "T3", 30).await;

    for tid in [t1, t2, t3] {
        link::add_link(
            &pool,
            project_id,
            tid,
            "project/tasks",
            Relationship::OneToMany,
            false,
            None,
        )
        .await
        .expect("link");
    }

    let config = RollupConfig {
        link_field: "project/tasks".into(),
        rollup_field: "task/score".into(),
        function: RollupFn::Sum,
    };
    let result = rollup::compute_rollup(&pool, project_id, &config)
        .await
        .expect("rollup sum");
    let sum = result.as_f64().expect("should be numeric");
    assert!((sum - 60.0).abs() < 0.001, "expected 60, got {sum}");

    cleanup_entities(&pool, &[project_id, t1, t2, t3]).await;
}

#[tokio::test]
async fn test_rollup_average() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Epsilon").await;
    let (t1, _) = create_task(&store, "T1", 10).await;
    let (t2, _) = create_task(&store, "T2", 20).await;

    for tid in [t1, t2] {
        link::add_link(
            &pool,
            project_id,
            tid,
            "project/tasks",
            Relationship::OneToMany,
            false,
            None,
        )
        .await
        .expect("link");
    }

    let config = RollupConfig {
        link_field: "project/tasks".into(),
        rollup_field: "task/score".into(),
        function: RollupFn::Average,
    };
    let result = rollup::compute_rollup(&pool, project_id, &config)
        .await
        .expect("rollup avg");
    let avg = result.as_f64().expect("should be numeric");
    assert!((avg - 15.0).abs() < 0.001, "expected 15, got {avg}");

    cleanup_entities(&pool, &[project_id, t1, t2]).await;
}

#[tokio::test]
async fn test_rollup_empty() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Zeta").await;

    let config = RollupConfig {
        link_field: "project/tasks".into(),
        rollup_field: "task/score".into(),
        function: RollupFn::Sum,
    };
    let result = rollup::compute_rollup(&pool, project_id, &config)
        .await
        .expect("rollup empty");
    // Empty rollup should return 0 for Sum.
    let val = result.as_f64().unwrap_or(0.0);
    assert!(val.abs() < 0.001, "empty sum should be 0, got {val}");

    cleanup_entities(&pool, &[project_id]).await;
}

// ===========================================================================
// 5. CASCADE DELETE
// ===========================================================================

#[tokio::test]
async fn test_cascade_delete_cleans_links() {
    let Some((pool, store)) = setup().await else {
        return;
    };

    let (project_id, _) = create_project(&store, "Cascade Project").await;
    let (task_id, _) = create_task(&store, "Cascade Task", 99).await;

    // Link project -> task with backlink.
    link::add_link(
        &pool,
        project_id,
        task_id,
        "project/tasks",
        Relationship::OneToMany,
        true,
        Some("task/project"),
    )
    .await
    .expect("link");

    // Verify forward link exists.
    let linked = link::get_linked(&pool, project_id, "project/tasks")
        .await
        .expect("forward");
    assert!(linked.contains(&task_id));

    // Cascade delete the task.
    let cascade_event = ddb_server::relations::cascade_delete(&pool, task_id, None, None)
        .await
        .expect("cascade delete");

    assert_eq!(cascade_event.trigger_entity_id, task_id);
    assert!(!cascade_event.affected_entity_ids.is_empty());

    // Forward link from project should now be broken.
    let linked_after = link::get_linked(&pool, project_id, "project/tasks")
        .await
        .expect("forward after");
    assert!(
        !linked_after.contains(&task_id),
        "link to deleted entity should be retracted"
    );

    // Backlink should also be gone.
    let backlink = link::get_linked(&pool, task_id, "task/project")
        .await
        .expect("backlink");
    assert!(backlink.is_empty(), "backlink should be retracted");

    cleanup_entities(&pool, &[project_id, task_id]).await;
}

// ===========================================================================
// 6. LINK META PERSISTENCE
// ===========================================================================

#[tokio::test]
async fn test_link_meta_create_and_read() {
    let Some((pool, _store)) = setup().await else {
        return;
    };

    let attr_name = format!("test_link_{}", Uuid::new_v4().as_simple());
    let config = LinkConfig {
        source_table: "Project".into(),
        target_table: "Task".into(),
        relationship: Relationship::OneToMany,
        symmetric: true,
        backlink_name: Some("task/project".into()),
    };

    link::create_link(&pool, &attr_name, config.clone())
        .await
        .expect("create link meta");

    let meta = link::get_link_meta(&pool, &attr_name)
        .await
        .expect("read meta")
        .expect("meta should exist");

    assert_eq!(meta.attribute, attr_name);
    assert_eq!(meta.config.source_table, "Project");
    assert_eq!(meta.config.target_table, "Task");
    assert_eq!(meta.config.relationship, Relationship::OneToMany);
    assert!(meta.config.symmetric);
    assert_eq!(meta.config.backlink_name, Some("task/project".into()));

    // Clean up link meta entity.
    let meta_eid = Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("link_meta:{attr_name}").as_bytes(),
    );
    cleanup_entities(&pool, &[meta_eid]).await;
}
