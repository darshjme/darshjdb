//! Cascade operations for relational field changes.
//!
//! When a linked record is updated or deleted, cascade ensures that:
//! - All link triples involving the entity are cleaned up.
//! - Lookup and rollup caches pointing at the entity are invalidated.
//! - Change events are emitted for real-time subscribers.
//!
//! The cascade module integrates with the formula dependency graph
//! when formulas reference lookup or rollup fields — a change to a
//! linked record propagates through the formula DAG.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::error::Result;
use crate::sync::broadcaster::ChangeEvent;
use crate::triple_store::schema::ValueType;

use super::link;
use super::lookup::LookupCache;

// ── Types ──────────────────────────────────────────────────────────

/// An event emitted when cascade operations complete.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeEvent {
    /// The entity that triggered the cascade.
    pub trigger_entity_id: Uuid,
    /// The operation that triggered the cascade.
    pub operation: CascadeOperation,
    /// Entity IDs that were affected by the cascade.
    pub affected_entity_ids: Vec<Uuid>,
    /// Attributes that were invalidated.
    pub invalidated_attributes: Vec<String>,
}

/// The type of operation that triggered a cascade.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CascadeOperation {
    /// An entity was deleted.
    Delete,
    /// An entity's attribute was updated.
    Update,
    /// A link was added or removed.
    LinkChange,
}

// ── Cascade delete ─────────────────────────────────────────────────

/// Handle the cascade effects of deleting an entity.
///
/// 1. Retract all link triples (both directions) involving this entity.
/// 2. Invalidate lookup/rollup caches.
/// 3. Emit a change event for real-time subscribers.
///
/// This does NOT delete the entity itself — that is the caller's
/// responsibility. This only handles the relational side effects.
pub async fn cascade_delete(
    pool: &PgPool,
    entity_id: Uuid,
    lookup_cache: Option<&LookupCache>,
    change_tx: Option<&broadcast::Sender<ChangeEvent>>,
) -> Result<CascadeEvent> {
    // Step 1: Find all entities that link TO this entity (reverse references).
    let affected = find_entities_referencing(pool, entity_id).await?;

    // Step 2: Retract all link triples involving this entity.
    link::retract_all_links_for_entity(pool, entity_id).await?;

    // Step 3: Invalidate caches.
    if let Some(cache) = lookup_cache {
        cache.invalidate_entity(entity_id).await;
        for aid in &affected {
            cache.invalidate_entity(*aid).await;
        }
    }

    // Step 4: Emit change event.
    let mut all_affected = affected.clone();
    all_affected.push(entity_id);

    if let Some(tx) = change_tx {
        let event = ChangeEvent {
            tx_id: 0, // Caller should fill with actual tx_id.
            entity_ids: all_affected.iter().map(|id| id.to_string()).collect(),
            attributes: vec!["*".to_string()], // All attributes invalidated.
            entity_type: None,
            actor_id: None,
        };
        let _ = tx.send(event);
    }

    Ok(CascadeEvent {
        trigger_entity_id: entity_id,
        operation: CascadeOperation::Delete,
        affected_entity_ids: affected,
        invalidated_attributes: vec!["*".to_string()],
    })
}

// ── Cascade update ─────────────────────────────────────────────────

/// Handle the cascade effects of updating an entity's attribute.
///
/// When a field value changes on an entity that is the TARGET of links,
/// any lookups/rollups that reference that field must be invalidated.
pub async fn cascade_update(
    pool: &PgPool,
    entity_id: Uuid,
    changed_attributes: &[String],
    lookup_cache: Option<&LookupCache>,
    change_tx: Option<&broadcast::Sender<ChangeEvent>>,
) -> Result<CascadeEvent> {
    // Find entities that link TO this entity.
    let referencing = find_entities_referencing(pool, entity_id).await?;

    // Invalidate lookup caches for the changed attributes.
    if let Some(cache) = lookup_cache {
        for attr in changed_attributes {
            cache.invalidate_target_field(attr).await;
        }
        // Also invalidate caches for entities that reference this one.
        for rid in &referencing {
            cache.invalidate_entity(*rid).await;
        }
    }

    // Emit change event for affected entities.
    if let Some(tx) = change_tx {
        let mut all_ids: Vec<String> = referencing.iter().map(|id| id.to_string()).collect();
        all_ids.push(entity_id.to_string());

        let event = ChangeEvent {
            tx_id: 0,
            entity_ids: all_ids,
            attributes: changed_attributes.to_vec(),
            entity_type: None,
            actor_id: None,
        };
        let _ = tx.send(event);
    }

    Ok(CascadeEvent {
        trigger_entity_id: entity_id,
        operation: CascadeOperation::Update,
        affected_entity_ids: referencing,
        invalidated_attributes: changed_attributes.to_vec(),
    })
}

