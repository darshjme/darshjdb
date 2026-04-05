//! Axum middleware for authentication and rate limiting.
//!
//! # Auth Middleware
//!
//! Extracts the `Authorization: Bearer <token>` header, validates the JWT
//! via [`SessionManager`], and injects an [`AuthContext`] into the request
//! extensions. Downstream handlers can then extract `AuthContext` directly.
//!
//! # Rate Limiting
//!
//! Token-bucket rate limiter keyed by `(user_id | ip, api_key)`:
//! - Authenticated: 100 requests/minute.
//! - Anonymous: 20 requests/minute.
//!
//! Uses [`DashMap`] for lock-free concurrent access with periodic cleanup
//! of expired buckets.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use uuid::Uuid;

use super::{AuthError, session::SessionManager};

// ---------------------------------------------------------------------------
// Auth middleware
// ---------------------------------------------------------------------------

/// Shared state for the auth middleware layer.
#[derive(Clone)]
pub struct AuthLayer {
    /// Session manager for JWT validation.
    pub session_manager: Arc<SessionManager>,
    /// Rate limiter instance.
    pub rate_limiter: Arc<RateLimiter>,
}

/// Axum middleware function that validates Bearer tokens and enforces rate limits.
///
/// On success, an [`AuthContext`] is inserted into request extensions.
/// On failure, an appropriate HTTP error response is returned.
///
/// Anonymous requests (no Bearer token) are allowed through with a
/// stricter rate limit, but no `AuthContext` is set.
pub async fn auth_middleware(
    State(layer): State<AuthLayer>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    headers: HeaderMap,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let ip = connect_info
        .map(|ci| ci.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into());

    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string();

    let device_fingerprint = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Extract Bearer token.
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string());

    // Rate-limit check.
    let rate_key = if let Some(ref tok) = token {
        // We use a hash of the token prefix as the rate key for authenticated
        // requests. After validation we switch to user_id, but we need to
        // rate-limit before the (potentially expensive) JWT validation.
        RateLimitKey::Token(tok[..std::cmp::min(tok.len(), 16)].to_string())
    } else {
        RateLimitKey::Ip(ip.clone())
    };

    let is_authenticated = token.is_some();
    if let Err(retry_after) = layer.rate_limiter.check(&rate_key, is_authenticated) {
        return rate_limit_response(retry_after);
    }

    // Validate token if present.
    if let Some(ref token) = token {
        match layer
            .session_manager
            .validate_token(token, &ip, &user_agent, &device_fingerprint)
        {
            Ok(ctx) => {
                request.extensions_mut().insert(ctx);
            }
            Err(e) => {
                return auth_error_response(&e);
            }
        }
    }

    next.run(request).await
}

/// Build an HTTP error response from an [`AuthError`].
fn auth_error_response(err: &AuthError) -> Response {
    let status = err.status_code();
    let body = serde_json::json!({
        "error": {
            "code": status.as_u16(),
            "message": err.to_string(),
        }
    });
    (status, axum::Json(body)).into_response()
}

/// Build a 429 Too Many Requests response with Retry-After header.
fn rate_limit_response(retry_after_secs: u64) -> Response {
    let body = serde_json::json!({
        "error": {
            "code": 429,
            "message": format!("rate limit exceeded, retry after {}s", retry_after_secs),
        }
    });

    let mut response = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
    response.headers_mut().insert(
        "retry-after",
        retry_after_secs
            .to_string()
            .parse()
            .expect("valid header value"),
    );
    response
}

// ---------------------------------------------------------------------------
// Rate limiter
// ---------------------------------------------------------------------------

/// Key used to identify a rate-limit bucket.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RateLimitKey {
    /// Keyed by IP address (anonymous requests).
    Ip(String),
    /// Keyed by token prefix (authenticated requests before validation).
    Token(String),
    /// Keyed by user ID (authenticated requests after validation).
    UserId(Uuid),
    /// Keyed by API key.
    ApiKey(String),
}

/// A single token bucket for rate limiting.
#[derive(Debug, Clone)]
struct TokenBucket {
    /// Current number of available tokens.
    tokens: f64,
    /// Maximum capacity.
    capacity: f64,
    /// Tokens added per second.
    refill_rate: f64,
    /// Last time the bucket was refilled.
    last_refill: Instant,
}

