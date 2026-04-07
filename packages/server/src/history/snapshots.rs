//! Table-level snapshots for entity types.
//!
//! A snapshot records the current `tx_id` as a checkpoint for a given
//! entity type. Later, you can diff against or restore to that snapshot,
//! effectively providing a "save point" for an entire collection.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, Triple, TripleInput};

// ── Types ─────────────────────────────────────────────────────────

/// A stored snapshot checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Snapshot {
    /// Unique snapshot id.
    pub id: Uuid,
    /// The entity type prefix (e.g. `"user"`, `"order"`).
    pub entity_type: String,
    /// Human-readable name for this snapshot.
    pub name: String,
    /// Optional description.
    pub description: String,
    /// Who created this snapshot.
    pub created_by: Option<Uuid>,
    /// When this snapshot was created.
    pub created_at: DateTime<Utc>,
    /// Number of records at the time of snapshot.
    pub record_count: i32,
    /// The tx_id at the time the snapshot was taken. All triples with
    /// `tx_id <= this` are considered part of the snapshot.
    pub tx_id_at_snapshot: i64,
}

/// Summary of changes since a snapshot was taken.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDiff {
    /// The snapshot this diff is relative to.
    pub snapshot_id: Uuid,
    /// Entity type.
    pub entity_type: String,
    /// The snapshot's tx_id.
    pub snapshot_tx_id: i64,
    /// Current latest tx_id.
    pub current_tx_id: i64,
    /// Number of entities that were created after the snapshot.
    pub entities_created: usize,
    /// Number of entities that were modified after the snapshot.
    pub entities_modified: usize,
    /// Number of entities that were deleted (all triples retracted) after the snapshot.
    pub entities_deleted: usize,
    /// Total number of new triples written since the snapshot.
    pub triples_added: i64,
    /// Total number of triples retracted since the snapshot.
    pub triples_retracted: i64,
}

// ── Public API ────────────────────────────────────────────────────

/// Create a new snapshot for an entity type.
///
/// Records the current maximum `tx_id` as the checkpoint and counts
/// the number of active records of that type.
pub async fn create_snapshot(
    pool: &PgPool,
    entity_type: &str,
    name: &str,
    description: &str,
    created_by: Option<Uuid>,
) -> Result<Snapshot> {
    // Get the current max tx_id.
    let max_tx: (Option<i64>,) =
        sqlx::query_as("SELECT MAX(tx_id) FROM triples")
            .fetch_one(pool)
            .await?;

    let tx_id_at_snapshot = max_tx.0.unwrap_or(0);

    // Count distinct active entities of this type.
    // Entity type is determined by the attribute prefix (e.g. "user/name" → type "user").
    let prefix = format!("{entity_type}/%");
    let record_count: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT entity_id)
        FROM triples
        WHERE attribute LIKE $1 AND NOT retracted AND tx_id <= $2
        "#,
    )
    .bind(&prefix)
    .bind(tx_id_at_snapshot)
    .fetch_one(pool)
    .await?;

    let snapshot = sqlx::query_as::<_, Snapshot>(
        r#"
        INSERT INTO snapshots (entity_type, name, description, created_by, record_count, tx_id_at_snapshot)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, entity_type, name, description, created_by, created_at, record_count, tx_id_at_snapshot
        "#,
    )
    .bind(entity_type)
    .bind(name)
    .bind(description)
    .bind(created_by)
    .bind(record_count.0 as i32)
    .bind(tx_id_at_snapshot)
    .fetch_one(pool)
    .await?;

    Ok(snapshot)
}

/// List all snapshots for an entity type, most recent first.
pub async fn list_snapshots(
    pool: &PgPool,
    entity_type: &str,
) -> Result<Vec<Snapshot>> {
    let snapshots = sqlx::query_as::<_, Snapshot>(
        r#"
        SELECT id, entity_type, name, description, created_by, created_at,
               record_count, tx_id_at_snapshot
        FROM snapshots
        WHERE entity_type = $1
        ORDER BY created_at DESC
        "#,
    )
    .bind(entity_type)
    .fetch_all(pool)
    .await?;

    Ok(snapshots)
}

