//! Webhook connector — POSTs entity change payloads to a configured URL.
//!
//! Configure via the `DARSHAN_WEBHOOK_URL` environment variable.
//! Retries up to 3 times with exponential backoff (1s, 2s, 4s) on
//! transient failures (network errors, 5xx responses).

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::Serialize;
use tracing::{info, warn};
use uuid::Uuid;

use super::{Connector, EntityChangeEvent};
use crate::error::{DarshanError, Result};

/// Maximum number of delivery attempts (initial + retries).
const MAX_ATTEMPTS: u32 = 3;

/// Base delay for exponential backoff.
const BASE_BACKOFF: Duration = Duration::from_secs(1);

/// Payload sent to the webhook endpoint on entity create / update.
#[derive(Debug, Serialize)]
struct WebhookPayload<'a> {
    event: &'a str,
    entity_id: Uuid,
    entity_type: &'a str,
    tx_id: i64,
    changed_attributes: &'a [String],
    entity: &'a std::collections::HashMap<String, serde_json::Value>,
}

/// Payload sent to the webhook endpoint on entity delete.
#[derive(Debug, Serialize)]
struct WebhookDeletePayload<'a> {
    event: &'a str,
    entity_id: Uuid,
    entity_type: &'a str,
}

/// A connector that delivers entity changes to an HTTP webhook.
pub struct WebhookConnector {
    url: String,
    client: reqwest::Client,
}

impl WebhookConnector {
    /// Create a new webhook connector targeting `url`.
    pub fn new(url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");

        Self { url, client }
    }

    /// Try to create a connector from the `DARSHAN_WEBHOOK_URL` env var.
    /// Returns `None` if the variable is not set.
    pub fn from_env() -> Option<Self> {
        std::env::var("DARSHAN_WEBHOOK_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .map(Self::new)
    }

    /// POST `body` to the webhook URL with retries.
    async fn post_with_retry(&self, body: &[u8]) -> Result<()> {
        let mut last_error: Option<String> = None;

        for attempt in 0..MAX_ATTEMPTS {
            if attempt > 0 {
                let delay = BASE_BACKOFF * 2u32.pow(attempt - 1);
                tokio::time::sleep(delay).await;
            }

            match self
                .client
                .post(&self.url)
                .header("Content-Type", "application/json")
                .header("User-Agent", "DarshanDB-Webhook/1.0")
                .body(body.to_vec())
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                Ok(resp) if resp.status().is_server_error() => {
                    let status = resp.status();
                    let msg = format!("webhook returned {status}");
                    warn!(
                        attempt = attempt + 1,
                        max = MAX_ATTEMPTS,
                        status = %status,
                        url = %self.url,
                        "webhook delivery failed (retryable)"
                    );
                    last_error = Some(msg);
                }
                Ok(resp) => {
                    // 4xx — not retryable.
                    let status = resp.status();
                    return Err(DarshanError::Internal(format!(
                        "webhook returned non-retryable status {status}"
                    )));
                }
                Err(e) => {
                    warn!(
                        attempt = attempt + 1,
                        max = MAX_ATTEMPTS,
                        error = %e,
                        url = %self.url,
                        "webhook delivery failed (network error, retrying)"
                    );
                    last_error = Some(e.to_string());
                }
            }
        }

        Err(DarshanError::Internal(format!(
            "webhook delivery failed after {MAX_ATTEMPTS} attempts: {}",
            last_error.unwrap_or_default()
        )))
    }
}

impl Connector for WebhookConnector {
    fn name(&self) -> &str {
        "webhook"
    }

    fn on_entity_changed(
        &self,
        event: EntityChangeEvent,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async move {
            let payload = WebhookPayload {
                event: "entity.changed",
                entity_id: event.entity_id,
                entity_type: &event.entity_type,
                tx_id: event.tx_id,
                changed_attributes: &event.changed_attributes,
                entity: &event.attributes,
            };

            let body = serde_json::to_vec(&payload).map_err(|e| {
                DarshanError::Internal(format!("failed to serialize webhook payload: {e}"))
            })?;

            self.post_with_retry(&body).await?;

            info!(
                entity_id = %event.entity_id,
                entity_type = %event.entity_type,
                url = %self.url,
                "webhook delivered entity.changed"
            );

            Ok(())
        })
    }

    fn on_entity_deleted(
        &self,
        entity_id: Uuid,
        entity_type: &str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let entity_type = entity_type.to_owned();
        Box::pin(async move {
            let payload = WebhookDeletePayload {
                event: "entity.deleted",
                entity_id,
                entity_type: &entity_type,
            };

            let body = serde_json::to_vec(&payload).map_err(|e| {
                DarshanError::Internal(format!("failed to serialize webhook payload: {e}"))
            })?;

            self.post_with_retry(&body).await?;

            info!(
                entity_id = %entity_id,
                entity_type = %entity_type,
                url = %self.url,
                "webhook delivered entity.deleted"
            );

            Ok(())
        })
    }

    fn initialize(&self) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async {
            info!(url = %self.url, "webhook connector initialized");
            Ok(())
        })
    }
}