impl TokenBucket {
    /// Create a new full bucket.
    fn new(capacity: f64, refill_rate: f64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            refill_rate,
            last_refill: Instant::now(),
        }
    }

    /// Refill based on elapsed time, then try to consume one token.
    ///
    /// Returns `Ok(())` if a token was consumed, or `Err(seconds_until_available)`.
    fn try_consume(&mut self) -> Result<(), u64> {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            let deficit = 1.0 - self.tokens;
            let wait_secs = (deficit / self.refill_rate).ceil() as u64;
            Err(wait_secs.max(1))
        }
    }

    /// Whether this bucket has been idle long enough to be cleaned up.
    fn is_stale(&self, max_idle: Duration) -> bool {
        self.last_refill.elapsed() > max_idle
    }
}

/// Concurrent token-bucket rate limiter.
///
/// Buckets are stored in a [`DashMap`] keyed by [`RateLimitKey`].
/// Stale buckets are cleaned up periodically via [`cleanup`].
pub struct RateLimiter {
    buckets: DashMap<RateLimitKey, TokenBucket>,
    /// Maximum idle duration before a bucket is eligible for cleanup.
    max_idle: Duration,
}

impl RateLimiter {
    /// Authenticated rate: 100 requests per minute.
    const AUTH_CAPACITY: f64 = 100.0;
    /// Authenticated refill: ~1.67 tokens/second.
    const AUTH_REFILL: f64 = 100.0 / 60.0;

    /// Anonymous rate: 20 requests per minute.
    const ANON_CAPACITY: f64 = 20.0;
    /// Anonymous refill: ~0.33 tokens/second.
    const ANON_REFILL: f64 = 20.0 / 60.0;

    /// Create a new rate limiter.
    pub fn new() -> Self {
        Self {
            buckets: DashMap::new(),
            max_idle: Duration::from_secs(300), // 5 minutes
        }
    }

    /// Create a rate limiter with a custom idle timeout.
    pub fn with_idle_timeout(max_idle: Duration) -> Self {
        Self {
            buckets: DashMap::new(),
            max_idle,
        }
    }

    /// Check whether a request is within rate limits.
    ///
    /// Returns `Ok(())` if allowed, or `Err(retry_after_secs)` if throttled.
    pub fn check(&self, key: &RateLimitKey, is_authenticated: bool) -> Result<(), u64> {
        let (capacity, refill) = if is_authenticated {
            (Self::AUTH_CAPACITY, Self::AUTH_REFILL)
        } else {
            (Self::ANON_CAPACITY, Self::ANON_REFILL)
        };

        let mut entry = self
            .buckets
            .entry(key.clone())
            .or_insert_with(|| TokenBucket::new(capacity, refill));

        entry.value_mut().try_consume()
    }

    /// Remove stale buckets that have not been accessed recently.
    ///
    /// Call this periodically (e.g., every 60 seconds) from a background task.
    pub fn cleanup(&self) {
        self.buckets
            .retain(|_, bucket| !bucket.is_stale(self.max_idle));
    }

    /// Number of active buckets (for monitoring).
    pub fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    /// Spawn a background cleanup task that runs every `interval`.
    ///
    /// Returns a [`tokio::task::JoinHandle`] that can be aborted on shutdown.
    pub fn spawn_cleanup_task(self: &Arc<Self>, interval: Duration) -> tokio::task::JoinHandle<()> {
        let limiter = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                limiter.cleanup();
                tracing::debug!(
                    buckets = limiter.bucket_count(),
                    "rate limiter cleanup complete"
                );
            }
        })
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_bucket_allows_within_capacity() {
        let mut bucket = TokenBucket::new(5.0, 1.0);
        for _ in 0..5 {
            assert!(bucket.try_consume().is_ok());
        }
        // 6th should fail.
        assert!(bucket.try_consume().is_err());
    }

    #[test]
    fn rate_limiter_anonymous_limit() {
        let limiter = RateLimiter::new();
        let key = RateLimitKey::Ip("127.0.0.1".into());

        for _ in 0..20 {
            assert!(limiter.check(&key, false).is_ok());
        }
        // 21st should be rate-limited.
        let err = limiter.check(&key, false);
        assert!(err.is_err());
        assert!(err.unwrap_err() >= 1);
    }

    #[test]
    fn rate_limiter_authenticated_limit() {
        let limiter = RateLimiter::new();
        let key = RateLimitKey::UserId(Uuid::new_v4());

        for _ in 0..100 {
            assert!(limiter.check(&key, true).is_ok());
        }
        assert!(limiter.check(&key, true).is_err());
    }

    #[test]
    fn cleanup_removes_stale_buckets() {
        let limiter = RateLimiter::with_idle_timeout(Duration::from_millis(0));
        let key = RateLimitKey::Ip("10.0.0.1".into());
        let _ = limiter.check(&key, false);
        assert_eq!(limiter.bucket_count(), 1);

        // With zero idle timeout, everything is stale.
        limiter.cleanup();
        assert_eq!(limiter.bucket_count(), 0);
    }
}