/// Compute a diff showing what changed since a snapshot was taken.
pub async fn diff_snapshot(
    pool: &PgPool,
    snapshot_id: Uuid,
) -> Result<SnapshotDiff> {
    let snapshot = sqlx::query_as::<_, Snapshot>(
        r#"
        SELECT id, entity_type, name, description, created_by, created_at,
               record_count, tx_id_at_snapshot
        FROM snapshots
        WHERE id = $1
        "#,
    )
    .bind(snapshot_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DarshJError::InvalidQuery(format!("snapshot {snapshot_id} not found")))?;

    let prefix = format!("{}/%", snapshot.entity_type);

    // Current max tx_id.
    let current_tx: (Option<i64>,) =
        sqlx::query_as("SELECT MAX(tx_id) FROM triples")
            .fetch_one(pool)
            .await?;
    let current_tx_id = current_tx.0.unwrap_or(0);

    // Count new non-retracted triples since snapshot.
    let added: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM triples
        WHERE attribute LIKE $1 AND NOT retracted AND tx_id > $2
        "#,
    )
    .bind(&prefix)
    .bind(snapshot.tx_id_at_snapshot)
    .fetch_one(pool)
    .await?;

    // Count retractions since snapshot.
    let retracted: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM triples
        WHERE attribute LIKE $1 AND retracted AND tx_id > $2
        "#,
    )
    .bind(&prefix)
    .bind(snapshot.tx_id_at_snapshot)
    .fetch_one(pool)
    .await?;

    // Entities that have triples ONLY after the snapshot (created).
    let created: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT entity_id)
        FROM triples
        WHERE attribute LIKE $1 AND tx_id > $2
          AND entity_id NOT IN (
              SELECT DISTINCT entity_id FROM triples
              WHERE attribute LIKE $1 AND tx_id <= $2
          )
        "#,
    )
    .bind(&prefix)
    .bind(snapshot.tx_id_at_snapshot)
    .fetch_one(pool)
    .await?;

    // Entities that had triples before AND after the snapshot (modified).
    let modified: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT entity_id)
        FROM triples
        WHERE attribute LIKE $1 AND tx_id > $2
          AND entity_id IN (
              SELECT DISTINCT entity_id FROM triples
              WHERE attribute LIKE $1 AND tx_id <= $2
          )
        "#,
    )
    .bind(&prefix)
    .bind(snapshot.tx_id_at_snapshot)
    .fetch_one(pool)
    .await?;

    // Entities that were fully retracted after the snapshot.
    // These existed at snapshot time but now have zero active triples.
    let deleted: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT t1.entity_id)
        FROM triples t1
        WHERE t1.attribute LIKE $1
          AND t1.tx_id <= $2
          AND NOT EXISTS (
              SELECT 1 FROM triples t2
              WHERE t2.entity_id = t1.entity_id
                AND t2.attribute LIKE $1
                AND NOT t2.retracted
          )
        "#,
    )
    .bind(&prefix)
    .bind(snapshot.tx_id_at_snapshot)
    .fetch_one(pool)
    .await?;

    Ok(SnapshotDiff {
        snapshot_id,
        entity_type: snapshot.entity_type,
        snapshot_tx_id: snapshot.tx_id_at_snapshot,
        current_tx_id,
        entities_created: created.0 as usize,
        entities_modified: modified.0 as usize,
        entities_deleted: deleted.0 as usize,
        triples_added: added.0,
        triples_retracted: retracted.0,
    })
}

