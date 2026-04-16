//! Embedded Postgres bootstrap for zero-dependency development mode.
//!
//! # Author
//! Darshankumar Joshi
//!
//! # Overview
//! Gated behind the `embedded-db` Cargo feature. When compiled with the flag
//! and invoked at startup (typically because `DATABASE_URL` is unset), this
//! module downloads and launches a portable Postgres 16 binary in a user-local
//! data directory, creates the `darshjdb` database, and returns a `sqlx`-ready
//! connection URI.
//!
//! The embedded server is started via [`pg_embed`] and uses a randomly picked
//! free TCP port supplied by [`portpicker`]. The caller is expected to hold the
//! returned [`PgEmbed`] handle for the lifetime of the server so that the
//! child Postgres process is gracefully shut down on drop.
//!
//! # Example
//! ```no_run
//! # #[cfg(feature = "embedded-db")]
//! # async fn run() -> anyhow::Result<()> {
//! use std::path::PathBuf;
//! let data_dir = PathBuf::from("/tmp/darshjdb-data");
//! let (mut pg, uri) = ddb_server::embedded_pg::start_embedded_postgres(&data_dir).await?;
//! // Use `uri` to build a sqlx pool. Keep `pg` alive.
//! # drop(pg);
//! # Ok(())
//! # }
//! ```

#![cfg(feature = "embedded-db")]

use std::path::Path;
use std::time::Duration;

use pg_embed::pg_enums::PgAuthMethod;
use pg_embed::pg_fetch::{PG_V16, PgFetchSettings};
use pg_embed::postgres::{PgEmbed, PgSettings};

/// Bootstraps an embedded Postgres 16 server rooted at `data_dir/pg`.
///
/// Returns a tuple `(handle, uri)` where `handle` MUST be kept alive by the
/// caller for the full lifetime of the server — dropping it terminates the
/// embedded Postgres process. `uri` is a ready-to-use `postgres://…/darshjdb`
/// connection string.
///
/// # Behaviour
/// * Picks an unused TCP port via [`portpicker::pick_unused_port`].
/// * Uses plain-text auth with user `darshj` / password `darshj` — suitable
///   for local dev only.
/// * Sets `persistent = true` so data survives restarts across invocations
///   with the same `data_dir`.
/// * Creates a `darshjdb` database if it does not already exist.
///
/// # Errors
/// Propagates any I/O, download, unpack, start, or database-creation error
/// from `pg_embed` via [`anyhow::Error`].
pub async fn start_embedded_postgres(data_dir: &Path) -> anyhow::Result<(PgEmbed, String)> {
    let port = portpicker::pick_unused_port().expect("no free TCP port for embedded postgres");

    let settings = PgSettings {
        database_dir: data_dir.join("pg"),
        port,
        user: "darshj".into(),
        password: "darshj".into(),
        auth_method: PgAuthMethod::Plain,
        persistent: true,
        timeout: Some(Duration::from_secs(30)),
        migration_dir: None,
    };

    let fetch = PgFetchSettings {
        version: PG_V16,
        ..Default::default()
    };

    let mut pg = PgEmbed::new(settings, fetch).await?;

    tracing::info!(port, data_dir = %data_dir.display(), "setting up embedded Postgres 16");
    pg.setup().await?;

    tracing::info!("starting embedded Postgres 16");
    pg.start_db().await?;

    // `create_database` is idempotent-safe when guarded by `database_exists`.
    let db_exists = pg.database_exists("darshjdb").await.unwrap_or(false);
    if !db_exists {
        tracing::info!("creating embedded database 'darshjdb'");
        pg.create_database("darshjdb").await?;
    } else {
        tracing::info!("reusing existing embedded database 'darshjdb'");
    }

    let uri = pg.full_db_uri("darshjdb");
    tracing::info!("embedded Postgres ready");
    Ok((pg, uri))
}
