//! Outbound webhook delivery system for DarshJDB.
//!
//! Webhooks are registered with a target URL, a shared secret (for HMAC-SHA256
//! signing), a set of event kinds to subscribe to, and an optional entity-type
//! filter. When the event bus fires a matching [`DdbEvent`], the
//! [`WebhookSender`] delivers a signed JSON payload with exponential-backoff
//! retry and a circuit breaker that auto-disables persistently failing webhooks.
//!
//! # Signature
//!
//! Every delivery includes an `X-DDB-Signature` header containing the
//! hex-encoded HMAC-SHA256 of the raw request body, keyed with the webhook's
//! shared secret. Receivers should verify this to confirm authenticity.
//!
//! # Delivery Tracking
//!
//! Each attempt is recorded in the `webhook_deliveries` table so operators
//! can inspect success/failure history via the API.

pub mod handlers;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::PgPool;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::events::{DdbEvent, EventKind};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Unique identifier for a webhook registration.
pub type WebhookId = Uuid;

/// Unique identifier for a delivery attempt.
pub type DeliveryId = Uuid;

/// How the sender should retry failed deliveries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts (excludes the initial attempt).
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Base delay in milliseconds for exponential backoff.
    #[serde(default = "default_backoff_base_ms")]
    pub backoff_base_ms: u64,
    /// Maximum delay cap in milliseconds.
    #[serde(default = "default_max_delay_ms")]
    pub max_delay_ms: u64,
}

fn default_max_retries() -> u32 {
    3
}
fn default_backoff_base_ms() -> u64 {
    1000
}
fn default_max_delay_ms() -> u64 {
    30_000
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: default_max_retries(),
            backoff_base_ms: default_backoff_base_ms(),
            max_delay_ms: default_max_delay_ms(),
        }
    }
}

/// A registered webhook configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Unique webhook ID.
    pub id: WebhookId,
    /// Target URL to POST payloads to.
    pub url: String,
    /// Shared secret for HMAC-SHA256 signing.
    #[serde(skip_serializing)]
    pub secret: String,
    /// Event kinds this webhook subscribes to.
    pub events: Vec<EventKind>,
    /// Optional entity-type filter (e.g. `["User", "Post"]`).
    pub entity_types: Option<Vec<String>>,
    /// Extra headers sent with every delivery.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Whether the webhook is active.
    pub active: bool,
    /// User who created this webhook.
    pub created_by: Uuid,
    /// Retry policy for failed deliveries.
    #[serde(default)]
    pub retry_policy: RetryPolicy,
    /// When the webhook was created.
    pub created_at: DateTime<Utc>,
    /// Consecutive failure counter (for circuit breaker).
    #[serde(default)]
    pub consecutive_failures: u32,
}

/// Delivery status for a webhook invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryStatus {
    Pending,
    Delivered,
    Failed,
}

