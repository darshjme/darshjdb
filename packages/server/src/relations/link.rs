//! Link field: bidirectional references between entities.
//!
//! Links are the foundation of the relational system. They store
//! `ValueType::Reference` triples and, for symmetric links, create
//! matching backlinks on the target entity.
//!
//! # Storage Layout
//!
//! - **OneToOne / OneToMany**: Direct reference triples on the entity.
//!   `(source_id, "tasks", target_id, Reference)` with a symmetric
//!   backlink `(target_id, "project", source_id, Reference)`.
//!
//! - **ManyToMany**: Junction entity `link:{uuid}` with two reference
//!   triples — `link/source → source_id` and `link/target → target_id`.
//!   This avoids unbounded fan-out on either side.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::schema::ValueType;
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

// ── Constants ──────────────────────────────────────────────────────

/// Attribute on junction entities pointing to the source of the link.
pub const JUNCTION_SOURCE_ATTR: &str = "link/source";
/// Attribute on junction entities pointing to the target of the link.
pub const JUNCTION_TARGET_ATTR: &str = "link/target";
/// Attribute on junction entities naming the link attribute.
pub const JUNCTION_ATTR_ATTR: &str = "link/attribute";
/// Type marker for junction entities.
pub const JUNCTION_TYPE_ATTR: &str = "db/type";
/// Type value for junction entities.
pub const JUNCTION_TYPE_VALUE: &str = "link_junction";

// ── Types ──────────────────────────────────────────────────────────

/// Cardinality of a link relationship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    /// One source links to exactly one target.
    OneToOne,
    /// One source links to many targets.
    OneToMany,
    /// Many sources link to many targets (uses junction entities).
    ManyToMany,
}

/// Configuration for creating a link field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkConfig {
    /// The entity type that owns this link field.
    pub source_table: String,
    /// The entity type being linked to.
    pub target_table: String,
    /// Cardinality of the relationship.
    pub relationship: Relationship,
    /// Whether to create a symmetric backlink on the target.
    pub symmetric: bool,
    /// Attribute name for the backlink on the target entity.
    /// Required when `symmetric` is true.
    pub backlink_name: Option<String>,
}

/// Metadata persisted as a triple to describe an active link field.
/// Stored on a sentinel entity `link_meta:{attribute}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkMeta {
    /// The link attribute name on the source entity.
    pub attribute: String,
    /// Full link configuration.
    pub config: LinkConfig,
}

// ── Link setup ─────────────────────────────────────────────────────

/// Register a link field by persisting its metadata as triples.
///
/// This does NOT create any actual references — it records the link
/// configuration so that [`add_link`] and [`remove_link`] know how
/// to handle each attribute.
pub async fn create_link(pool: &PgPool, attribute: &str, config: LinkConfig) -> Result<()> {
    // Validate config.
    if config.symmetric && config.backlink_name.is_none() {
        return Err(DarshJError::InvalidAttribute(
            "symmetric links require a backlink_name".into(),
        ));
    }

    let store = PgTripleStore::new_lazy(pool.clone());
    let meta_entity = link_meta_entity_id(attribute);

    let meta_json = serde_json::to_value(&LinkMeta {
        attribute: attribute.to_string(),
        config,
    })
    .map_err(|e| DarshJError::Internal(format!("failed to serialize link meta: {e}")))?;

    let triples = vec![
        TripleInput {
            entity_id: meta_entity,
            attribute: "link_meta/config".to_string(),
            value: meta_json,
            value_type: ValueType::Json as i16,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id: meta_entity,
            attribute: JUNCTION_TYPE_ATTR.to_string(),
            value: serde_json::Value::String("link_meta".to_string()),
            value_type: ValueType::String as i16,
            ttl_seconds: None,
        },
    ];

    store.set_triples(&triples).await?;
    Ok(())
}

/// Load the link metadata for a given attribute, if it exists.
pub async fn get_link_meta(pool: &PgPool, attribute: &str) -> Result<Option<LinkMeta>> {
    let store = PgTripleStore::new_lazy(pool.clone());
    let meta_entity = link_meta_entity_id(attribute);
    let triples = store.get_entity(meta_entity).await?;

    for t in &triples {
        if t.attribute == "link_meta/config" {
            let meta: LinkMeta = serde_json::from_value(t.value.clone())?;
            return Ok(Some(meta));
        }
    }
    Ok(None)
}

