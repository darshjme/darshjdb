//! Prometheus metrics surface for DarshJDB.
//!
//! This module wires [`metrics`] macros to a [`PrometheusBuilder`] handle that
//! is installed once at server startup and stored on [`AppState`] as a
//! [`MetricsHandle`]. A Tower middleware observes every HTTP request and
//! records both a counter (requests_total) and a histogram (latency_seconds).
//! A `/metrics` route renders the Prometheus text exposition format, but is
//! protected by [`MetricsIpAllowList`] to avoid leaking cardinality to the
//! internet.
//!
//! ## Metric names
//!
//! All metrics are namespaced under `ddb_`:
//!
//! | Name | Type | Labels | Description |
//! |------|------|--------|-------------|
//! | `ddb_http_requests_total`       | counter   | method,path,status | Total HTTP requests |
//! | `ddb_http_latency_seconds`      | histogram | method,path        | Request latency |
//! | `ddb_ws_connections_active`     | gauge     | —                  | Live WebSocket sessions |
//! | `ddb_ws_messages_total`         | counter   | type               | WebSocket messages |
//! | `ddb_query_duration_seconds`    | histogram | kind               | DarshJQL query duration |
//! | `ddb_triple_writes_total`       | counter   | —                  | Total triple writes |
//! | `ddb_triple_reads_total`        | counter   | —                  | Total triple reads |
//! | `ddb_cache_l1_hits_total`       | counter   | —                  | L1 cache hits |
//! | `ddb_cache_l1_misses_total`     | counter   | —                  | L1 cache misses |
//! | `ddb_cache_memory_bytes`        | gauge     | —                  | L1 cache memory usage |
//! | `ddb_agent_sessions_active`     | gauge     | —                  | Active AI agent sessions |
//! | `ddb_memory_entries_total`      | gauge     | tier               | Memory entries per tier |
//! | `ddb_embeddings_pending`        | gauge     | —                  | Pending embedding jobs |
//! | `ddb_embeddings_generated_total`| counter   | —                  | Embeddings generated |
//! | `ddb_memory_compressions_total` | counter   | —                  | Memory compression events |
//! | `ddb_tx_total`                  | counter   | —                  | Transactions committed |
//! | `ddb_tx_duration_seconds`       | histogram | —                  | Transaction durations |
//!
//! Created by Darshankumar Joshi.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use metrics::{counter, histogram};
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

/// Default IPs allowed to reach `/metrics` when `DDB_METRICS_ALLOWED_IPS` is unset.
pub const DEFAULT_METRICS_ALLOWED_IPS: &[&str] = &["127.0.0.1", "::1"];

/// Metric name constants — kept in one place so callers do not drift.
pub mod names {
    pub const HTTP_REQUESTS_TOTAL: &str = "ddb_http_requests_total";
    pub const HTTP_LATENCY_SECONDS: &str = "ddb_http_latency_seconds";
    pub const WS_CONNECTIONS_ACTIVE: &str = "ddb_ws_connections_active";
    pub const WS_MESSAGES_TOTAL: &str = "ddb_ws_messages_total";
    pub const QUERY_DURATION_SECONDS: &str = "ddb_query_duration_seconds";
    pub const TRIPLE_WRITES_TOTAL: &str = "ddb_triple_writes_total";
    pub const TRIPLE_READS_TOTAL: &str = "ddb_triple_reads_total";
    pub const CACHE_L1_HITS_TOTAL: &str = "ddb_cache_l1_hits_total";
    pub const CACHE_L1_MISSES_TOTAL: &str = "ddb_cache_l1_misses_total";
    pub const CACHE_MEMORY_BYTES: &str = "ddb_cache_memory_bytes";
    pub const AGENT_SESSIONS_ACTIVE: &str = "ddb_agent_sessions_active";
    pub const MEMORY_ENTRIES_TOTAL: &str = "ddb_memory_entries_total";
    pub const EMBEDDINGS_PENDING: &str = "ddb_embeddings_pending";
    pub const EMBEDDINGS_GENERATED_TOTAL: &str = "ddb_embeddings_generated_total";
    pub const MEMORY_COMPRESSIONS_TOTAL: &str = "ddb_memory_compressions_total";
    pub const TX_TOTAL: &str = "ddb_tx_total";
    pub const TX_DURATION_SECONDS: &str = "ddb_tx_duration_seconds";
}

/// Strongly-typed handle stored on [`AppState`] so any part of the server
/// can render the current Prometheus snapshot.
#[derive(Clone)]
pub struct MetricsHandle {
    pub handle: Arc<PrometheusHandle>,
}

impl MetricsHandle {
    /// Render the current Prometheus text exposition format.
    pub fn render(&self) -> String {
        self.handle.render()
    }
}

impl std::fmt::Debug for MetricsHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsHandle")
            .field("installed", &true)
            .finish()
    }
}

/// IP allow-list enforcement for `/metrics` scrapes.
///
/// Reads `DDB_METRICS_ALLOWED_IPS` (comma-separated). If unset, falls back to
/// localhost (`127.0.0.1, ::1`). `*` disables the allow-list entirely.
#[derive(Clone, Debug)]
pub struct MetricsIpAllowList {
    ips: Vec<IpAddr>,
    allow_all: bool,
}

