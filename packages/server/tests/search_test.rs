//! Phase 3 search integration tests (slice 16/30).
//!
//! Covers:
//!   - Postgres FTS over `triples.value::text` with `entity_type` filter.
//!   - Reciprocal Rank Fusion combining semantic + text rankings.
//!   - The `embeddings` table upsert path the handlers depend on.
//!
//! Tests run against a real Postgres database with the `pgvector` extension
//! installed. They follow the same opt-in convention as the rest of the
//! integration suite: when `DATABASE_URL` is not set every test silently
//! passes (returns early) so they stay compatible with the rest of the
//! suite and CI environments without a database.
//!
//! These tests exercise the SQL layer that backs the HTTP handlers in
//! `packages/server/src/api/rest.rs` (slice 16/30). Mirroring the handler
//! SQL keeps the tests honest without needing to construct an AppState
//! and Router for every assertion.
//!
//! Author: Darshankumar Joshi.

#![cfg(test)]

use ddb_server::api::rest::ensure_search_schema;
use ddb_server::triple_store::{PgTripleStore, TripleInput, TripleStore};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

/// Connect to the integration database and ensure every schema the search
/// endpoints depend on is in place. Returns `None` when `DATABASE_URL` is
/// unset so the test can no-op.
async fn setup_pool() -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&url).await.ok()?;
    PgTripleStore::new(pool.clone()).await.ok()?;
    ensure_search_schema(&pool).await.ok()?;
    Some(pool)
}

/// Insert a `:db/type` triple plus a `body` triple for each fixture so both
/// the FTS index and the entity_type join have something to chew on.
async fn seed_articles(triple_store: &PgTripleStore, fixtures: &[(Uuid, &str, &str)]) {
    for (id, kind, body) in fixtures {
        let triples = vec![
            TripleInput {
                entity_id: *id,
                attribute: ":db/type".to_string(),
                value: Value::String((*kind).to_string()),
                value_type: 0,
                ttl_seconds: None,
            },
            TripleInput {
                entity_id: *id,
                attribute: "body".to_string(),
                value: Value::String((*body).to_string()),
                value_type: 0,
                ttl_seconds: None,
            },
        ];
        triple_store
            .set_triples(&triples)
            .await
            .expect("seed triples");
    }
}

async fn cleanup(pool: &PgPool, ids: &[Uuid]) {
    if ids.is_empty() {
        return;
    }
    sqlx::query("DELETE FROM triples WHERE entity_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM embeddings WHERE entity_id = ANY($1)")
        .bind(ids)
        .execute(pool)
        .await
        .ok();
}

/// Format a slice of `f32` as a pgvector literal — mirrors the helper in
/// `rest.rs` so the test does not need to depend on a private function.
fn pgvector_literal(vec: &[f32]) -> String {
    let mut s = String::with_capacity(vec.len() * 8 + 2);
    s.push('[');
    for (i, v) in vec.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&v.to_string());
    }
    s.push(']');
    s
}

/// Direct-SQL replica of `run_text_search` from `rest.rs` so we can assert
/// the FTS + entity_type filter without spinning up the HTTP router.
async fn run_text_search(
    pool: &PgPool,
    q: &str,
    entity_type: &str,
    limit: i32,
) -> Vec<(Uuid, f64)> {
    let sql = "SELECT t.entity_id, \
                      MAX(ts_rank(to_tsvector('english', t.value::text), \
                                  plainto_tsquery('english', $1))::float8) AS rank \
               FROM triples t \
               INNER JOIN triples t_type ON t_type.entity_id = t.entity_id \
                 AND t_type.attribute = ':db/type' \
                 AND t_type.value = $2::jsonb \
                 AND NOT t_type.retracted \
               WHERE NOT t.retracted \
                 AND t.value IS NOT NULL \
                 AND to_tsvector('english', t.value::text) @@ plainto_tsquery('english', $1) \
               GROUP BY t.entity_id \
               ORDER BY rank DESC \
               LIMIT $3";
    sqlx::query_as::<_, (Uuid, f64)>(sql)
        .bind(q)
        .bind(Value::String(entity_type.to_string()))
        .bind(limit)
        .fetch_all(pool)
        .await
        .expect("text search")
}

/// Direct-SQL replica of `run_semantic_search` (no attribute filter).
async fn run_semantic_search(
    pool: &PgPool,
    vector: &[f32],
    entity_type: &str,
    attribute: &str,
    limit: i32,
) -> Vec<(Uuid, String, f64)> {
    let lit = pgvector_literal(vector);
    let sql = "SELECT e.entity_id, e.attribute, \
                      (e.embedding <=> $4::vector) AS distance \
               FROM embeddings e \
               INNER JOIN triples t_type ON t_type.entity_id = e.entity_id \
                 AND t_type.attribute = ':db/type' \
                 AND t_type.value = $1::jsonb \
                 AND NOT t_type.retracted \
               WHERE e.attribute = $2 \
               ORDER BY e.embedding <=> $4::vector \
               LIMIT $3";
    sqlx::query_as::<_, (Uuid, String, f64)>(sql)
        .bind(Value::String(entity_type.to_string()))
        .bind(attribute)
        .bind(limit)
        .bind(&lit)
        .fetch_all(pool)
        .await
        .expect("semantic search")
}

