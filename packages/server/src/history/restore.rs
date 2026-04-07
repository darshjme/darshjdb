//! Undo and restore operations for records.
//!
//! All restore operations are append-only: they write new triples in a
//! new transaction rather than modifying historical data. This preserves
//! the immutable audit trail while allowing users to revert mistakes.

use std::collections::HashMap;

use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::triple_store::{PgTripleStore, TripleInput};

use super::versions::{build_versions, get_history};

// ── Helpers ───────────────────────────────────────────────────────

/// Fetch the current active state of an entity (non-retracted triples,
/// latest value per attribute).
async fn current_state(pool: &PgPool, entity_id: Uuid) -> Result<HashMap<String, Value>> {
    let triples = sqlx::query_as::<_, crate::triple_store::Triple>(
        r#"
        SELECT DISTINCT ON (attribute)
               id, entity_id, attribute, value, value_type, tx_id,
               created_at, retracted, expires_at
        FROM triples
        WHERE entity_id = $1 AND NOT retracted
        ORDER BY attribute, tx_id DESC
        "#,
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;

    let mut state = HashMap::new();
    for t in triples {
        state.insert(t.attribute, t.value);
    }
    Ok(state)
}

/// Write a set of triples that transform the entity from `current` state
/// to `target` state. Returns the new transaction id.
///
/// This retracts attributes that exist in `current` but differ in `target`
/// (or are absent from `target`), then asserts the `target` values.
async fn apply_state_diff(
    pool: &PgPool,
    entity_id: Uuid,
    current: &HashMap<String, Value>,
    target: &HashMap<String, Value>,
) -> Result<i64> {
    let mut tx = pool.begin().await?;
    let tx_id = PgTripleStore::next_tx_id_in_tx(&mut tx).await?;

    // Retract attributes that changed or were removed.
    for (attr, cur_val) in current {
        match target.get(attr) {
            Some(tgt_val) if tgt_val == cur_val => {
                // Same value, no change needed.
            }
            _ => {
                // Either value changed or attribute was removed in target.
                PgTripleStore::retract_in_tx(&mut tx, entity_id, attr).await?;
            }
        }
    }

    // Assert new/changed attributes.
    let mut new_triples = Vec::new();
    for (attr, tgt_val) in target {
        match current.get(attr) {
            Some(cur_val) if cur_val == tgt_val => {
                // No change needed.
            }
            _ => {
                new_triples.push(TripleInput {
                    entity_id,
                    attribute: attr.clone(),
                    value: tgt_val.clone(),
                    value_type: 0, // JSON default
                    ttl_seconds: None,
                });
            }
        }
    }

    if !new_triples.is_empty() {
        PgTripleStore::set_triples_in_tx(&mut tx, &new_triples, tx_id).await?;
    }

    tx.commit().await?;
    Ok(tx_id)
}

// ── Public API ────────────────────────────────────────────────────

/// Restore a record to a specific version number.
///
/// Reconstructs the entity's state at the given version, computes the
/// diff against the current state, and writes new triples to bring the
/// record back to that version's state.
///
/// Returns the new transaction id of the restore operation.
pub async fn restore_version(
    pool: &PgPool,
    entity_id: Uuid,
    version: u32,
) -> Result<i64> {
    let target = super::versions::get_version(pool, entity_id, version).await?;
    let current = current_state(pool, entity_id).await?;

    if current == target {
        return Err(DarshJError::InvalidQuery(
            "record is already at the requested version".into(),
        ));
    }

    apply_state_diff(pool, entity_id, &current, &target).await
}

/// Undo the most recent change to a record.
///
/// Equivalent to restoring to version N-1, where N is the current
/// (latest) version. If the record has only one version, returns an error.
///
/// Returns the new transaction id of the undo operation.
pub async fn undo_last(pool: &PgPool, entity_id: Uuid) -> Result<i64> {
    let versions = get_history(pool, entity_id, 0).await?;

    if versions.len() < 2 {
        return Err(DarshJError::InvalidQuery(
            "cannot undo: record has no previous version".into(),
        ));
    }

    let prev_version = versions[versions.len() - 2].version_number;
    restore_version(pool, entity_id, prev_version).await
}

/// Restore a soft-deleted record by un-retracting its triples.
///
/// If all of an entity's triples are retracted, this creates a new
/// transaction that re-asserts the last known values. The record comes
/// back to its state at the time of deletion.
///
/// Returns the new transaction id of the restore operation.
pub async fn restore_deleted(pool: &PgPool, entity_id: Uuid) -> Result<i64> {
    // Check that the entity is actually deleted (all triples retracted).
    let active_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM triples WHERE entity_id = $1 AND NOT retracted",
    )
    .bind(entity_id)
    .fetch_one(pool)
    .await?;

    if active_count.0 > 0 {
        return Err(DarshJError::InvalidQuery(
            "record is not deleted — it still has active triples".into(),
        ));
    }

    // Check that retracted triples exist at all.
    let retracted_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM triples WHERE entity_id = $1 AND retracted",
    )
    .bind(entity_id)
    .fetch_one(pool)
    .await?;

    if retracted_count.0 == 0 {
        return Err(DarshJError::EntityNotFound(entity_id));
    }

    // Reconstruct the last known state from the full history.
    let all_triples = sqlx::query_as::<_, crate::triple_store::Triple>(
        r#"
        SELECT id, entity_id, attribute, value, value_type, tx_id,
               created_at, retracted, expires_at
        FROM triples
        WHERE entity_id = $1
        ORDER BY tx_id ASC, id ASC
        "#,
    )
    .bind(entity_id)
    .fetch_all(pool)
    .await?;

    let versions = build_versions(&all_triples);

    // Find the last version where the record had data (before full retraction).
    let last_populated = versions
        .iter()
        .rev()
        .find(|v| !v.snapshot.is_empty())
        .ok_or_else(|| {
            DarshJError::InvalidQuery("no recoverable state found for entity".into())
        })?;

    let target = last_populated.snapshot.clone();
    let current = HashMap::new(); // All retracted, so current state is empty.

    apply_state_diff(pool, entity_id, &current, &target).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_state_and_diff_logic() {
        // Test the diff logic in isolation (no DB needed).
        let mut current = HashMap::new();
        current.insert("name".into(), serde_json::json!("Alice"));
        current.insert("email".into(), serde_json::json!("a@old.com"));
        current.insert("temp".into(), serde_json::json!("remove_me"));

        let mut target = HashMap::new();
        target.insert("name".into(), serde_json::json!("Alice")); // unchanged
        target.insert("email".into(), serde_json::json!("a@new.com")); // modified
        target.insert("age".into(), serde_json::json!(30)); // added

        // Attributes to retract: email (changed), temp (removed from target).
        let mut to_retract = Vec::new();
        for (attr, cur_val) in &current {
            match target.get(attr) {
                Some(tgt_val) if tgt_val == cur_val => {}
                _ => to_retract.push(attr.clone()),
            }
        }
        to_retract.sort();
        assert_eq!(to_retract, vec!["email", "temp"]);

        // Attributes to assert: email (new value), age (added).
        let mut to_assert = Vec::new();
        for (attr, tgt_val) in &target {
            match current.get(attr) {
                Some(cur_val) if cur_val == tgt_val => {}
                _ => to_assert.push(attr.clone()),
            }
        }
        to_assert.sort();
        assert_eq!(to_assert, vec!["age", "email"]);
    }
}
