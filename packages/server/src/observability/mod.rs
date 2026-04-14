//! Phase 10 — Observability stack for DarshJDB (slice 29/30).
//!
//! Provides three pillars:
//!
//! 1. **Prometheus metrics** ([`metrics`]): a `PrometheusHandle` stored on
//!    [`AppState`], a Tower middleware that observes every HTTP request,
//!    and a `/metrics` route guarded by an IP allow-list.
//! 2. **Health endpoints** ([`health`]): `/health`, `/ready`, `/live` that
//!    are mounted **before** the auth middleware so load balancers and
//!    orchestrators can probe liveness/readiness without a token.
//! 3. **Structured JSON logging** ([`logging`]): a `tracing_subscriber`
//!    configured with `json()` + span-list, plus a per-request middleware
//!    that generates a UUID `request_id`, injects it into the span, and
//!    echoes it back as the `X-Request-Id` response header.
//!
//! Created by Darshankumar Joshi.

pub mod health;
pub mod logging;
pub mod metrics;

pub use health::{health_router, live_handler, ready_handler, HealthState};
pub use logging::{init_json_logging, request_id_middleware};
pub use metrics::{
    http_metrics_middleware, init_prometheus, metrics_router, MetricsHandle, MetricsIpAllowList,
};
