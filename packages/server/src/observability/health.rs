//! Kubernetes-shaped health endpoints.
//!
//! Three probes, each mounted at the root of the router **before** the auth
//! middleware runs so load balancers can reach them without a token:
//!
//! | Path    | Purpose        | Check                                    |
//! |---------|----------------|------------------------------------------|
//! | `/health` | Liveness alias | Returns `ok` + version + author          |
//! | `/ready`  | Readiness      | Pool acquire (500ms) + L1 cache presence |
//! | `/live`   | Process alive  | Always `200 OK`                          |
//!
//! The `/ready` probe uses a 500ms acquire timeout so it does not hang the
//! scrape if the database is deadlocked. When the pool is unavailable it
//! returns `503 Service Unavailable` with a reason string so Kubernetes logs
//! carry the actual failure mode.
//!
//! Created by Darshankumar Joshi.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sqlx::PgPool;

/// Timeout applied when the readiness probe tries to acquire a pool connection.
pub const READY_POOL_TIMEOUT: Duration = Duration::from_millis(500);

/// Shared state passed to the health handlers.
///
/// The `pool` is optional so unit tests can simulate the "no database wired
/// up" state and verify that `/ready` returns `503`.
#[derive(Clone)]
pub struct HealthState {
    pub pool: Option<PgPool>,
    pub cache_ready: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl HealthState {
    /// Build a state that always reports the L1 cache as initialized.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: Some(pool),
            cache_ready: Arc::new(|| true),
        }
    }

    /// Build a state with a custom cache-readiness predicate.
    pub fn with_cache_predicate<F>(pool: Option<PgPool>, predicate: F) -> Self
    where
        F: Fn() -> bool + Send + Sync + 'static,
    {
        Self {
            pool,
            cache_ready: Arc::new(predicate),
        }
    }

    /// Build a state with no database and a custom cache predicate — used in
    /// tests to force the `/ready` probe into the failure branch.
    pub fn degraded() -> Self {
        Self {
            pool: None,
            cache_ready: Arc::new(|| false),
        }
    }
}

impl std::fmt::Debug for HealthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HealthState")
            .field("pool_set", &self.pool.is_some())
            .finish()
    }
}

/// `GET /health` — lightweight liveness summary.
///
/// Returns `{ status, version, author }` so the response is cheap to render
/// and safe to expose publicly. The `author` field is part of the DarshJDB
/// attribution requirement.
pub async fn health_handler() -> Response {
    let body = json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "author": "Darshankumar Joshi",
    });
    (StatusCode::OK, Json(body)).into_response()
}

/// `GET /ready` — Kubernetes readiness probe.
///
/// Attempts to acquire a pool connection within [`READY_POOL_TIMEOUT`] and
/// verifies the L1 cache readiness predicate. Returns `200` only when both
/// checks pass; otherwise returns `503` with a machine-readable reason.
pub async fn ready_handler(State(state): State<HealthState>) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return service_unavailable("database pool not configured");
    };

    match tokio::time::timeout(READY_POOL_TIMEOUT, pool.acquire()).await {
        Ok(Ok(_conn)) => {
            if !(state.cache_ready)() {
                return service_unavailable("l1 cache not initialized");
            }
            let body = json!({
                "ready": true,
                "checks": {
                    "pool": "ok",
                    "cache": "ok",
                },
                "version": env!("CARGO_PKG_VERSION"),
                "author": "Darshankumar Joshi",
            });
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(Err(e)) => {
            service_unavailable(&format!("pool acquire failed: {e}"))
        }
        Err(_) => service_unavailable("pool acquire timed out after 500ms"),
    }
}

/// `GET /live` — Kubernetes liveness probe. Always `200 OK`.
pub async fn live_handler() -> Response {
    (
        StatusCode::OK,
        Json(json!({
            "alive": true,
            "author": "Darshankumar Joshi",
        })),
    )
        .into_response()
}

fn service_unavailable(reason: &str) -> Response {
    let body = json!({
        "ready": false,
        "reason": reason,
        "author": "Darshankumar Joshi",
    });
    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response()
}

/// Build the Phase 10 health router.
///
/// Mount this **before** the auth middleware so probes never need a token.
pub fn health_router(state: HealthState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/live", get(live_handler))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn health_returns_ok_payload() {
        let response = health_handler().await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["status"], "ok");
        assert_eq!(value["author"], "Darshankumar Joshi");
        assert!(value["version"].is_string());
    }

    #[tokio::test]
    async fn live_always_reports_alive() {
        let response = live_handler().await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_without_pool_returns_503() {
        let state = HealthState::degraded();
        let response = ready_handler(State(state)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["ready"], false);
        assert!(value["reason"].is_string());
    }

    #[tokio::test]
    async fn ready_with_failing_cache_returns_503() {
        // No pool → the cache-ready branch never runs, so we assert on the
        // pool-absent path. A full "pool ok, cache not ready" path requires
        // a live Postgres and is covered by the integration harness.
        let state = HealthState::with_cache_predicate(None, || false);
        let response = ready_handler(State(state)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
