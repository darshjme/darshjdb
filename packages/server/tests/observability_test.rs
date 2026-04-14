//! Phase 10 — Observability integration tests (slice 29/30).
//!
//! Exercises the Prometheus `/metrics` route, the health probes, and the
//! request-id middleware end-to-end via Axum's `oneshot` harness. These
//! tests do **not** require a running Postgres; they use the `HealthState`
//! degraded mode to verify `/ready` returns `503` when the pool is absent,
//! and a lightweight in-process router for the happy paths.
//!
//! Created by Darshankumar Joshi.

use axum::body::{to_bytes, Body};
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Request, StatusCode};
use axum::Router;
use ddb_server::observability::{
    health_router, init_prometheus, live_handler, metrics_router, ready_handler, HealthState,
    MetricsIpAllowList,
};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Mutex;
use tower::util::ServiceExt;

// A process-wide guard — `init_prometheus` installs a global recorder and can
// only run once per test binary. We gate it behind a OnceLock so any test that
// needs a live MetricsHandle can call `ensure_prometheus()`.
static PROMETHEUS_HANDLE: Mutex<Option<ddb_server::observability::MetricsHandle>> =
    Mutex::new(None);

fn ensure_prometheus() -> ddb_server::observability::MetricsHandle {
    let mut guard = PROMETHEUS_HANDLE.lock().expect("prometheus mutex");
    if let Some(h) = guard.as_ref() {
        return h.clone();
    }
    let (handle, _allow) = init_prometheus().expect("prometheus install");
    *guard = Some(handle.clone());
    handle
}

#[tokio::test]
async fn health_endpoint_returns_ok_with_author() {
    let state = HealthState::degraded();
    let app: Router = health_router(state);
    let response = app
        .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["status"], "ok");
    assert_eq!(value["author"], "Darshankumar Joshi");
    assert!(value["version"].is_string());
}

#[tokio::test]
async fn live_endpoint_always_returns_200() {
    let app: Router = Router::new().route("/live", axum::routing::get(live_handler));
    let response = app
        .oneshot(Request::builder().uri("/live").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn ready_returns_503_when_pool_absent() {
    // HealthState::degraded() has no pool — the /ready branch must return 503
    // with a machine-readable reason so Kubernetes can restart us.
    let state = HealthState::degraded();
    let app: Router = health_router(state);
    let response = app
        .oneshot(Request::builder().uri("/ready").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["ready"], false);
    assert!(value["reason"].is_string());
}

#[tokio::test]
async fn ready_via_handler_direct() {
    // Redundant safety test — invoke the handler directly without a router
    // so regressions in axum's route matching cannot hide a 200-for-none bug.
    let state = HealthState::degraded();
    let response = ready_handler(axum::extract::State(state)).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn metrics_returns_prometheus_text_format_for_allowed_ip() {
    let handle = ensure_prometheus();
    let allow = MetricsIpAllowList::parse("127.0.0.1,::1");
    let router = metrics_router(handle, allow).layer(MockConnectInfo(SocketAddr::from((
        [127, 0, 0, 1],
        12345,
    ))));

    let response = router
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.contains("text/plain"),
        "content-type should be text/plain, got {content_type}"
    );

    let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();

    // Every metric we describe should render at least its HELP/TYPE header
    // even if no event has fired. Assert on a representative subset.
    for needle in [
        "ddb_http_requests_total",
        "ddb_http_latency_seconds",
        "ddb_triple_writes_total",
        "ddb_tx_total",
    ] {
        assert!(
            text.contains(needle),
            "expected metric {needle} in /metrics output, got:\n{text}"
        );
    }
}

#[tokio::test]
async fn metrics_rejects_disallowed_ip() {
    let handle = ensure_prometheus();
    let allow = MetricsIpAllowList::parse("127.0.0.1");
    let router = metrics_router(handle, allow).layer(MockConnectInfo(SocketAddr::from((
        [10, 0, 0, 42],
        12345,
    ))));

    let response = router
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn metrics_wildcard_allow_list_permits_all() {
    let handle = ensure_prometheus();
    let allow = MetricsIpAllowList::parse("*");
    let router = metrics_router(handle, allow).layer(MockConnectInfo(SocketAddr::from((
        [8, 8, 8, 8],
        4242,
    ))));

    let response = router
        .oneshot(Request::builder().uri("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