/// Upsert an embedding row via SQL (the same statement `embeddings_store`
/// uses).
async fn upsert_embedding(
    pool: &PgPool,
    entity_id: Uuid,
    attribute: &str,
    vector: &[f32],
    model: &str,
) {
    let lit = pgvector_literal(vector);
    sqlx::query(
        "INSERT INTO embeddings (entity_id, attribute, embedding, model) \
         VALUES ($1, $2, $3::vector, $4) \
         ON CONFLICT (entity_id, attribute) DO UPDATE \
            SET embedding  = EXCLUDED.embedding, \
                model      = EXCLUDED.model, \
                created_at = now()",
    )
    .bind(entity_id)
    .bind(attribute)
    .bind(&lit)
    .bind(model)
    .execute(pool)
    .await
    .expect("upsert embedding");
}

/// Reciprocal Rank Fusion — replica of the production helper used so the
/// test can assert end-to-end fused ordering without exposing the private
/// `reciprocal_rank_fuse` function from `rest.rs`.
fn rrf_fuse(
    semantic: &[(Uuid, String, f64)],
    text: &[(Uuid, f64)],
    w_sem: f64,
    w_text: f64,
) -> Vec<(Uuid, Option<f64>, Option<f64>, f64)> {
    use std::collections::{HashMap, HashSet};
    const K: f64 = 60.0;

    let mut map: HashMap<Uuid, (Option<f64>, Option<f64>, f64)> = HashMap::new();

    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut sem_rank = 0usize;
    for (id, _, dist) in semantic {
        if !seen.insert(*id) {
            continue;
        }
        sem_rank += 1;
        let entry = map.entry(*id).or_insert((None, None, 0.0));
        entry.0 = Some(1.0 - *dist);
        entry.2 += w_sem / (K + sem_rank as f64);
    }

    let mut text_rank = 0usize;
    for (id, rank) in text {
        text_rank += 1;
        let entry = map.entry(*id).or_insert((None, None, 0.0));
        entry.1 = Some(*rank);
        entry.2 += w_text / (K + text_rank as f64);
    }

    let mut out: Vec<(Uuid, Option<f64>, Option<f64>, f64)> = map
        .into_iter()
        .map(|(id, (sem, txt, score))| (id, sem, txt, score))
        .collect();
    out.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
    out
}

// ---------------------------------------------------------------------------
// Test 1 — text search respects entity_type filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_text_respects_entity_type_filter() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let triple_store = PgTripleStore::new_lazy(pool.clone());

    let article_id = Uuid::new_v4();
    let comment_id = Uuid::new_v4();
    let other_article_id = Uuid::new_v4();

    seed_articles(
        &triple_store,
        &[
            (
                article_id,
                "SearchTestArticle",
                "Postgres pgvector ranks high on hybrid search benchmarks",
            ),
            (
                comment_id,
                "SearchTestComment",
                "pgvector hybrid search comment commentary",
            ),
            (
                other_article_id,
                "SearchTestArticle",
                "An unrelated story about kitchen appliances",
            ),
        ],
    )
    .await;

    let rows = run_text_search(&pool, "pgvector", "SearchTestArticle", 10).await;
    let returned_ids: Vec<Uuid> = rows.iter().map(|(id, _)| *id).collect();

    assert!(
        returned_ids.contains(&article_id),
        "matching article should be returned, got {:?}",
        returned_ids,
    );
    assert!(
        !returned_ids.contains(&comment_id),
        "entity_type filter must exclude SearchTestComment hits, got {:?}",
        returned_ids,
    );
    assert!(
        !returned_ids.contains(&other_article_id),
        "non-matching article should not appear in FTS results, got {:?}",
        returned_ids,
    );

    // Ranks are non-increasing.
    for win in rows.windows(2) {
        assert!(
            win[0].1 >= win[1].1,
            "ts_rank must be non-increasing: {:?}",
            rows,
        );
    }

    cleanup(&pool, &[article_id, comment_id, other_article_id]).await;
}