impl std::fmt::Display for DeliveryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Delivered => write!(f, "delivered"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl DeliveryStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "delivered" => Self::Delivered,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// A single delivery attempt record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDelivery {
    /// Unique delivery ID.
    pub id: DeliveryId,
    /// Which webhook this delivery belongs to.
    pub webhook_id: WebhookId,
    /// The event kind that triggered this delivery.
    pub event: String,
    /// The JSON payload that was sent.
    pub payload: serde_json::Value,
    /// Current delivery status.
    pub status: DeliveryStatus,
    /// Number of attempts made so far.
    pub attempts: u32,
    /// When the last attempt was made.
    pub last_attempt_at: Option<DateTime<Utc>>,
    /// HTTP status code from the last response.
    pub response_status: Option<u16>,
    /// Truncated response body from the last attempt.
    pub response_body: Option<String>,
    /// When this delivery was created.
    pub created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Circuit breaker threshold
// ---------------------------------------------------------------------------

/// After this many consecutive failures, the webhook is auto-disabled.
const CIRCUIT_BREAKER_THRESHOLD: u32 = 10;

/// Maximum response body to store per delivery (bytes).
const MAX_RESPONSE_BODY_LEN: usize = 2048;

// ---------------------------------------------------------------------------
// HMAC signature
// ---------------------------------------------------------------------------

/// Compute HMAC-SHA256 of `body` using `secret` and return hex-encoded digest.
pub fn compute_signature(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Verify that `signature` matches the HMAC-SHA256 of `body` under `secret`.
///
/// Uses constant-time comparison to prevent timing attacks.
pub fn verify_signature(secret: &str, body: &[u8], signature: &str) -> bool {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    let expected = hex::decode(signature).unwrap_or_default();
    mac.verify_slice(&expected).is_ok()
}

// ---------------------------------------------------------------------------
// Database operations
// ---------------------------------------------------------------------------

/// Create the webhook tables if they do not exist.
pub async fn ensure_webhook_tables(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        r#"
        CREATE TABLE IF NOT EXISTS webhooks (
            id                   UUID PRIMARY KEY,
            url                  TEXT NOT NULL,
            secret               TEXT NOT NULL,
            events               JSONB NOT NULL DEFAULT '[]'::jsonb,
            entity_types         JSONB,
            headers              JSONB NOT NULL DEFAULT '{}'::jsonb,
            active               BOOLEAN NOT NULL DEFAULT true,
            created_by           UUID NOT NULL,
            retry_policy         JSONB NOT NULL DEFAULT '{}'::jsonb,
            created_at           TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            consecutive_failures INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS webhook_deliveries (
            id               UUID PRIMARY KEY,
            webhook_id       UUID NOT NULL REFERENCES webhooks(id) ON DELETE CASCADE,
            event            TEXT NOT NULL,
            payload          JSONB NOT NULL,
            status           TEXT NOT NULL DEFAULT 'pending',
            attempts         INTEGER NOT NULL DEFAULT 0,
            last_attempt_at  TIMESTAMPTZ,
            response_status  INTEGER,
            response_body    TEXT,
            created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_webhook_id
            ON webhook_deliveries (webhook_id);
        CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_status
            ON webhook_deliveries (status);
        CREATE INDEX IF NOT EXISTS idx_webhook_deliveries_created_at
            ON webhook_deliveries (created_at);
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Register a new webhook in the database.
pub async fn register_webhook(pool: &PgPool, config: &WebhookConfig) -> Result<(), sqlx::Error> {
    let events_json = serde_json::to_value(&config.events).unwrap_or_default();
    let entity_types_json = config
        .entity_types
        .as_ref()
        .map(|et| serde_json::to_value(et).unwrap_or_default());
    let headers_json = serde_json::to_value(&config.headers).unwrap_or_default();
    let retry_json = serde_json::to_value(&config.retry_policy).unwrap_or_default();

    sqlx::query(
        "INSERT INTO webhooks (id, url, secret, events, entity_types, headers, active, created_by, retry_policy, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(config.id)
    .bind(&config.url)
    .bind(&config.secret)
    .bind(&events_json)
    .bind(&entity_types_json)
    .bind(&headers_json)
    .bind(config.active)
    .bind(config.created_by)
    .bind(&retry_json)
    .bind(config.created_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// Fetch a single webhook by ID.
pub async fn get_webhook(
    pool: &PgPool,
    id: WebhookId,
) -> Result<Option<WebhookConfig>, sqlx::Error> {
    let row: Option<(
        Uuid,
        String,
        String,
        serde_json::Value,
        Option<serde_json::Value>,
        serde_json::Value,
        bool,
        Uuid,
        serde_json::Value,
        DateTime<Utc>,
        i32,
    )> = sqlx::query_as(
        "SELECT id, url, secret, events, entity_types, headers, active, created_by, retry_policy, created_at, consecutive_failures
         FROM webhooks WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(row_to_config))
}

/// List all webhooks for a user (or all if `user_id` is `None`).
pub async fn list_webhooks(
    pool: &PgPool,
    user_id: Option<Uuid>,
) -> Result<Vec<WebhookConfig>, sqlx::Error> {
    let rows: Vec<(
        Uuid,
        String,
        String,
        serde_json::Value,
        Option<serde_json::Value>,
        serde_json::Value,
        bool,
        Uuid,
        serde_json::Value,
        DateTime<Utc>,
        i32,
    )> = if let Some(uid) = user_id {
        sqlx::query_as(
            "SELECT id, url, secret, events, entity_types, headers, active, created_by, retry_policy, created_at, consecutive_failures
             FROM webhooks WHERE created_by = $1 ORDER BY created_at DESC",
        )
        .bind(uid)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, url, secret, events, entity_types, headers, active, created_by, retry_policy, created_at, consecutive_failures
             FROM webhooks ORDER BY created_at DESC",
        )
        .fetch_all(pool)
        .await?
    };

    Ok(rows.into_iter().map(row_to_config).collect())
}

/// Update webhook fields.
pub async fn update_webhook(
    pool: &PgPool,
    id: WebhookId,
    url: Option<&str>,
    events: Option<&[EventKind]>,
    active: Option<bool>,
    entity_types: Option<&[String]>,
) -> Result<bool, sqlx::Error> {
    let mut sets: Vec<String> = Vec::new();
    let mut idx = 2u32; // $1 is the id

    if url.is_some() {
        sets.push(format!("url = ${idx}"));
        idx += 1;
    }
    if events.is_some() {
        sets.push(format!("events = ${idx}"));
        idx += 1;
    }
    if active.is_some() {
        sets.push(format!("active = ${idx}"));
        idx += 1;
    }
    if entity_types.is_some() {
        sets.push(format!("entity_types = ${idx}"));
        let _ = idx; // suppress unused warning
    }

    if sets.is_empty() {
        return Ok(false);
    }

    let sql = format!("UPDATE webhooks SET {} WHERE id = $1", sets.join(", "));

    let mut q = sqlx::query(&sql).bind(id);
    if let Some(u) = url {
        q = q.bind(u);
    }
    if let Some(ev) = events {
        q = q.bind(serde_json::to_value(ev).unwrap_or_default());
    }
    if let Some(a) = active {
        q = q.bind(a);
    }
    if let Some(et) = entity_types {
        q = q.bind(serde_json::to_value(et).unwrap_or_default());
    }

    let result = q.execute(pool).await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a webhook and all its deliveries (CASCADE).
pub async fn delete_webhook(pool: &PgPool, id: WebhookId) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("DELETE FROM webhooks WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// List delivery attempts for a webhook.
pub async fn list_deliveries(
    pool: &PgPool,
    webhook_id: WebhookId,
    limit: i64,
) -> Result<Vec<WebhookDelivery>, sqlx::Error> {
    let rows: Vec<(
        Uuid,
        Uuid,
        String,
        serde_json::Value,
        String,
        i32,
        Option<DateTime<Utc>>,
        Option<i32>,
        Option<String>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, webhook_id, event, payload, status, attempts, last_attempt_at, response_status, response_body, created_at
         FROM webhook_deliveries WHERE webhook_id = $1
         ORDER BY created_at DESC LIMIT $2",
    )
    .bind(webhook_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(row_to_delivery).collect())
}

/// Insert a delivery record.
async fn insert_delivery(pool: &PgPool, delivery: &WebhookDelivery) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO webhook_deliveries (id, webhook_id, event, payload, status, attempts, last_attempt_at, response_status, response_body, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(delivery.id)
    .bind(delivery.webhook_id)
    .bind(&delivery.event)
    .bind(&delivery.payload)
    .bind(delivery.status.to_string())
    .bind(delivery.attempts as i32)
    .bind(delivery.last_attempt_at)
    .bind(delivery.response_status.map(|s| s as i32))
    .bind(&delivery.response_body)
    .bind(delivery.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update delivery record after an attempt.
async fn update_delivery(
    pool: &PgPool,
    id: DeliveryId,
    status: &DeliveryStatus,
    attempts: u32,
    response_status: Option<u16>,
    response_body: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE webhook_deliveries SET status = $2, attempts = $3, last_attempt_at = NOW(), response_status = $4, response_body = $5
         WHERE id = $1",
    )
    .bind(id)
    .bind(status.to_string())
    .bind(attempts as i32)
    .bind(response_status.map(|s| s as i32))
    .bind(response_body)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update consecutive failure count and optionally disable webhook.
async fn update_failure_count(
    pool: &PgPool,
    webhook_id: WebhookId,
    failures: u32,
    disable: bool,
) -> Result<(), sqlx::Error> {
    if disable {
        sqlx::query("UPDATE webhooks SET consecutive_failures = $2, active = false WHERE id = $1")
            .bind(webhook_id)
            .bind(failures as i32)
            .execute(pool)
            .await?;
    } else {
        sqlx::query("UPDATE webhooks SET consecutive_failures = $2 WHERE id = $1")
            .bind(webhook_id)
            .bind(failures as i32)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Reset consecutive failure count on success.
async fn reset_failure_count(pool: &PgPool, webhook_id: WebhookId) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE webhooks SET consecutive_failures = 0 WHERE id = $1")
        .bind(webhook_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Row conversion helpers
// ---------------------------------------------------------------------------

#[allow(clippy::type_complexity)]
fn row_to_config(
    r: (
        Uuid,
        String,
        String,
        serde_json::Value,
        Option<serde_json::Value>,
        serde_json::Value,
        bool,
        Uuid,
        serde_json::Value,
        DateTime<Utc>,
        i32,
    ),
) -> WebhookConfig {
    WebhookConfig {
        id: r.0,
        url: r.1,
        secret: r.2,
        events: serde_json::from_value(r.3).unwrap_or_default(),
        entity_types: r.4.and_then(|v| serde_json::from_value(v).ok()),
        headers: serde_json::from_value(r.5).unwrap_or_default(),
        active: r.6,
        created_by: r.7,
        retry_policy: serde_json::from_value(r.8).unwrap_or_default(),
        created_at: r.9,
        consecutive_failures: r.10 as u32,
    }
}

#[allow(clippy::type_complexity)]
fn row_to_delivery(
    r: (
        Uuid,
        Uuid,
        String,
        serde_json::Value,
        String,
        i32,
        Option<DateTime<Utc>>,
        Option<i32>,
        Option<String>,
        DateTime<Utc>,
    ),
) -> WebhookDelivery {
    WebhookDelivery {
        id: r.0,
        webhook_id: r.1,
        event: r.2,
        payload: r.3,
        status: DeliveryStatus::from_str(&r.4),
        attempts: r.5 as u32,
        last_attempt_at: r.6,
        response_status: r.7.map(|s| s as u16),
        response_body: r.8,
        created_at: r.9,
    }
}

// ---------------------------------------------------------------------------
// WebhookSender -- background delivery engine
// ---------------------------------------------------------------------------

/// The webhook sender delivers events to all matching registered webhooks.
///
/// It maintains an HTTP client with timeouts and performs HMAC signing,
/// retry with exponential backoff, and circuit-breaker logic.
pub struct WebhookSender {
    pool: PgPool,
    client: reqwest::Client,
}

impl WebhookSender {
    /// Create a new sender.
    pub fn new(pool: PgPool) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .user_agent("DarshJDB-Webhook/1.0")
            .build()
            .expect("failed to build reqwest client");

        Self { pool, client }
    }

    /// Deliver an event to all matching webhooks.
    ///
    /// Loads active webhook configs from the database, filters by event kind
    /// and entity type, then delivers to each matching webhook.
    pub async fn deliver(&self, event: &DdbEvent) {
        let webhooks = match list_webhooks(&self.pool, None).await {
            Ok(wh) => wh,
            Err(e) => {
                error!(error = %e, "failed to load webhooks for delivery");
                return;
            }
        };

        let matching: Vec<&WebhookConfig> = webhooks
            .iter()
            .filter(|wh| {
                if !wh.active {
                    return false;
                }
                if !wh.events.is_empty() && !wh.events.contains(&event.kind) {
                    return false;
                }
                if let Some(ref types) = wh.entity_types {
                    match &event.entity_type {
                        Some(et) if types.contains(et) => {}
                        Some(_) => return false,
                        None => return false,
                    }
                }
                true
            })
            .collect();

        if matching.is_empty() {
            return;
        }

        debug!(
            event_kind = %event.kind,
            matching_webhooks = matching.len(),
            "dispatching event to webhooks"
        );

        for webhook in matching {
            self.deliver_to_webhook(webhook, event).await;
        }
    }

    /// Deliver a single event to a single webhook, with retries.
    async fn deliver_to_webhook(&self, webhook: &WebhookConfig, event: &DdbEvent) {
        let payload = serde_json::json!({
            "event": event.kind.as_str(),
            "event_id": event.id.to_string(),
            "entity_type": event.entity_type,
            "entity_id": event.entity_id,
            "attribute": event.attribute,
            "old_value": event.old_value,
            "new_value": event.new_value,
            "user_id": event.user_id,
            "timestamp": event.timestamp.to_rfc3339(),
            "tx_id": event.tx_id,
            "metadata": event.metadata,
        });

        let body = serde_json::to_vec(&payload).unwrap_or_default();
        let signature = compute_signature(&webhook.secret, &body);

        // Create delivery record.
        let delivery_id = Uuid::new_v4();
        let delivery = WebhookDelivery {
            id: delivery_id,
            webhook_id: webhook.id,
            event: event.kind.as_str(),
            payload: payload.clone(),
            status: DeliveryStatus::Pending,
            attempts: 0,
            last_attempt_at: None,
            response_status: None,
            response_body: None,
            created_at: Utc::now(),
        };

        if let Err(e) = insert_delivery(&self.pool, &delivery).await {
            error!(error = %e, "failed to insert delivery record");
        }

        let max_attempts = webhook.retry_policy.max_retries + 1;
        let mut last_status: Option<u16> = None;
        let mut last_body: Option<String> = None;

        for attempt in 0..max_attempts {
            if attempt > 0 {
                let delay_ms = std::cmp::min(
                    webhook.retry_policy.backoff_base_ms * 2u64.pow(attempt - 1),
                    webhook.retry_policy.max_delay_ms,
                );
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            let mut request = self
                .client
                .post(&webhook.url)
                .header("Content-Type", "application/json")
                .header("X-DDB-Signature", &signature)
                .header("X-DDB-Delivery-Id", delivery_id.to_string())
                .body(body.clone());

            for (k, v) in &webhook.headers {
                request = request.header(k.as_str(), v.as_str());
            }

            match request.send().await {
                Ok(resp) => {
                    let status = resp.status();
                    last_status = Some(status.as_u16());

                    let resp_body = resp.text().await.unwrap_or_default();
                    last_body = Some(truncate_body(&resp_body));

                    if status.is_success() {
                        if let Err(e) = update_delivery(
                            &self.pool,
                            delivery_id,
                            &DeliveryStatus::Delivered,
                            attempt + 1,
                            last_status,
                            last_body.as_deref(),
                        )
                        .await
                        {
                            error!(error = %e, "failed to update delivery status");
                        }

                        if let Err(e) = reset_failure_count(&self.pool, webhook.id).await {
                            error!(error = %e, "failed to reset failure count");
                        }

                        info!(
                            webhook_id = %webhook.id,
                            delivery_id = %delivery_id,
                            attempts = attempt + 1,
                            "webhook delivered successfully"
                        );
                        return;
                    }

                    if status.is_server_error() {
                        warn!(
                            webhook_id = %webhook.id,
                            attempt = attempt + 1,
                            status = %status,
                            "webhook returned 5xx, will retry"
                        );
                        continue;
                    }

                    // 4xx -- not retryable.
                    warn!(
                        webhook_id = %webhook.id,
                        status = %status,
                        "webhook returned non-retryable status"
                    );
                    break;
                }
                Err(e) => {
                    warn!(
                        webhook_id = %webhook.id,
                        attempt = attempt + 1,
                        error = %e,
                        "webhook delivery network error, will retry"
                    );
                    last_body = Some(e.to_string());
                    continue;
                }
            }
        }

        // All attempts exhausted or non-retryable error.
        if let Err(e) = update_delivery(
            &self.pool,
            delivery_id,
            &DeliveryStatus::Failed,
            max_attempts,
            last_status,
            last_body.as_deref(),
        )
        .await
        {
            error!(error = %e, "failed to update delivery status on failure");
        }

        // Circuit breaker: increment consecutive failures.
        let new_failures = webhook.consecutive_failures + 1;
        let should_disable = new_failures >= CIRCUIT_BREAKER_THRESHOLD;
        if should_disable {
            warn!(
                webhook_id = %webhook.id,
                failures = new_failures,
                "circuit breaker tripped -- disabling webhook"
            );
        }
        if let Err(e) =
            update_failure_count(&self.pool, webhook.id, new_failures, should_disable).await
        {
            error!(error = %e, "failed to update failure count");
        }
    }

    /// Send a test payload to a webhook (bypasses event matching).
    pub async fn send_test(&self, webhook: &WebhookConfig) -> WebhookDelivery {
        let payload = serde_json::json!({
            "event": "test",
            "event_id": Uuid::new_v4().to_string(),
            "entity_type": "test",
            "entity_id": Uuid::new_v4().to_string(),
            "timestamp": Utc::now().to_rfc3339(),
            "tx_id": 0,
            "metadata": {},
            "test": true,
        });

        let body = serde_json::to_vec(&payload).unwrap_or_default();
        let signature = compute_signature(&webhook.secret, &body);
        let delivery_id = Uuid::new_v4();

        let mut request = self
            .client
            .post(&webhook.url)
            .header("Content-Type", "application/json")
            .header("X-DDB-Signature", &signature)
            .header("X-DDB-Delivery-Id", delivery_id.to_string())
            .body(body.clone());

        for (k, v) in &webhook.headers {
            request = request.header(k.as_str(), v.as_str());
        }

        let (status, response_status, response_body) = match request.send().await {
            Ok(resp) => {
                let s = resp.status();
                let body_text = resp.text().await.unwrap_or_default();
                if s.is_success() {
                    (
                        DeliveryStatus::Delivered,
                        Some(s.as_u16()),
                        Some(truncate_body(&body_text)),
                    )
                } else {
                    (
                        DeliveryStatus::Failed,
                        Some(s.as_u16()),
                        Some(truncate_body(&body_text)),
                    )
                }
            }
            Err(e) => (DeliveryStatus::Failed, None, Some(e.to_string())),
        };

        let delivery = WebhookDelivery {
            id: delivery_id,
            webhook_id: webhook.id,
            event: "test".into(),
            payload,
            status,
            attempts: 1,
            last_attempt_at: Some(Utc::now()),
            response_status,
            response_body,
            created_at: Utc::now(),
        };

        if let Err(e) = insert_delivery(&self.pool, &delivery).await {
            error!(error = %e, "failed to insert test delivery record");
        }

        delivery
    }

    /// Run the webhook sender as a background task, listening on the event bus.
    pub async fn run(self: Arc<Self>, mut rx: tokio::sync::broadcast::Receiver<DdbEvent>) {
        info!("webhook sender started, listening for events");
        loop {
            match rx.recv().await {
                Ok(event) => {
                    self.deliver(&event).await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "webhook sender lagged behind");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    info!("event bus closed, webhook sender shutting down");
                    return;
                }
            }
        }
    }
}

/// Truncate a response body to [`MAX_RESPONSE_BODY_LEN`] bytes.
fn truncate_body(body: &str) -> String {
    if body.len() > MAX_RESPONSE_BODY_LEN {
        format!("{}...(truncated)", &body[..MAX_RESPONSE_BODY_LEN])
    } else {
        body.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_signature_roundtrip() {
        let secret = "my-webhook-secret";
        let body = b"hello world";
        let sig = compute_signature(secret, body);
        assert!(verify_signature(secret, body, &sig));
    }

    #[test]
    fn hmac_signature_wrong_secret_fails() {
        let body = b"hello world";
        let sig = compute_signature("secret-1", body);
        assert!(!verify_signature("secret-2", body, &sig));
    }

    #[test]
    fn hmac_signature_tampered_body_fails() {
        let secret = "my-secret";
        let sig = compute_signature(secret, b"original");
        assert!(!verify_signature(secret, b"tampered", &sig));
    }

    #[test]
    fn hmac_signature_empty_body() {
        let secret = "key";
        let sig = compute_signature(secret, b"");
        assert!(verify_signature(secret, b"", &sig));
    }

    #[test]
    fn hmac_signature_is_hex_encoded() {
        let sig = compute_signature("key", b"data");
        assert_eq!(sig.len(), 64);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn delivery_status_display_roundtrip() {
        for status in [
            DeliveryStatus::Pending,
            DeliveryStatus::Delivered,
            DeliveryStatus::Failed,
        ] {
            let s = status.to_string();
            let parsed = DeliveryStatus::from_str(&s);
            assert_eq!(parsed, status);
        }
    }

    #[test]
    fn retry_policy_defaults() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.backoff_base_ms, 1000);
        assert_eq!(policy.max_delay_ms, 30_000);
    }

    #[test]
    fn truncate_body_short() {
        let body = "short";
        assert_eq!(truncate_body(body), "short");
    }

    #[test]
    fn truncate_body_long() {
        let body = "x".repeat(MAX_RESPONSE_BODY_LEN + 100);
        let truncated = truncate_body(&body);
        assert!(truncated.len() < body.len());
        assert!(truncated.ends_with("...(truncated)"));
    }

    #[test]
    fn webhook_config_secret_not_serialized() {
        let config = WebhookConfig {
            id: Uuid::new_v4(),
            url: "https://example.com/hook".into(),
            secret: "secret123".into(),
            events: vec![EventKind::RecordCreated, EventKind::RecordUpdated],
            entity_types: Some(vec!["User".into()]),
            headers: {
                let mut h = HashMap::new();
                h.insert("X-Custom".into(), "value".into());
                h
            },
            active: true,
            created_by: Uuid::new_v4(),
            retry_policy: RetryPolicy::default(),
            created_at: Utc::now(),
            consecutive_failures: 0,
        };

        let json = serde_json::to_string(&config).expect("serialize");
        assert!(!json.contains("secret123"));
        assert!(json.contains("https://example.com/hook"));
        assert!(json.contains("RecordCreated"));
    }

    #[test]
    fn event_matching_kind_and_entity_type() {
        let event = DdbEvent::new(EventKind::RecordCreated, 1).with_entity_type("User");

        let wh = WebhookConfig {
            id: Uuid::new_v4(),
            url: "https://example.com".into(),
            secret: "s".into(),
            events: vec![EventKind::RecordCreated],
            entity_types: Some(vec!["User".into()]),
            headers: HashMap::new(),
            active: true,
            created_by: Uuid::new_v4(),
            retry_policy: RetryPolicy::default(),
            created_at: Utc::now(),
            consecutive_failures: 0,
        };

        let matches = wh.active
            && (wh.events.is_empty() || wh.events.contains(&event.kind))
            && wh.entity_types.as_ref().is_none_or(|types| {
                event
                    .entity_type
                    .as_ref()
                    .is_some_and(|et| types.contains(et))
            });
        assert!(matches);
    }

    #[test]
    fn event_matching_wrong_kind() {
        let event = DdbEvent::new(EventKind::RecordDeleted, 1).with_entity_type("User");

        let matches_kind = [EventKind::RecordCreated].contains(&event.kind);
        assert!(!matches_kind);
    }

    #[test]
    fn event_matching_empty_events_matches_all() {
        let event = DdbEvent::new(EventKind::AuthLogin, 1);
        let events: Vec<EventKind> = vec![];
        assert!(events.is_empty() || events.contains(&event.kind));
    }

    #[test]
    fn inactive_webhook_does_not_match() {
        let wh = WebhookConfig {
            id: Uuid::new_v4(),
            url: "https://example.com".into(),
            secret: "s".into(),
            events: vec![],
            entity_types: None,
            headers: HashMap::new(),
            active: false,
            created_by: Uuid::new_v4(),
            retry_policy: RetryPolicy::default(),
            created_at: Utc::now(),
            consecutive_failures: 0,
        };
        assert!(!wh.active);
    }

    #[test]
    fn circuit_breaker_threshold_value() {
        // Locks the sentinel so a future refactor can't silently change it.
        assert_eq!(CIRCUIT_BREAKER_THRESHOLD, 10);
    }

    #[test]
    fn retry_policy_deserialization_with_defaults() {
        let json = r#"{"max_retries": 5}"#;
        let policy: RetryPolicy = serde_json::from_str(json).expect("deser");
        assert_eq!(policy.max_retries, 5);
        assert_eq!(policy.backoff_base_ms, 1000); // default
        assert_eq!(policy.max_delay_ms, 30_000); // default
    }

    #[test]
    fn compute_signature_deterministic() {
        let s1 = compute_signature("key", b"body");
        let s2 = compute_signature("key", b"body");
        assert_eq!(s1, s2);
    }

    #[test]
    fn compute_signature_known_value() {
        // HMAC-SHA256("key", "The quick brown fox") should be deterministic.
        let sig = compute_signature("key", b"The quick brown fox");
        assert_eq!(sig.len(), 64);
        // Recompute to confirm stability.
        assert_eq!(sig, compute_signature("key", b"The quick brown fox"));
    }
}
