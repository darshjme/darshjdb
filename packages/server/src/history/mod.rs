//! Record history, point-in-time snapshots, and undo/restore for DarshJDB.
//!
//! The EAV triple store's append-only design means version history is
//! essentially free: every mutation creates new triples with a monotonically
//! increasing `tx_id`, and "deletions" are retractions rather than physical
//! removes. This module leverages that natural versioning to provide:
//!
//! - **Version history:** reconstruct any record's state at any version or
//!   point in time by replaying triples ordered by `tx_id`.
//! - **Undo/restore:** revert a record to a previous version by writing new
//!   triples that match the old state (append-only, never mutates history).
//! - **Table-level snapshots:** checkpoint an entity type at a given `tx_id`
//!   and later diff or restore to that point.
//!
//! # Architecture
//!
//! ```text
//! tx_1: name="Alice"  email="a@x.com"     ── version 1
//! tx_2: email="a@y.com"  (retract old)    ── version 2
//! tx_3: age=30                             ── version 3
//! tx_4: name="Bob"  (retract old name)     ── version 4
//!
//! get_version(entity, 2) →  { name: "Alice", email: "a@y.com" }
//! restore_version(entity, 2) → tx_5 writes name="Alice", email="a@y.com"
//! ```

pub mod handlers;
pub mod restore;
pub mod snapshots;
pub mod versions;

pub use restore::{restore_deleted, restore_version, undo_last};
pub use snapshots::{
    Snapshot, SnapshotDiff, create_snapshot, diff_snapshot, list_snapshots, restore_snapshot,
};
pub use versions::{ChangeType, FieldChange, RecordVersion, get_at_time, get_history, get_version};

/// Create the `snapshots` table if it does not exist.
///
/// Called during server bootstrap alongside the triple-store schema.
pub async fn ensure_history_schema(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS snapshots (
            id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            entity_type     TEXT NOT NULL,
            name            TEXT NOT NULL,
            description     TEXT NOT NULL DEFAULT '',
            created_by      UUID,
            created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
            record_count    INTEGER NOT NULL DEFAULT 0,
            tx_id_at_snapshot BIGINT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_snapshots_entity_type
            ON snapshots (entity_type);
        CREATE INDEX IF NOT EXISTS idx_snapshots_created_at
            ON snapshots (created_at DESC);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}