// ---------------------------------------------------------------------------
// Test 2 — hybrid search merges semantic + text rankings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_hybrid_returns_merged_ranking() {
    let Some(pool) = setup_pool().await else {
        return;
    };
    let triple_store = PgTripleStore::new_lazy(pool.clone());

    // Two entities: one wins on text relevance, one wins on vector proximity.
    let text_winner = Uuid::new_v4();
    let vector_winner = Uuid::new_v4();
    let unrelated = Uuid::new_v4();

    seed_articles(
        &triple_store,
        &[
            (
                text_winner,
                "HybridTestDoc",
                "darshjdb darshjdb darshjdb darshjdb hybrid search exemplar",
            ),
            (
                vector_winner,
                "HybridTestDoc",
                "this body has nothing in common with the user query at all",
            ),
            (
                unrelated,
                "HybridTestDoc",
                "completely unrelated content about gardening tools",
            ),
        ],
    )
    .await;

    // Build three 1536-d embeddings so the cosine ANN has something to chew on.
    let mut query_vec = vec![0.0_f32; 1536];
    query_vec[0] = 1.0;

    let mut vector_winner_vec = vec![0.0_f32; 1536];
    vector_winner_vec[0] = 1.0; // identical to query → cosine distance 0

    let mut text_winner_vec = vec![0.0_f32; 1536];
    text_winner_vec[1] = 1.0; // orthogonal → cosine distance 1

    let mut unrelated_vec = vec![0.0_f32; 1536];
    unrelated_vec[2] = 1.0;

    upsert_embedding(
        &pool,
        text_winner,
        "body",
        &text_winner_vec,
        "text-embedding-3-small",
    )
    .await;
    upsert_embedding(
        &pool,
        vector_winner,
        "body",
        &vector_winner_vec,
        "text-embedding-3-small",
    )
    .await;
    upsert_embedding(
        &pool,
        unrelated,
        "body",
        &unrelated_vec,
        "text-embedding-3-small",
    )
    .await;

    // Run both sides at the SQL level...
    let semantic_rows =
        run_semantic_search(&pool, &query_vec, "HybridTestDoc", "body", 20).await;
    let text_rows = run_text_search(&pool, "darshjdb hybrid search", "HybridTestDoc", 20).await;

    // ...and fuse them with the same RRF formula the handler uses.
    let fused = rrf_fuse(&semantic_rows, &text_rows, 1.0, 1.0);

    assert!(
        fused.len() >= 2,
        "expected at least 2 fused hits, got {:?}",
        fused,
    );

    let ids: Vec<Uuid> = fused.iter().map(|(id, _, _, _)| *id).collect();

    // The vector winner sits at the top of the semantic side, so it must
    // appear in the fused output.
    assert!(
        ids.contains(&vector_winner),
        "vector-side winner missing from fused output: {:?}",
        ids,
    );
    // The text winner should be in the FTS top hits, so it must also appear.
    assert!(
        ids.contains(&text_winner),
        "text-side winner missing from fused output: {:?}",
        ids,
    );

    // RRF scores are strictly non-increasing.
    for win in fused.windows(2) {
        assert!(
            win[0].3 >= win[1].3,
            "rrf_score must be non-increasing: {:?}",
            fused,
        );
    }

    // Entities that appear in BOTH lists must outrank entities that only
    // appear in one — the very property RRF is designed for. Sanity-check
    // by confirming the top hit has both scores populated whenever both
    // winners are present.
    let top = &fused[0];
    if ids.contains(&text_winner) && ids.contains(&vector_winner) {
        // At least one of the two top fused hits must have BOTH scores set
        // (the entity that benefits from both rankings).
        let both = fused
            .iter()
            .take(2)
            .any(|(_, sem, txt, _)| sem.is_some() && txt.is_some());
        assert!(
            both,
            "top fused hits should include an entity with both scores; top={:?} all={:?}",
            top, fused,
        );
    }

    // Bias the fusion heavily toward the semantic side: vector_winner
    // (rank 1 on the semantic list, distance 0) must climb to the very top.
    let fused_sem_heavy = rrf_fuse(&semantic_rows, &text_rows, 10.0, 1.0);
    if !fused_sem_heavy.is_empty() {
        // The first fused hit's id must be the semantic-side winner.
        assert_eq!(
            fused_sem_heavy[0].0, vector_winner,
            "with semantic weight=10 the vector winner should top the ranking, got {:?}",
            fused_sem_heavy,
        );
    }

    // Verify the upsert path works: re-upserting the same key must not
    // create a duplicate row.
    upsert_embedding(
        &pool,
        text_winner,
        "body",
        &text_winner_vec,
        "text-embedding-3-small",
    )
    .await;
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM embeddings WHERE entity_id = $1 AND attribute = $2")
            .bind(text_winner)
            .bind("body")
            .fetch_one(&pool)
            .await
            .expect("count embeddings");
    assert_eq!(
        count.0, 1,
        "upsert must keep exactly one row per (entity_id, attribute)",
    );

    cleanup(&pool, &[text_winner, vector_winner, unrelated]).await;
}