/// Restore all records of an entity type to their state at the snapshot's tx_id.
///
/// For each entity of the given type:
/// 1. Reconstruct its state at `snapshot.tx_id_at_snapshot` (replay triples up to that tx).
/// 2. Compare against the current state.
/// 3. Write new triples to bring it back to the snapshot state.
///
/// Entities created after the snapshot are fully retracted.
/// Entities deleted after the snapshot are re-asserted.
/// This is a potentially expensive operation on large datasets.
pub async fn restore_snapshot(
    pool: &PgPool,
    snapshot_id: Uuid,
) -> Result<i64> {
    let snapshot = sqlx::query_as::<_, Snapshot>(
        r#"
        SELECT id, entity_type, name, description, created_by, created_at,
               record_count, tx_id_at_snapshot
        FROM snapshots
        WHERE id = $1
        "#,
    )
    .bind(snapshot_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DarshJError::InvalidQuery(format!("snapshot {snapshot_id} not found")))?;

    let prefix = format!("{}/%", snapshot.entity_type);

    // Find all entity_ids that have any triples matching this type.
    let entity_ids: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT entity_id
        FROM triples
        WHERE attribute LIKE $1
        "#,
    )
    .bind(&prefix)
    .fetch_all(pool)
    .await?;

    let mut db_tx = pool.begin().await?;
    let tx_id = PgTripleStore::next_tx_id_in_tx(&mut db_tx).await?;

    for (entity_id,) in &entity_ids {
        // Get all triples for this entity up to the snapshot tx_id.
        let snapshot_triples: Vec<Triple> = sqlx::query_as(
            r#"
            SELECT id, entity_id, attribute, value, value_type, tx_id,
                   created_at, retracted, expires_at
            FROM triples
            WHERE entity_id = $1 AND attribute LIKE $2 AND tx_id <= $3
            ORDER BY tx_id ASC, id ASC
            "#,
        )
        .bind(entity_id)
        .bind(&prefix)
        .bind(snapshot.tx_id_at_snapshot)
        .fetch_all(&mut *db_tx)
        .await?;

        // Replay to build snapshot state.
        let mut snapshot_state: HashMap<String, Value> = HashMap::new();
        for t in &snapshot_triples {
            if t.retracted {
                snapshot_state.remove(&t.attribute);
            } else {
                snapshot_state.insert(t.attribute.clone(), t.value.clone());
            }
        }

        // Get current active state.
        let current_triples: Vec<Triple> = sqlx::query_as(
            r#"
            SELECT DISTINCT ON (attribute)
                   id, entity_id, attribute, value, value_type, tx_id,
                   created_at, retracted, expires_at
            FROM triples
            WHERE entity_id = $1 AND attribute LIKE $2 AND NOT retracted
            ORDER BY attribute, tx_id DESC
            "#,
        )
        .bind(entity_id)
        .bind(&prefix)
        .fetch_all(&mut *db_tx)
        .await?;

        let mut current_state: HashMap<String, Value> = HashMap::new();
        for t in &current_triples {
            current_state.insert(t.attribute.clone(), t.value.clone());
        }

        // Skip if already matching.
        if current_state == snapshot_state {
            continue;
        }

        // Retract attributes that differ or are absent in snapshot state.
        for (attr, cur_val) in &current_state {
            match snapshot_state.get(attr) {
                Some(snap_val) if snap_val == cur_val => {}
                _ => {
                    sqlx::query(
                        "UPDATE triples SET retracted = true WHERE entity_id = $1 AND attribute = $2 AND NOT retracted",
                    )
                    .bind(entity_id)
                    .bind(attr)
                    .execute(&mut *db_tx)
                    .await?;
                }
            }
        }

        // Assert attributes from snapshot state that are new or changed.
        let mut new_triples = Vec::new();
        for (attr, snap_val) in &snapshot_state {
            match current_state.get(attr) {
                Some(cur_val) if cur_val == snap_val => {}
                _ => {
                    new_triples.push(TripleInput {
                        entity_id: *entity_id,
                        attribute: attr.clone(),
                        value: snap_val.clone(),
                        value_type: 0,
                        ttl_seconds: None,
                    });
                }
            }
        }

        if !new_triples.is_empty() {
            PgTripleStore::set_triples_in_tx(&mut db_tx, &new_triples, tx_id).await?;
        }
    }

    db_tx.commit().await?;
    Ok(tx_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_diff_struct() {
        let diff = SnapshotDiff {
            snapshot_id: Uuid::new_v4(),
            entity_type: "user".into(),
            snapshot_tx_id: 10,
            current_tx_id: 20,
            entities_created: 3,
            entities_modified: 5,
            entities_deleted: 1,
            triples_added: 15,
            triples_retracted: 4,
        };

        // Verify serialization round-trip.
        let json = serde_json::to_string(&diff).unwrap();
        let parsed: SnapshotDiff = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.snapshot_tx_id, 10);
        assert_eq!(parsed.entities_created, 3);
        assert_eq!(parsed.entities_deleted, 1);
    }

    #[test]
    fn test_snapshot_struct_serialization() {
        let snap = Snapshot {
            id: Uuid::new_v4(),
            entity_type: "order".into(),
            name: "pre-migration".into(),
            description: "Snapshot before schema migration".into(),
            created_by: Some(Uuid::new_v4()),
            created_at: Utc::now(),
            record_count: 42,
            tx_id_at_snapshot: 100,
        };

        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["entity_type"], "order");
        assert_eq!(json["record_count"], 42);
        assert_eq!(json["tx_id_at_snapshot"], 100);
    }
}