/// Handle the cascade effects of a link being added or removed.
///
/// Both the source and target entity caches must be invalidated,
/// and any lookups/rollups involving the link field need recomputation.
pub async fn cascade_link_change(
    _pool: &PgPool,
    source_id: Uuid,
    target_id: Uuid,
    link_attribute: &str,
    lookup_cache: Option<&LookupCache>,
    change_tx: Option<&broadcast::Sender<ChangeEvent>>,
) -> Result<CascadeEvent> {
    // Invalidate caches.
    if let Some(cache) = lookup_cache {
        cache.invalidate_entity(source_id).await;
        cache.invalidate_entity(target_id).await;
        cache.invalidate_link(link_attribute).await;
    }

    // Emit change event.
    if let Some(tx) = change_tx {
        let event = ChangeEvent {
            tx_id: 0,
            entity_ids: vec![source_id.to_string(), target_id.to_string()],
            attributes: vec![link_attribute.to_string()],
            entity_type: None,
            actor_id: None,
        };
        let _ = tx.send(event);
    }

    Ok(CascadeEvent {
        trigger_entity_id: source_id,
        operation: CascadeOperation::LinkChange,
        affected_entity_ids: vec![target_id],
        invalidated_attributes: vec![link_attribute.to_string()],
    })
}

// ── Helpers ────────────────────────────────────────────────────────

/// Find all entity IDs that have a Reference triple pointing to `target_id`.
async fn find_entities_referencing(pool: &PgPool, target_id: Uuid) -> Result<Vec<Uuid>> {
    let target_str = target_id.to_string();

    let rows: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT entity_id
        FROM triples
        WHERE value_type = $1
          AND value = $2::jsonb
          AND NOT retracted
        "#,
    )
    .bind(ValueType::Reference as i16)
    .bind(serde_json::Value::String(target_str))
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id,)| id).collect())
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_event_serialization() {
        let event = CascadeEvent {
            trigger_entity_id: Uuid::nil(),
            operation: CascadeOperation::Delete,
            affected_entity_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
            invalidated_attributes: vec!["name".into(), "email".into()],
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["operation"], "delete");
        assert_eq!(json["invalidated_attributes"].as_array().unwrap().len(), 2);

        let back: CascadeEvent = serde_json::from_value(json).unwrap();
        assert_eq!(back.operation, CascadeOperation::Delete);
        assert_eq!(back.affected_entity_ids.len(), 2);
    }

    #[test]
    fn cascade_operation_serialization() {
        let ops = [
            (CascadeOperation::Delete, "delete"),
            (CascadeOperation::Update, "update"),
            (CascadeOperation::LinkChange, "link_change"),
        ];
        for (op, expected) in ops {
            let json = serde_json::to_value(op).unwrap();
            assert_eq!(json, expected);
            let back: CascadeOperation = serde_json::from_value(json).unwrap();
            assert_eq!(back, op);
        }
    }

    #[test]
    fn cascade_event_with_wildcard_attributes() {
        let event = CascadeEvent {
            trigger_entity_id: Uuid::nil(),
            operation: CascadeOperation::Delete,
            affected_entity_ids: vec![],
            invalidated_attributes: vec!["*".into()],
        };
        assert_eq!(event.invalidated_attributes, vec!["*"]);
    }
}