impl MetricsIpAllowList {
    /// Build the allow-list from the environment.
    pub fn from_env() -> Self {
        match std::env::var("DDB_METRICS_ALLOWED_IPS") {
            Ok(raw) => Self::parse(&raw),
            Err(_) => Self::parse(&DEFAULT_METRICS_ALLOWED_IPS.join(",")),
        }
    }

    /// Parse a comma-separated list of IPs.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if raw == "*" {
            return Self {
                ips: Vec::new(),
                allow_all: true,
            };
        }
        let ips = raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .collect();
        Self {
            ips,
            allow_all: false,
        }
    }

    /// Returns true when the given peer address may scrape `/metrics`.
    pub fn is_allowed(&self, peer: IpAddr) -> bool {
        if self.allow_all {
            return true;
        }
        self.ips.iter().any(|allowed| *allowed == peer)
    }
}

/// Initialize the Prometheus recorder. Safe to call exactly once per process.
///
/// Returns a [`MetricsHandle`] that renders the current exposition output, a
/// pre-built [`MetricsIpAllowList`] for the `/metrics` route, and records the
/// initial descriptions for every known metric so they appear in output even
/// when no events have fired yet.
pub fn init_prometheus() -> Result<(MetricsHandle, MetricsIpAllowList), String> {
    let handle = PrometheusBuilder::new()
        .set_buckets_for_metric(
            metrics_exporter_prometheus::Matcher::Suffix("_seconds".to_string()),
            &[
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ],
        )
        .map_err(|e| format!("failed to set histogram buckets: {e}"))?
        .install_recorder()
        .map_err(|e| format!("failed to install Prometheus recorder: {e}"))?;

    describe_metrics();

    Ok((
        MetricsHandle {
            handle: Arc::new(handle),
        },
        MetricsIpAllowList::from_env(),
    ))
}

/// Register descriptions **and** materialize every metric in the Prometheus
/// registry so scrapes include them even before any event has fired.
///
/// `metrics-exporter-prometheus` only surfaces metrics that have been observed
/// at least once, so we fire a zero-valued call against each one to seed the
/// exposition output. A zero increment leaves counter/histogram values at 0
/// but registers the name with the exporter.
fn describe_metrics() {
    use metrics::{
        counter, describe_counter, describe_gauge, describe_histogram, gauge, histogram,
    };

    describe_counter!(names::HTTP_REQUESTS_TOTAL, "Total HTTP requests served");
    describe_histogram!(
        names::HTTP_LATENCY_SECONDS,
        "End-to-end HTTP request latency in seconds"
    );
    describe_gauge!(
        names::WS_CONNECTIONS_ACTIVE,
        "Number of currently connected WebSocket clients"
    );
    describe_counter!(
        names::WS_MESSAGES_TOTAL,
        "Total WebSocket messages exchanged"
    );
    describe_histogram!(
        names::QUERY_DURATION_SECONDS,
        "DarshJQL query duration in seconds"
    );
    describe_counter!(names::TRIPLE_WRITES_TOTAL, "Total triple writes");
    describe_counter!(names::TRIPLE_READS_TOTAL, "Total triple reads");
    describe_counter!(names::CACHE_L1_HITS_TOTAL, "L1 query-cache hits");
    describe_counter!(names::CACHE_L1_MISSES_TOTAL, "L1 query-cache misses");
    describe_gauge!(names::CACHE_MEMORY_BYTES, "L1 query-cache memory usage");
    describe_gauge!(names::AGENT_SESSIONS_ACTIVE, "Active AI agent sessions");
    describe_gauge!(
        names::MEMORY_ENTRIES_TOTAL,
        "Memory entries per tier (working/episodic/semantic)"
    );
    describe_gauge!(names::EMBEDDINGS_PENDING, "Pending embedding jobs");
    describe_counter!(
        names::EMBEDDINGS_GENERATED_TOTAL,
        "Total embeddings generated"
    );
    describe_counter!(
        names::MEMORY_COMPRESSIONS_TOTAL,
        "Total memory compression events"
    );
    describe_counter!(names::TX_TOTAL, "Total committed transactions");
    describe_histogram!(
        names::TX_DURATION_SECONDS,
        "Transaction duration in seconds"
    );

    // Materialize each metric in the registry with a zero-valued record.
    counter!(
        names::HTTP_REQUESTS_TOTAL,
        "method" => "STARTUP",
        "path" => "-",
        "status" => "0",
    )
    .increment(0);
    histogram!(
        names::HTTP_LATENCY_SECONDS,
        "method" => "STARTUP",
        "path" => "-",
    )
    .record(0.0);
    gauge!(names::WS_CONNECTIONS_ACTIVE).set(0.0);
    counter!(names::WS_MESSAGES_TOTAL, "type" => "-").increment(0);
    histogram!(names::QUERY_DURATION_SECONDS, "kind" => "-").record(0.0);
    counter!(names::TRIPLE_WRITES_TOTAL).increment(0);
    counter!(names::TRIPLE_READS_TOTAL).increment(0);
    counter!(names::CACHE_L1_HITS_TOTAL).increment(0);
    counter!(names::CACHE_L1_MISSES_TOTAL).increment(0);
    gauge!(names::CACHE_MEMORY_BYTES).set(0.0);
    gauge!(names::AGENT_SESSIONS_ACTIVE).set(0.0);
    gauge!(names::MEMORY_ENTRIES_TOTAL, "tier" => "working").set(0.0);
    gauge!(names::MEMORY_ENTRIES_TOTAL, "tier" => "episodic").set(0.0);
    gauge!(names::MEMORY_ENTRIES_TOTAL, "tier" => "semantic").set(0.0);
    gauge!(names::EMBEDDINGS_PENDING).set(0.0);
    counter!(names::EMBEDDINGS_GENERATED_TOTAL).increment(0);
    counter!(names::MEMORY_COMPRESSIONS_TOTAL).increment(0);
    counter!(names::TX_TOTAL).increment(0);
    histogram!(names::TX_DURATION_SECONDS).record(0.0);
}

