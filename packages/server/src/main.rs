//! DarshanDB server binary entry point.
//!
//! This is a placeholder that validates all modules compile and
//! will be expanded into the full HTTP server in subsequent phases.

use darshandb_server::error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tracing::info!("DarshanDB server starting");
    tracing::info!("triple_store, query engine, and reactive tracker modules loaded");

    // Future: bind axum router, connect PgPool, serve.
    Ok(())
}
