//! Embedding provider implementations.
//!
//! Each provider takes text input and returns a dense vector of `f32` values.
//! Providers handle their own HTTP transport, serialization, and retry logic.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

/// Trait for embedding providers.
///
/// Object-safe (`Send + Sync`) for use behind `Box<dyn EmbeddingProvider>`.
/// Takes owned `String` to avoid lifetime issues with async trait methods.
pub trait EmbeddingProvider: Send + Sync {
    /// Generate an embedding vector for the given text.
    fn embed(
        &self,
        text: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<Vec<f32>, String>> + Send + '_>,
    >;
}

// ---------------------------------------------------------------------------
// OpenAI provider
// ---------------------------------------------------------------------------

/// OpenAI `/v1/embeddings` API provider.
///
/// Supports `text-embedding-ada-002`, `text-embedding-3-small`,
/// `text-embedding-3-large`, and any model served on a compatible API.
pub struct OpenAIProvider {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct OpenAIRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    data: Vec<OpenAIEmbedding>,
}

#[derive(Deserialize)]
struct OpenAIEmbedding {
    embedding: Vec<f32>,
}

impl OpenAIProvider {
    /// Create a new OpenAI provider.
    pub fn new(api_key: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");

        Self {
            api_key,
            model,
            client,
        }
    }

    /// Make the API call with retry on rate-limit (429) responses.
    async fn call_with_retry(&self, text: &str) -> std::result::Result<Vec<f32>, String> {
        let max_retries = 3;
        let mut backoff = Duration::from_millis(500);

        for attempt in 0..=max_retries {
            let request = OpenAIRequest {
                model: &self.model,
                input: text,
            };

            let response = self
                .client
                .post("https://api.openai.com/v1/embeddings")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await
                .map_err(|e| format!("OpenAI request failed: {e}"))?;

            let status = response.status();

            if status.is_success() {
                let body: OpenAIResponse = response
                    .json()
                    .await
                    .map_err(|e| format!("failed to parse OpenAI response: {e}"))?;

                return body
                    .data
                    .into_iter()
                    .next()
                    .map(|e| e.embedding)
                    .ok_or_else(|| "OpenAI returned empty embedding data".to_string());
            }

            if status.as_u16() == 429 && attempt < max_retries {
                // Rate limited -- extract Retry-After if present.
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(Duration::from_secs)
                    .unwrap_or(backoff);

                warn!(
                    attempt = attempt + 1,
                    retry_after_ms = retry_after.as_millis() as u64,
                    "OpenAI rate limited, retrying"
                );

                tokio::time::sleep(retry_after).await;
                backoff *= 2; // Exponential backoff for subsequent retries.
                continue;
            }

            let error_body = response.text().await.unwrap_or_default();
            return Err(format!("OpenAI API error (status {status}): {error_body}"));
        }

        Err("OpenAI API: max retries exceeded".to_string())
    }
}

impl EmbeddingProvider for OpenAIProvider {
    fn embed(
        &self,
        text: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<Vec<f32>, String>> + Send + '_>,
    > {
        Box::pin(async move { self.call_with_retry(&text).await })
    }
}

// ---------------------------------------------------------------------------
// Ollama provider
// ---------------------------------------------------------------------------

/// Local Ollama embedding provider.
///
/// Calls `POST /api/embeddings` on a local (or remote) Ollama instance.
/// Popular models: `nomic-embed-text`, `mxbai-embed-large`, `all-minilm`.
pub struct OllamaProvider {
    url: String,
    model: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct OllamaResponse {
    embedding: Vec<f32>,
}

impl OllamaProvider {
    /// Create a new Ollama provider.
    pub fn new(url: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60)) // Local models can be slower.
            .build()
            .expect("failed to build reqwest client");

        Self { url, model, client }
    }
}

impl EmbeddingProvider for OllamaProvider {
    fn embed(
        &self,
        text: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<Vec<f32>, String>> + Send + '_>,
    > {
        Box::pin(async move {
            let endpoint = format!("{}/api/embeddings", self.url.trim_end_matches('/'));

            let request = OllamaRequest {
                model: &self.model,
                prompt: &text,
            };

            let response = self
                .client
                .post(&endpoint)
                .json(&request)
                .send()
                .await
                .map_err(|e| format!("Ollama request failed: {e}"))?;

            let status = response.status();
            if !status.is_success() {
                let error_body = response.text().await.unwrap_or_default();
                return Err(format!("Ollama API error (status {status}): {error_body}"));
            }

            let body: OllamaResponse = response
                .json()
                .await
                .map_err(|e| format!("failed to parse Ollama response: {e}"))?;

            if body.embedding.is_empty() {
                return Err("Ollama returned empty embedding".to_string());
            }

            debug!(
                model = %self.model,
                dimensions = body.embedding.len(),
                "Ollama embedding generated"
            );

            Ok(body.embedding)
        })
    }
}

// ---------------------------------------------------------------------------
// Noop provider (fallback when provider = None)
// ---------------------------------------------------------------------------

/// No-op provider that always returns an error.
///
/// Used as a fallback when `ProviderConfig::None` is selected but the
/// service is somehow instantiated anyway.
pub struct NoopProvider;

impl EmbeddingProvider for NoopProvider {
    fn embed(
        &self,
        _text: String,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<Vec<f32>, String>> + Send + '_>,
    > {
        Box::pin(async {
            Err("embedding provider is disabled (set DDB_EMBEDDING_PROVIDER)".to_string())
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_noop_provider_returns_error() {
        let provider = NoopProvider;
        let result = provider.embed("hello".to_string()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("disabled"));
    }
}