/// Normalize a request path into a low-cardinality label value.
///
/// Prometheus cardinality explodes if every entity UUID becomes its own label
/// value. We keep at most the first two path segments (the static route
/// prefix) so `/api/entities/550e8400-...` collapses to `/api/entities`, and
/// also replace any segment that looks like a UUID or a pure integer with
/// `:id`. The goal is "route shape, not instance" — Prometheus labels must
/// be bounded cardinality.
fn normalize_path(path: &str) -> String {
    const MAX_SEGMENTS: usize = 2;
    let mut out = String::new();
    let mut kept = 0;
    for seg in path.split('/') {
        if seg.is_empty() {
            continue;
        }
        if kept >= MAX_SEGMENTS {
            break;
        }
        out.push('/');
        if looks_like_id(seg) {
            out.push_str(":id");
        } else {
            out.push_str(seg);
        }
        kept += 1;
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Rough heuristic — a segment is treated as an ID when it parses as a UUID,
/// is all digits, or is a long hex-ish string. Used only for label shaping.
fn looks_like_id(seg: &str) -> bool {
    if seg.len() == 36 && seg.chars().filter(|c| *c == '-').count() == 4 {
        return true;
    }
    if !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    false
}

/// Tower middleware that observes every HTTP request.
pub async fn http_metrics_middleware(req: Request<Body>, next: Next) -> Response {
    let method = req.method().clone();
    let path = normalize_path(req.uri().path());
    let start = Instant::now();

    let response = next.run(req).await;

    let elapsed = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    counter!(
        names::HTTP_REQUESTS_TOTAL,
        "method" => method.to_string(),
        "path" => path.clone(),
        "status" => status,
    )
    .increment(1);

    histogram!(
        names::HTTP_LATENCY_SECONDS,
        "method" => method.to_string(),
        "path" => path,
    )
    .record(elapsed);

    response
}

/// Shared state handed to the `/metrics` route.
#[derive(Clone)]
pub struct MetricsRouteState {
    pub handle: MetricsHandle,
    pub allow_list: MetricsIpAllowList,
}

/// `GET /metrics` — Prometheus text exposition format.
///
/// Guarded by an IP allow-list; unauthorized peers receive `403 Forbidden`.
pub async fn metrics_handler(
    State(state): State<MetricsRouteState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    if !state.allow_list.is_allowed(peer.ip()) {
        return (
            StatusCode::FORBIDDEN,
            [("content-type", "text/plain; charset=utf-8")],
            "metrics endpoint not available for this IP",
        )
            .into_response();
    }

    let body = state.handle.render();
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Build the `/metrics` sub-router.
///
/// The router is mounted at the root (not under `/api`) so Prometheus can
/// scrape without the API base path. It does **not** require authentication —
/// defense relies on the IP allow-list. Mount this before the auth middleware.
pub fn metrics_router(handle: MetricsHandle, allow_list: MetricsIpAllowList) -> Router {
    Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(MetricsRouteState { handle, allow_list })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_normalization_collapses_ids() {
        assert_eq!(
            normalize_path("/api/entities/550e8400-e29b-41d4-a716-446655440000"),
            "/api/entities"
        );
        assert_eq!(normalize_path("/api/query"), "/api/query");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("/health"), "/health");
    }

    #[test]
    fn allow_list_parses_defaults() {
        let list = MetricsIpAllowList::parse("127.0.0.1,::1");
        assert!(list.is_allowed("127.0.0.1".parse().unwrap()));
        assert!(list.is_allowed("::1".parse().unwrap()));
        assert!(!list.is_allowed("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn allow_list_star_allows_everything() {
        let list = MetricsIpAllowList::parse("*");
        assert!(list.is_allowed("10.0.0.1".parse().unwrap()));
        assert!(list.is_allowed("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn allow_list_skips_invalid_entries() {
        let list = MetricsIpAllowList::parse("not-an-ip,127.0.0.1,,");
        assert!(list.is_allowed("127.0.0.1".parse().unwrap()));
        assert!(!list.is_allowed("::1".parse().unwrap()));
    }
}
