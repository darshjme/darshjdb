//! Lookup field: pull values from linked records.
//!
//! A lookup field follows a link to the target entity and reads a
//! specific attribute value from it. For OneToOne links this produces
//! a single value; for OneToMany/ManyToMany it produces an array.
//!
//! Results can be cached and are invalidated when the link or target
//! field changes (see [`cascade`](super::cascade)).

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::schema::ValueType;
use crate::triple_store::{PgTripleStore, TripleStore};

use super::link;

// ── Types ──────────────────────────────────────────────────────────

/// Configuration for a lookup field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupConfig {
    /// The link attribute to follow on the source entity.
    pub link_field: String,
    /// The attribute to read from the linked entity.
    pub lookup_field: String,
}

/// A lookup field definition with its resolution metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupField {
    /// Human-readable name of this lookup field.
    pub name: String,
    /// The link to traverse.
    pub source_link: String,
    /// The field to read on the target entity.
    pub target_field: String,
    /// Expected result type. `None` means infer from target field.
    pub result_type: Option<ValueType>,
}

/// Cached lookup result.
#[derive(Debug, Clone)]
struct CachedLookup {
    values: Vec<serde_json::Value>,
    cached_at: Instant,
}

/// Thread-safe lookup cache keyed by `(entity_id, link_field, lookup_field)`.
#[derive(Clone)]
pub struct LookupCache {
    inner: Arc<RwLock<HashMap<(Uuid, String, String), CachedLookup>>>,
    ttl: Duration,
}

impl LookupCache {
    /// Create a new cache with the given TTL for entries.
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            ttl,
        }
    }

    /// Create a cache with 30-second default TTL.
    pub fn default_ttl() -> Self {
        Self::new(Duration::from_secs(30))
    }

    /// Get a cached result if it exists and has not expired.
    pub async fn get(
        &self,
        entity_id: Uuid,
        link_field: &str,
        lookup_field: &str,
    ) -> Option<Vec<serde_json::Value>> {
        let guard = self.inner.read().await;
        let key = (entity_id, link_field.to_string(), lookup_field.to_string());
        if let Some(cached) = guard.get(&key) {
            if cached.cached_at.elapsed() < self.ttl {
                return Some(cached.values.clone());
            }
        }
        None
    }

    /// Store a result in the cache.
    pub async fn set(
        &self,
        entity_id: Uuid,
        link_field: &str,
        lookup_field: &str,
        values: Vec<serde_json::Value>,
    ) {
        let mut guard = self.inner.write().await;
        let key = (entity_id, link_field.to_string(), lookup_field.to_string());
        guard.insert(
            key,
            CachedLookup {
                values,
                cached_at: Instant::now(),
            },
        );
    }

    /// Invalidate all cached lookups for a given entity.
    pub async fn invalidate_entity(&self, entity_id: Uuid) {
        let mut guard = self.inner.write().await;
        guard.retain(|k, _| k.0 != entity_id);
    }

    /// Invalidate cached lookups that depend on a specific link field.
    pub async fn invalidate_link(&self, link_field: &str) {
        let mut guard = self.inner.write().await;
        guard.retain(|k, _| k.1 != link_field);
    }

    /// Invalidate lookups that read a specific target field
    /// (used when the target entity's field value changes).
    pub async fn invalidate_target_field(&self, lookup_field: &str) {
        let mut guard = self.inner.write().await;
        guard.retain(|k, _| k.2 != lookup_field);
    }

    /// Clear the entire cache.
    pub async fn clear(&self) {
        let mut guard = self.inner.write().await;
        guard.clear();
    }
}

// ── Resolution ─────────────────────────────────────────────────────

