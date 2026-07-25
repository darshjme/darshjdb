//! Startup migration runner for the Postgres backend.
//!
//! Every file under `packages/server/migrations/` is embedded into the binary
//! at compile time and applied, in order, during boot. Applied files are
//! recorded in `_ddb_migrations` so subsequent boots are a single `SELECT`.
//!
//! Two properties make this safe against a live production database:
//!
//! * **Every migration is written to be idempotent.** Tables and indexes use
//!   `CREATE ... IF NOT EXISTS`, columns use `ADD COLUMN IF NOT EXISTS`,
//!   functions use `CREATE OR REPLACE`, the one trigger is preceded by
//!   `DROP TRIGGER IF EXISTS`, and every type/rename/drop is wrapped in a
//!   `DO` block gated on a catalog lookup. A database that was bootstrapped by
//!   hand from `001_initial.sql` therefore has an empty ledger but re-applying
//!   the file is a no-op rather than a `duplicate_table` failure.
//! * **Each file runs in its own transaction and failures are isolated.** A
//!   migration that cannot apply on this deployment (for example the
//!   TimescaleDB hypertable on vanilla Postgres, or the pgvector section
//!   without the extension available) rolls back cleanly, is *not* recorded,
//!   and is retried on the next boot. It never aborts startup, which keeps the
//!   runner strictly additive over the previous behaviour where no migration
//!   ran at all.
//!
//! Set `DDB_SKIP_MIGRATIONS=1` to bypass the runner entirely.

use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// Ledger table recording which embedded migrations have been applied.
///
/// Distinct from `_schema_migrations`, which the user-facing schema engine in
/// [`crate::schema::migration`] uses for per-table version history.
const LEDGER_TABLE: &str = "_ddb_migrations";

/// Every migration file, in application order.
///
/// `seed.sql` is deliberately excluded — it is development fixture data, not
/// schema. The two files sharing the `20260414002030` prefix are ordered
/// alphabetically; they touch disjoint objects.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_initial.sql",
        include_str!("../migrations/001_initial.sql"),
    ),
    (
        "002_views_fields_tables.sql",
        include_str!("../migrations/002_views_fields_tables.sql"),
    ),
    (
        "20260414002020_kv_store.sql",
        include_str!("../migrations/20260414002020_kv_store.sql"),
    ),
    (
        "20260414002030_magic_link_tokens.sql",
        include_str!("../migrations/20260414002030_magic_link_tokens.sql"),
    ),
    (
        "20260414002030_session_hardening.sql",
        include_str!("../migrations/20260414002030_session_hardening.sql"),
    ),
    (
        "20260414002048_login_attempts.sql",
        include_str!("../migrations/20260414002048_login_attempts.sql"),
    ),
    (
        "20260414002423_chunked_uploads.sql",
        include_str!("../migrations/20260414002423_chunked_uploads.sql"),
    ),
    (
        "20260414003000_pgvector_bootstrap.sql",
        include_str!("../migrations/20260414003000_pgvector_bootstrap.sql"),
    ),
    (
        "20260414004000_schema_definitions_and_audit.sql",
        include_str!("../migrations/20260414004000_schema_definitions_and_audit.sql"),
    ),
    (
        "20260414055500_agent_memory.sql",
        include_str!("../migrations/20260414055500_agent_memory.sql"),
    ),
    (
        "20260414090000_timescale.sql",
        include_str!("../migrations/20260414090000_timescale.sql"),
    ),
    (
        "20260414100000_anchor_receipts.sql",
        include_str!("../migrations/20260414100000_anchor_receipts.sql"),
    ),
    (
        "20260414130000_sessions_cascade.sql",
        include_str!("../migrations/20260414130000_sessions_cascade.sql"),
    ),
    (
        "20260725000000_retraction_tx_id.sql",
        include_str!("../migrations/20260725000000_retraction_tx_id.sql"),
    ),
    (
        "20260725010000_embedding_worker_columns.sql",
        include_str!("../migrations/20260725010000_embedding_worker_columns.sql"),
    ),
];

/// Outcome of a single [`run`] pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MigrationReport {
    /// Files executed and recorded during this pass.
    pub applied: usize,
    /// Files already present in the ledger.
    pub skipped: usize,
    /// Files that failed and will be retried on the next boot.
    pub failed: usize,
}

fn checksum(sql: &str) -> String {
    hex::encode(Sha256::digest(sql.as_bytes()))
}

/// Apply every embedded migration that is not yet recorded in the ledger.
///
/// Callers should hold the schema advisory lock so concurrent replicas do not
/// race on DDL. Returns `Err` only when the ledger itself is unusable; an
/// individual migration failure is reported via [`MigrationReport::failed`].
pub async fn run(pool: &PgPool) -> Result<MigrationReport, sqlx::Error> {
    if std::env::var("DDB_SKIP_MIGRATIONS").is_ok_and(|v| v != "0" && !v.is_empty()) {
        tracing::warn!("DDB_SKIP_MIGRATIONS set — embedded migrations not applied");
        return Ok(MigrationReport::default());
    }

    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {LEDGER_TABLE} (
             name       TEXT        PRIMARY KEY,
             checksum   TEXT        NOT NULL,
             applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
         )"
    ))
    .execute(pool)
    .await?;

    let recorded: Vec<(String, String)> =
        sqlx::query_as(&format!("SELECT name, checksum FROM {LEDGER_TABLE}"))
            .fetch_all(pool)
            .await?;

    let mut report = MigrationReport::default();

    for (name, sql) in MIGRATIONS {
        let digest = checksum(sql);

        if let Some((_, applied_digest)) = recorded.iter().find(|(n, _)| n == name) {
            if applied_digest != &digest {
                // The base files are edited over time to mirror later
                // migrations. Re-running is not required for correctness, so
                // surface the drift instead of touching production schema.
                tracing::warn!(
                    migration = %name,
                    "migration checksum drifted since it was applied — not re-run"
                );
            }
            report.skipped += 1;
            continue;
        }

        let mut tx = pool.begin().await?;
        match sqlx::raw_sql(sql).execute(&mut *tx).await {
            Ok(_) => {
                sqlx::query(&format!(
                    "INSERT INTO {LEDGER_TABLE} (name, checksum) VALUES ($1, $2)
                     ON CONFLICT (name) DO NOTHING"
                ))
                .bind(name)
                .bind(&digest)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                report.applied += 1;
                tracing::info!(migration = %name, "migration applied");
            }
            Err(e) => {
                tx.rollback().await.ok();
                report.failed += 1;
                tracing::error!(
                    migration = %name,
                    error = %e,
                    "migration failed — rolled back, will retry on next boot"
                );
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_list_matches_directory() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
            .expect("migrations directory")
            .filter_map(|e| {
                let name = e.ok()?.file_name().to_string_lossy().into_owned();
                (name.ends_with(".sql") && name != "seed.sql").then_some(name)
            })
            .collect();
        on_disk.sort();

        let mut embedded: Vec<String> = MIGRATIONS.iter().map(|(n, _)| (*n).to_string()).collect();
        embedded.sort();

        assert_eq!(
            embedded, on_disk,
            "MIGRATIONS is out of sync with migrations/"
        );
    }

    #[test]
    fn migrations_are_in_ascending_order() {
        let names: Vec<&str> = MIGRATIONS.iter().map(|(n, _)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    #[test]
    fn checksum_is_stable_hex() {
        let digest = checksum("SELECT 1;");
        assert_eq!(digest.len(), 64);
        assert_eq!(digest, checksum("SELECT 1;"));
    }
}