// ── Link CRUD ──────────────────────────────────────────────────────

/// Add a link between two entities.
///
/// For OneToOne/OneToMany relationships, creates a reference triple on the
/// source entity. For ManyToMany, creates a junction entity.
///
/// If the link is symmetric, also creates the reverse reference (or a
/// second junction triple pointing back).
pub async fn add_link(
    pool: &PgPool,
    source_id: Uuid,
    target_id: Uuid,
    link_attribute: &str,
    relationship: Relationship,
    symmetric: bool,
    backlink_name: Option<&str>,
) -> Result<i64> {
    let store = PgTripleStore::new_lazy(pool.clone());

    match relationship {
        Relationship::OneToOne | Relationship::OneToMany => {
            let mut triples = vec![make_ref_triple(source_id, link_attribute, target_id)];

            if symmetric {
                let bl = backlink_name.ok_or_else(|| {
                    DarshJError::InvalidAttribute("symmetric links require a backlink_name".into())
                })?;
                triples.push(make_ref_triple(target_id, bl, source_id));
            }

            // For OneToOne, retract any existing reference first.
            if relationship == Relationship::OneToOne {
                let _ = store.retract(source_id, link_attribute).await;
                if symmetric && let Some(bl) = backlink_name {
                    let _ = store.retract(target_id, bl).await;
                }
            }

            store.set_triples(&triples).await
        }

        Relationship::ManyToMany => {
            let junction_id = Uuid::new_v4();
            let mut triples = vec![
                TripleInput {
                    entity_id: junction_id,
                    attribute: JUNCTION_TYPE_ATTR.to_string(),
                    value: serde_json::Value::String(JUNCTION_TYPE_VALUE.to_string()),
                    value_type: ValueType::String as i16,
                    ttl_seconds: None,
                },
                TripleInput {
                    entity_id: junction_id,
                    attribute: JUNCTION_ATTR_ATTR.to_string(),
                    value: serde_json::Value::String(link_attribute.to_string()),
                    value_type: ValueType::String as i16,
                    ttl_seconds: None,
                },
                make_junction_ref(junction_id, JUNCTION_SOURCE_ATTR, source_id),
                make_junction_ref(junction_id, JUNCTION_TARGET_ATTR, target_id),
            ];

            // Also store a direct reference on the source for fast lookups.
            triples.push(make_ref_triple(source_id, link_attribute, target_id));

            if symmetric {
                let bl = backlink_name.ok_or_else(|| {
                    DarshJError::InvalidAttribute("symmetric links require a backlink_name".into())
                })?;
                triples.push(make_ref_triple(target_id, bl, source_id));
            }

            store.set_triples(&triples).await
        }
    }
}

/// Remove a link between two entities.
///
/// Retracts the reference triple(s) and, for ManyToMany, the junction entity.
/// If symmetric, also retracts the reverse direction.
pub async fn remove_link(
    pool: &PgPool,
    source_id: Uuid,
    target_id: Uuid,
    link_attribute: &str,
    relationship: Relationship,
    symmetric: bool,
    backlink_name: Option<&str>,
) -> Result<()> {
    let store = PgTripleStore::new_lazy(pool.clone());

    match relationship {
        Relationship::OneToOne | Relationship::OneToMany => {
            // Retract the specific reference triple.
            retract_specific_ref(&store, source_id, link_attribute, target_id).await?;

            if symmetric && let Some(bl) = backlink_name {
                retract_specific_ref(&store, target_id, bl, source_id).await?;
            }
        }

        Relationship::ManyToMany => {
            // Find and retract the junction entity.
            retract_junction(&store, source_id, target_id, link_attribute).await?;

            // Also retract the direct reference on the source.
            retract_specific_ref(&store, source_id, link_attribute, target_id).await?;

            if symmetric && let Some(bl) = backlink_name {
                retract_specific_ref(&store, target_id, bl, source_id).await?;
            }
        }
    }

    Ok(())
}

