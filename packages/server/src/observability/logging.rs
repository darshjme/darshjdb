//! Structured JSON logging + request-id propagation.
//!
//! Replaces the legacy `tracing_subscriber::fmt().with_env_filter(...)` setup
//! with a JSON formatter that emits one log line per span/event, including:
//!
//! - `request_id` — a UUIDv4 generated at the edge of the HTTP pipeline.
//! - `method`, `path`, `status`, `duration_ms` — standard request fields.
//! - `user_id`, `session_id` — populated only after auth middleware attaches
//!   an [`AuthContext`] to the request extensions.
//!
//! A Tower middleware ([`request_id_middleware`]) owns the lifecycle:
//!
//! 1. On request arrival it generates a fresh UUID, stashes it in a
//!    per-request [`tracing::Span`] field, and also inserts it as a request
//!    extension so downstream handlers can read it.
//! 2. On response it copies the UUID into the `X-Request-Id` response header.
//!
//! Created by Darshankumar Joshi.

use std::time::Instant;

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

use crate::auth::AuthContext;

/// Name of the header that echoes the request ID back to clients.
pub const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Request extension wrapper so handlers can fetch the current request ID.
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Initialize the global tracing subscriber with JSON output.
///
/// Safe to call once per process. Honors `RUST_LOG` via [`EnvFilter`] and
/// defaults to `info`. Unlike `fmt()`, the JSON layer emits the span list on
/// every event, so downstream log aggregators see both the top-level request
/// span and any child spans that fired during the request.
///
/// Returns `Ok(())` if the subscriber was installed successfully. If the
/// global subscriber is already set (e.g. by a test harness), this is a
/// no-op that returns `Ok(())` — callers should treat re-initialization as
/// success so tests that spawn multiple servers do not panic.
pub fn init_json_logging() -> Result<(), String> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let builder = tracing_subscriber::fmt()
        .json()
        .with_current_span(true)
        .with_span_list(true)
        .with_env_filter(filter);

    // `try_init` is preferred over `init` because it returns an error instead
    // of panicking when a subscriber is already installed (common in tests).
    match builder.try_init() {
        Ok(()) => Ok(()),
        Err(e) => {
            // A subscriber is already installed — we do not treat this as a
            // hard error because production startup and tests should both
            // succeed. We log to stderr so the operator still sees the hint.
            eprintln!(
                "observability: json logging already initialized ({e}); continuing"
            );
            Ok(())
        }
    }
}

/// Tower middleware that generates a request_id, opens a tracing span, and
/// echoes the id back as `X-Request-Id`.
///
/// The span carries the method, path, user_id (`-` until auth attaches a
/// context), session_id, and duration_ms. All downstream `tracing::info!`
/// calls inherit the request_id automatically via span context.
pub async fn request_id_middleware(mut req: Request<Body>, next: Next) -> Response {
    // Honor an existing upstream request id if the caller supplied one — this
    // lets proxies forward a trace id end-to-end.
    let incoming = req
        .headers()
        .get(&REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let request_id = incoming.unwrap_or_else(|| Uuid::new_v4().to_string());

    req.extensions_mut().insert(RequestId(request_id.clone()));

    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let auth_ctx = req.extensions().get::<AuthContext>().cloned();
    let user_id = auth_ctx
        .as_ref()
        .map(|c| c.user_id.to_string())
        .unwrap_or_else(|| "-".to_string());
    let session_id = auth_ctx
        .as_ref()
        .map(|c| c.session_id.to_string())
        .unwrap_or_else(|| "-".to_string());

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        method = %method,
        path = %path,
        user_id = %user_id,
        session_id = %session_id,
        status = tracing::field::Empty,
        duration_ms = tracing::field::Empty,
    );

    let start = Instant::now();
    let mut response = async move { next.run(req).await }.instrument(span.clone()).await;
    let duration_ms = start.elapsed().as_millis() as u64;
    let status = response.status().as_u16();

    span.record("status", status);
    span.record("duration_ms", duration_ms);

    tracing::info!(
        parent: &span,
        status,
        duration_ms,
        "request completed"
    );

    if let Ok(header_value) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, header_value);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::middleware::from_fn;
    use axum::routing::get;
    use axum::Router;
    use tower::util::ServiceExt;

    async fn noop() -> StatusCode {
        StatusCode::OK
    }

    fn test_app() -> Router {
        Router::new()
            .route("/", get(noop))
            .layer(from_fn(request_id_middleware))
    }

    #[tokio::test]
    async fn middleware_generates_request_id_header() {
        let app = test_app();
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let header = response.headers().get(&REQUEST_ID_HEADER).unwrap();
        let value = header.to_str().unwrap();
        // UUIDs are 36 chars with hyphens.
        assert_eq!(value.len(), 36);
        assert!(Uuid::parse_str(value).is_ok());
    }

    #[tokio::test]
    async fn middleware_preserves_upstream_request_id() {
        let app = test_app();
        let upstream = "abc-123-upstream";
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(&REQUEST_ID_HEADER, upstream)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let header = response.headers().get(&REQUEST_ID_HEADER).unwrap();
        assert_eq!(header.to_str().unwrap(), upstream);
    }
}