/// Resolve a lookup field for a given entity.
///
/// 1. Follow the link to find all linked entity IDs.
/// 2. For each linked entity, read the `lookup_field` attribute.
/// 3. Collect all values into a flat `Vec`.
///
/// If `cache` is provided, results are cached and served from cache
/// on subsequent calls until invalidated.
pub async fn resolve_lookup(
    pool: &PgPool,
    entity_id: Uuid,
    config: &LookupConfig,
    cache: Option<&LookupCache>,
) -> Result<Vec<serde_json::Value>> {
    // Check cache first.
    if let Some(c) = cache {
        if let Some(cached) = c.get(entity_id, &config.link_field, &config.lookup_field).await {
            return Ok(cached);
        }
    }

    // Step 1: Resolve linked entity IDs.
    let linked_ids = link::get_linked(pool, entity_id, &config.link_field).await?;

    if linked_ids.is_empty() {
        let empty = Vec::new();
        if let Some(c) = cache {
            c.set(entity_id, &config.link_field, &config.lookup_field, empty.clone())
                .await;
        }
        return Ok(empty);
    }

    // Step 2: Read the target field from each linked entity.
    let store = PgTripleStore::new_lazy(pool.clone());
    let mut values = Vec::with_capacity(linked_ids.len());

    for target_id in &linked_ids {
        let triples = store.get_attribute(*target_id, &config.lookup_field).await?;
        for t in triples {
            values.push(t.value);
        }
    }

    // Cache the result.
    if let Some(c) = cache {
        c.set(
            entity_id,
            &config.link_field,
            &config.lookup_field,
            values.clone(),
        )
        .await;
    }

    Ok(values)
}