/// Resolve all entity IDs linked from `entity_id` via `link_attribute`.
///
/// Returns the UUIDs of all linked entities (following active, non-retracted
/// reference triples).
pub async fn get_linked(pool: &PgPool, entity_id: Uuid, link_attribute: &str) -> Result<Vec<Uuid>> {
    let store = PgTripleStore::new_lazy(pool.clone());
    let triples = store.get_attribute(entity_id, link_attribute).await?;

    let mut linked = Vec::new();
    for t in triples {
        if t.value_type == ValueType::Reference as i16
            && let Some(uuid_str) = t.value.as_str()
            && let Ok(id) = Uuid::parse_str(uuid_str)
        {
            linked.push(id);
        }
    }
    Ok(linked)
}

/// Remove all links involving a given entity (both as source and target).
///
/// Used by cascade delete to clean up all references when an entity is removed.
pub async fn retract_all_links_for_entity(pool: &PgPool, entity_id: Uuid) -> Result<()> {
    // Retract all triples on this entity that are references.
    sqlx::query(
        r#"
        UPDATE triples
        SET retracted = true
        WHERE entity_id = $1
          AND value_type = $2
          AND NOT retracted
        "#,
    )
    .bind(entity_id)
    .bind(ValueType::Reference as i16)
    .execute(pool)
    .await?;

    // Retract all triples pointing TO this entity (backlinks).
    let entity_str = entity_id.to_string();
    sqlx::query(
        r#"
        UPDATE triples
        SET retracted = true
        WHERE value_type = $1
          AND NOT retracted
          AND value = $2::jsonb
        "#,
    )
    .bind(ValueType::Reference as i16)
    .bind(serde_json::Value::String(entity_str.clone()))
    .execute(pool)
    .await?;

    // Retract any junction entities where this entity is source or target.
    sqlx::query(
        r#"
        UPDATE triples
        SET retracted = true
        WHERE entity_id IN (
            SELECT entity_id
            FROM triples
            WHERE attribute IN ($1, $2)
              AND value_type = $3
              AND value = $4::jsonb
              AND NOT retracted
        )
        AND NOT retracted
        "#,
    )
    .bind(JUNCTION_SOURCE_ATTR)
    .bind(JUNCTION_TARGET_ATTR)
    .bind(ValueType::Reference as i16)
    .bind(serde_json::Value::String(entity_str))
    .execute(pool)
    .await?;

    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────

/// Deterministic entity ID for link metadata, derived from the attribute name.
///
/// Uses a SHA-256 hash truncated to 16 bytes to produce a stable UUID
/// without requiring the `v5` feature on the `uuid` crate.
fn link_meta_entity_id(attribute: &str) -> Uuid {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(format!("link_meta:{attribute}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    // Set version 4 variant bits so it's a valid UUID.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

/// Build a reference triple input.
fn make_ref_triple(entity_id: Uuid, attribute: &str, target_id: Uuid) -> TripleInput {
    TripleInput {
        entity_id,
        attribute: attribute.to_string(),
        value: serde_json::Value::String(target_id.to_string()),
        value_type: ValueType::Reference as i16,
        ttl_seconds: None,
    }
}

/// Build a junction reference triple input.
fn make_junction_ref(junction_id: Uuid, attribute: &str, target_id: Uuid) -> TripleInput {
    TripleInput {
        entity_id: junction_id,
        attribute: attribute.to_string(),
        value: serde_json::Value::String(target_id.to_string()),
        value_type: ValueType::Reference as i16,
        ttl_seconds: None,
    }
}

/// Retract a specific reference triple (source → target via attribute).
///
/// Unlike `store.retract(entity_id, attribute)` which retracts ALL triples
/// for that attribute, this only retracts the one pointing to `target_id`.
async fn retract_specific_ref(
    store: &PgTripleStore,
    source_id: Uuid,
    attribute: &str,
    target_id: Uuid,
) -> Result<()> {
    let pool = store.pool();
    let target_str = target_id.to_string();

    sqlx::query(
        r#"
        UPDATE triples
        SET retracted = true
        WHERE entity_id = $1
          AND attribute = $2
          AND value_type = $3
          AND value = $4::jsonb
          AND NOT retracted
        "#,
    )
    .bind(source_id)
    .bind(attribute)
    .bind(ValueType::Reference as i16)
    .bind(serde_json::Value::String(target_str))
    .execute(pool)
    .await?;

    Ok(())
}

/// Retract a junction entity connecting source → target for a given link attribute.
async fn retract_junction(
    store: &PgTripleStore,
    source_id: Uuid,
    target_id: Uuid,
    link_attribute: &str,
) -> Result<()> {
    let pool = store.pool();
    let source_str = source_id.to_string();
    let target_str = target_id.to_string();

    // Find junction entities matching this specific source → target + attribute.
    let junction_ids: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT j.entity_id
        FROM triples j
        WHERE j.attribute = $1
          AND j.value = $2::jsonb
          AND j.value_type = $3
          AND NOT j.retracted
          AND j.entity_id IN (
              SELECT t.entity_id
              FROM triples t
              WHERE t.attribute = $4
                AND t.value = $5::jsonb
                AND t.value_type = $3
                AND NOT t.retracted
          )
          AND j.entity_id IN (
              SELECT a.entity_id
              FROM triples a
              WHERE a.attribute = $6
                AND a.value = $7::jsonb
                AND NOT a.retracted
          )
        "#,
    )
    .bind(JUNCTION_SOURCE_ATTR)
    .bind(serde_json::Value::String(source_str))
    .bind(ValueType::Reference as i16)
    .bind(JUNCTION_TARGET_ATTR)
    .bind(serde_json::Value::String(target_str))
    .bind(JUNCTION_ATTR_ATTR)
    .bind(serde_json::Value::String(link_attribute.to_string()))
    .fetch_all(pool)
    .await?;

    // Retract all triples on each matched junction entity.
    for (jid,) in junction_ids {
        sqlx::query("UPDATE triples SET retracted = true WHERE entity_id = $1 AND NOT retracted")
            .bind(jid)
            .execute(pool)
            .await?;
    }

    Ok(())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_meta_entity_id_deterministic() {
        let a = link_meta_entity_id("tasks");
        let b = link_meta_entity_id("tasks");
        assert_eq!(a, b);
    }

    #[test]
    fn link_meta_entity_id_different_for_different_attrs() {
        let a = link_meta_entity_id("tasks");
        let b = link_meta_entity_id("projects");
        assert_ne!(a, b);
    }

    #[test]
    fn make_ref_triple_has_correct_shape() {
        let src = Uuid::new_v4();
        let tgt = Uuid::new_v4();
        let t = make_ref_triple(src, "assigned_to", tgt);
        assert_eq!(t.entity_id, src);
        assert_eq!(t.attribute, "assigned_to");
        assert_eq!(t.value, serde_json::Value::String(tgt.to_string()));
        assert_eq!(t.value_type, ValueType::Reference as i16);
        assert!(t.ttl_seconds.is_none());
    }

    #[test]
    fn relationship_serialization_roundtrip() {
        let variants = [
            Relationship::OneToOne,
            Relationship::OneToMany,
            Relationship::ManyToMany,
        ];
        for r in variants {
            let json = serde_json::to_string(&r).unwrap();
            let back: Relationship = serde_json::from_str(&json).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn link_config_serialization() {
        let config = LinkConfig {
            source_table: "project".into(),
            target_table: "task".into(),
            relationship: Relationship::OneToMany,
            symmetric: true,
            backlink_name: Some("project".into()),
        };
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["source_table"], "project");
        assert_eq!(json["target_table"], "task");
        assert_eq!(json["relationship"], "one_to_many");
        assert!(json["symmetric"].as_bool().unwrap());
        assert_eq!(json["backlink_name"], "project");

        let back: LinkConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back.source_table, config.source_table);
    }

    #[test]
    fn link_meta_serialization() {
        let meta = LinkMeta {
            attribute: "tasks".into(),
            config: LinkConfig {
                source_table: "project".into(),
                target_table: "task".into(),
                relationship: Relationship::ManyToMany,
                symmetric: false,
                backlink_name: None,
            },
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: LinkMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.attribute, "tasks");
        assert_eq!(back.config.relationship, Relationship::ManyToMany);
    }

    #[test]
    fn junction_constants_are_namespaced() {
        assert!(JUNCTION_SOURCE_ATTR.starts_with("link/"));
        assert!(JUNCTION_TARGET_ATTR.starts_with("link/"));
        assert!(JUNCTION_ATTR_ATTR.starts_with("link/"));
    }
}
