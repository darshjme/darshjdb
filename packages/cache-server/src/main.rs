// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// ddb-cache-server :: main — entry point for the standalone RESP3
// protocol server.
//
// Environment:
//   DARSH_CACHE_PORT       — TCP port to bind (default 7701)
//   DARSH_CACHE_PASSWORD   — if set, requires AUTH before any command
//   RUST_LOG               — tracing filter (default "info")

use std::sync::Arc;

use ddb_cache::DdbCache;
use ddb_cache_server::{ServerConfig, serve};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = ServerConfig::from_env();
    let cache = Arc::new(DdbCache::new());

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %config.addr,
        auth = config.password.is_some(),
        "starting ddb-cache-server"
    );

    serve(config, cache).await
}