/// Batch resolve a lookup for multiple entities at once.
///
/// More efficient than calling `resolve_lookup` in a loop when you need
/// the lookup values for an entire result set.
pub async fn resolve_lookup_batch(
    pool: &PgPool,
    entity_ids: &[Uuid],
    config: &LookupConfig,
) -> Result<HashMap<Uuid, Vec<serde_json::Value>>> {
    let mut results = HashMap::with_capacity(entity_ids.len());

    // Batch-fetch all reference triples for the link attribute across entities.
    let ref_triples: Vec<(Uuid, String)> = sqlx::query_as(
        r#"
        SELECT entity_id, value::text
        FROM triples
        WHERE entity_id = ANY($1)
          AND attribute = $2
          AND value_type = $3
          AND NOT retracted
        "#,
    )
    .bind(entity_ids)
    .bind(&config.link_field)
    .bind(ValueType::Reference as i16)
    .fetch_all(pool)
    .await?;

    // Build a map from source → linked target IDs.
    let mut links_map: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut all_target_ids: Vec<Uuid> = Vec::new();

    for (source_id, target_str) in &ref_triples {
        // The value is stored as a JSON string, so strip quotes.
        let clean = target_str.trim_matches('"');
        if let Ok(target_id) = Uuid::parse_str(clean) {
            links_map
                .entry(*source_id)
                .or_default()
                .push(target_id);
            all_target_ids.push(target_id);
        }
    }

    if all_target_ids.is_empty() {
        for eid in entity_ids {
            results.insert(*eid, Vec::new());
        }
        return Ok(results);
    }

    // Batch-fetch the lookup field values from all target entities.
    all_target_ids.sort();
    all_target_ids.dedup();

    let target_triples: Vec<(Uuid, serde_json::Value)> = sqlx::query_as(
        r#"
        SELECT entity_id, value
        FROM triples
        WHERE entity_id = ANY($1)
          AND attribute = $2
          AND NOT retracted
        "#,
    )
    .bind(&all_target_ids)
    .bind(&config.lookup_field)
    .fetch_all(pool)
    .await?;

    // Build target → values map.
    let mut target_values: HashMap<Uuid, Vec<serde_json::Value>> = HashMap::new();
    for (tid, val) in target_triples {
        target_values.entry(tid).or_default().push(val);
    }

    // Assemble results per source entity.
    for eid in entity_ids {
        let mut vals = Vec::new();
        if let Some(linked) = links_map.get(eid) {
            for tid in linked {
                if let Some(tv) = target_values.get(tid) {
                    vals.extend(tv.iter().cloned());
                }
            }
        }
        results.insert(*eid, vals);
    }

    Ok(results)
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_config_serialization() {
        let config = LookupConfig {
            link_field: "project".into(),
            lookup_field: "name".into(),
        };
        let json = serde_json::to_string(&config).unwrap();
        let back: LookupConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.link_field, "project");
        assert_eq!(back.lookup_field, "name");
    }

    #[test]
    fn lookup_field_serialization() {
        let field = LookupField {
            name: "Project Name".into(),
            source_link: "project".into(),
            target_field: "name".into(),
            result_type: Some(ValueType::String),
        };
        let json = serde_json::to_value(&field).unwrap();
        assert_eq!(json["name"], "Project Name");
        assert_eq!(json["source_link"], "project");
        assert_eq!(json["target_field"], "name");

        let back: LookupField = serde_json::from_value(json).unwrap();
        assert_eq!(back.result_type, Some(ValueType::String));
    }

    #[test]
    fn lookup_field_no_result_type() {
        let field = LookupField {
            name: "Budget".into(),
            source_link: "department".into(),
            target_field: "budget".into(),
            result_type: None,
        };
        let json = serde_json::to_value(&field).unwrap();
        let back: LookupField = serde_json::from_value(json).unwrap();
        assert!(back.result_type.is_none());
    }

    #[tokio::test]
    async fn lookup_cache_basic_operations() {
        let cache = LookupCache::new(Duration::from_secs(60));
        let id = Uuid::new_v4();

        // Miss on empty cache.
        assert!(cache.get(id, "link", "field").await.is_none());

        // Set and hit.
        let vals = vec![serde_json::json!("hello")];
        cache.set(id, "link", "field", vals.clone()).await;
        let got = cache.get(id, "link", "field").await.unwrap();
        assert_eq!(got, vals);

        // Invalidate by entity.
        cache.invalidate_entity(id).await;
        assert!(cache.get(id, "link", "field").await.is_none());
    }

    #[tokio::test]
    async fn lookup_cache_invalidate_link() {
        let cache = LookupCache::new(Duration::from_secs(60));
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        cache
            .set(id1, "projects", "name", vec![serde_json::json!("a")])
            .await;
        cache
            .set(id2, "projects", "status", vec![serde_json::json!("b")])
            .await;
        cache
            .set(id1, "tasks", "title", vec![serde_json::json!("c")])
            .await;

        // Invalidate all lookups going through "projects" link.
        cache.invalidate_link("projects").await;

        assert!(cache.get(id1, "projects", "name").await.is_none());
        assert!(cache.get(id2, "projects", "status").await.is_none());
        // "tasks" link should survive.
        assert!(cache.get(id1, "tasks", "title").await.is_some());
    }

    #[tokio::test]
    async fn lookup_cache_invalidate_target_field() {
        let cache = LookupCache::new(Duration::from_secs(60));
        let id = Uuid::new_v4();

        cache
            .set(id, "link_a", "name", vec![serde_json::json!("x")])
            .await;
        cache
            .set(id, "link_b", "name", vec![serde_json::json!("y")])
            .await;
        cache
            .set(id, "link_a", "email", vec![serde_json::json!("z")])
            .await;

        cache.invalidate_target_field("name").await;

        assert!(cache.get(id, "link_a", "name").await.is_none());
        assert!(cache.get(id, "link_b", "name").await.is_none());
        assert!(cache.get(id, "link_a", "email").await.is_some());
    }

    #[tokio::test]
    async fn lookup_cache_expiry() {
        let cache = LookupCache::new(Duration::from_millis(1));
        let id = Uuid::new_v4();

        cache
            .set(id, "link", "field", vec![serde_json::json!(42)])
            .await;

        // Wait for TTL to expire.
        tokio::time::sleep(Duration::from_millis(5)).await;

        assert!(cache.get(id, "link", "field").await.is_none());
    }

    #[tokio::test]
    async fn lookup_cache_clear() {
        let cache = LookupCache::new(Duration::from_secs(60));
        let id = Uuid::new_v4();

        cache
            .set(id, "a", "b", vec![serde_json::json!(1)])
            .await;
        cache.clear().await;
        assert!(cache.get(id, "a", "b").await.is_none());
    }
}
