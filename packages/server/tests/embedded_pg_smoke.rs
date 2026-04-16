//! Smoke test for the `embedded-db` feature (slice 17/30).
//!
//! Author: Darshankumar Joshi
//!
//! Verifies that `start_embedded_postgres` can:
//!   1. Download + unpack a portable Postgres 16 distribution
//!   2. Start a local server on a free port
//!   3. Create the `darshjdb` database
//!   4. Hand back a valid `postgres://…/darshjdb` URI
//!   5. Shut down cleanly when the `PgEmbed` handle drops
//!
//! This test is expensive (downloads ~20 MB on first run) and is therefore
//! both feature-gated (`embedded-db`) AND ignored by default. Run manually:
//!
//! ```bash
//! cargo test -p ddb-server --features embedded-db \
//!   --test embedded_pg_smoke -- --ignored --nocapture
//! ```

#![cfg(feature = "embedded-db")]

use std::time::Duration;

#[tokio::test]
#[ignore = "downloads Postgres 16 binary — run manually"]
async fn embedded_postgres_boots_and_hands_back_uri() {
    let tmp = tempfile::tempdir().expect("tmpdir");

    let (pg, uri) = ddb_server::embedded_pg::start_embedded_postgres(tmp.path())
        .await
        .expect("embedded Postgres should start");

    assert!(
        uri.starts_with("postgres://"),
        "expected postgres URI, got {uri}"
    );
    assert!(
        uri.contains("/darshjdb"),
        "URI missing database name: {uri}"
    );

    // Hold the process alive briefly, then drop to verify graceful shutdown.
    tokio::time::sleep(Duration::from_millis(250)).await;
    drop(pg);
}
