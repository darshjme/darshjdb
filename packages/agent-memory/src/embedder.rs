// SPDX-License-Identifier: MIT
// Author: Darshankumar Joshi
// Part of DarshJDB — the native multi-model database.
//
// Pluggable embedding providers for the agent-memory layer.
//
// The Phase 2.5 design calls for four concrete providers:
//
//   * `OpenAIEmbeddingProvider`    — `text-embedding-3-small`, 1536 dims.
//   * `OllamaEmbeddingProvider`    — local `nomic-embed-text`.
//   * `AnthropicEmbeddingProvider` — OpenAI-compatible endpoint override.
//   * `NoneProvider`               — zero-vector sink for dev/no-key mode.
//
// All providers implement the [`EmbeddingProvider`] trait which takes an
// owned `Vec<String>` (batch) and returns a `Vec<Vec<f32>>`. The batch API
// matters: the background worker in [`crate::worker`] fetches up to 50
// pending rows per tick and expects a single round-trip to the provider.
//
// All providers are `Send + Sync + 'static` so they can live behind
// `Box<dyn EmbeddingProvider>` inside the worker's `Arc`.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use async_openai::{
    Client as OpenAIClient,
    config::OpenAIConfig,
    types::embeddings::{CreateEmbeddingRequestArgs, EmbeddingInput},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Pluggable embedding provider.
///
/// Implementations MUST be `Send + Sync` so they can be shared across the
/// background worker task and live behind `Box<dyn EmbeddingProvider>`.
///
/// The contract:
///
/// * `embed(texts)` returns one vector per input, in the same order. An empty
///   input slice MUST return an empty output slice without network I/O.
/// * `model()` returns a stable identifier for the underlying model. The
///   worker stores this on each row so downstream retrieval can detect
///   re-embedding churn.
/// * `dimensions()` returns the fixed vector width. This is used to validate
///   the `VECTOR(N)` column on `memory_entries` at startup.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a batch of texts. Must preserve input order.
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>>;

    /// Return the stable model identifier (e.g. `text-embedding-3-small`).
    fn model(&self) -> &str;

    /// Return the fixed vector width in elements.
    fn dimensions(&self) -> usize;
}

// ---------------------------------------------------------------------------
// OpenAI provider
// ---------------------------------------------------------------------------

/// OpenAI embeddings provider (`text-embedding-3-small`, 1536 dims).
///
/// Uses the `async-openai` SDK so we inherit proper request serialization,
/// retry semantics, and the streamed batch API. The API key is read from
/// `DARSH_EMBEDDING_API_KEY` at construction time.
pub struct OpenAIEmbeddingProvider {
    client: OpenAIClient<OpenAIConfig>,
    model: String,
    dimensions: usize,
}

impl OpenAIEmbeddingProvider {
    /// Construct the provider with the default `text-embedding-3-small` model.
    pub fn new(api_key: String) -> Self {
        Self::with_model(api_key, "text-embedding-3-small".to_string(), 1536)
    }

    /// Construct the provider with a custom model + dimensions.
    pub fn with_model(api_key: String, model: String, dimensions: usize) -> Self {
        let config = OpenAIConfig::new().with_api_key(api_key);
        Self {
            client: OpenAIClient::with_config(config),
            model,
            dimensions,
        }
    }

    /// Construct the provider targeting an OpenAI-compatible endpoint.
    ///
    /// Used by the Anthropic provider (and, transitively, any proxy such as
    /// LiteLLM that speaks the OpenAI embeddings schema).
    pub fn with_base_url(
        api_key: String,
        base_url: String,
        model: String,
        dimensions: usize,
    ) -> Self {
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(base_url);
        Self {
            client: OpenAIClient::with_config(config),
            model,
            dimensions,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIEmbeddingProvider {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let request = CreateEmbeddingRequestArgs::default()
            .model(&self.model)
            .input(EmbeddingInput::StringArray(texts.clone()))
            .build()
            .context("failed to build OpenAI embeddings request")?;

        let response = self
            .client
            .embeddings()
            .create(request)
            .await
            .context("OpenAI embeddings request failed")?;

        if response.data.len() != texts.len() {
            return Err(anyhow!(
                "OpenAI returned {} embeddings for {} inputs",
                response.data.len(),
                texts.len()
            ));
        }

        // `async-openai` does not guarantee that `data` is returned in the
        // same order as the input batch — it does in practice, but the spec
        // says to sort on `index`. We do it here to be safe.
        let mut sorted = response.data;
        sorted.sort_by_key(|e| e.index);

        Ok(sorted.into_iter().map(|e| e.embedding).collect())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

// ---------------------------------------------------------------------------
// Ollama provider
// ---------------------------------------------------------------------------

/// Local Ollama embeddings provider (`nomic-embed-text` by default).
///
/// Calls `POST {base}/api/embeddings` once per input because Ollama's
/// embeddings endpoint does not accept batches. The worker tolerates this —
/// batching is a performance optimization, not a correctness requirement.
pub struct OllamaEmbeddingProvider {
    base_url: String,
    model: String,
    dimensions: usize,
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

impl OllamaEmbeddingProvider {
    /// Construct the provider with the standard `nomic-embed-text` defaults.
    pub fn new(base_url: String) -> Self {
        Self::with_model(base_url, "nomic-embed-text".to_string(), 768)
    }

    /// Construct the provider with a custom model + vector width.
    pub fn with_model(base_url: String, model: String, dimensions: usize) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to build reqwest client for Ollama provider");
        Self {
            base_url,
            model,
            dimensions,
            client,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingProvider {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let endpoint = format!("{}/api/embeddings", self.base_url.trim_end_matches('/'));
        let mut out = Vec::with_capacity(texts.len());

        for text in texts {
            let req = OllamaRequest {
                model: &self.model,
                prompt: &text,
            };

            let response = self
                .client
                .post(&endpoint)
                .json(&req)
                .send()
                .await
                .context("Ollama embeddings request failed")?;

            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(anyhow!(
                    "Ollama embeddings error (status {status}): {body}"
                ));
            }

            let body: OllamaResponse = response
                .json()
                .await
                .context("failed to parse Ollama embeddings response")?;

            if body.embedding.is_empty() {
                return Err(anyhow!("Ollama returned empty embedding"));
            }
            if body.embedding.len() != self.dimensions {
                warn!(
                    expected = self.dimensions,
                    actual = body.embedding.len(),
                    "Ollama dimension mismatch, using provider-reported width"
                );
            }
            out.push(body.embedding);
        }

        debug!(count = out.len(), model = %self.model, "Ollama batch embedded");
        Ok(out)
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

// ---------------------------------------------------------------------------
// Anthropic provider
// ---------------------------------------------------------------------------

/// Anthropic-branded embeddings provider.
///
/// Anthropic does not (as of Phase 2.5) ship a first-party embeddings API,
/// so this provider is really an OpenAI-compatible client pointed at a
/// configurable base URL (LiteLLM, Voyage proxy, etc.). Keeping it as its
/// own type makes the factory + metrics labels clean.
pub struct AnthropicEmbeddingProvider {
    inner: OpenAIEmbeddingProvider,
}

impl AnthropicEmbeddingProvider {
    /// Construct the provider pointed at an OpenAI-compatible endpoint.
    pub fn new(api_key: String, base_url: String, model: String, dimensions: usize) -> Self {
        Self {
            inner: OpenAIEmbeddingProvider::with_base_url(api_key, base_url, model, dimensions),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for AnthropicEmbeddingProvider {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        self.inner.embed(texts).await
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn dimensions(&self) -> usize {
        self.inner.dimensions()
    }
}

// ---------------------------------------------------------------------------
// NoneProvider — dev / no-key fallback
// ---------------------------------------------------------------------------

/// No-op provider that returns fixed-size zero vectors.
///
/// Selected automatically when `DARSH_EMBEDDING_PROVIDER` is unset or set to
/// `none`. Keeps the worker loop well-typed during local development and
/// CI runs where no API key is available.
pub struct NoneProvider {
    dimensions: usize,
    model: String,
}

impl NoneProvider {
    /// Construct with the canonical 1536-wide zero vectors.
    pub fn new() -> Self {
        Self::with_dimensions(1536)
    }

    /// Construct with a custom width (used by tests).
    pub fn with_dimensions(dimensions: usize) -> Self {
        Self {
            dimensions,
            model: "none-zero".to_string(),
        }
    }
}

impl Default for NoneProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EmbeddingProvider for NoneProvider {
    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0_f32; self.dimensions]).collect())
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }
}

// ---------------------------------------------------------------------------
// Factory — from_env
// ---------------------------------------------------------------------------

/// Construct a provider from the `DARSH_EMBEDDING_*` environment variables.
///
/// Recognized values of `DARSH_EMBEDDING_PROVIDER`:
///
/// | value        | provider                        | required env                                               |
/// | ------------ | ------------------------------- | ---------------------------------------------------------- |
/// | `openai`     | [`OpenAIEmbeddingProvider`]     | `DARSH_EMBEDDING_API_KEY`                                  |
/// | `ollama`     | [`OllamaEmbeddingProvider`]     | optional `DARSH_EMBEDDING_ENDPOINT` (default `localhost`)  |
/// | `anthropic`  | [`AnthropicEmbeddingProvider`]  | `DARSH_EMBEDDING_API_KEY` + `DARSH_EMBEDDING_ENDPOINT`     |
/// | `none` / ""  | [`NoneProvider`]                | —                                                          |
///
/// Any unknown value falls back to [`NoneProvider`] with a warning so the
/// server never refuses to boot on a typo.
pub fn from_env() -> Box<dyn EmbeddingProvider> {
    let provider = std::env::var("DARSH_EMBEDDING_PROVIDER")
        .unwrap_or_else(|_| "none".to_string())
        .to_lowercase();

    match provider.as_str() {
        "openai" => {
            let key = std::env::var("DARSH_EMBEDDING_API_KEY").unwrap_or_default();
            if key.is_empty() {
                warn!(
                    "DARSH_EMBEDDING_PROVIDER=openai but DARSH_EMBEDDING_API_KEY is unset; \
                     falling back to NoneProvider"
                );
                return Box::new(NoneProvider::new());
            }
            Box::new(OpenAIEmbeddingProvider::new(key))
        }
        "ollama" => {
            let endpoint = std::env::var("DARSH_EMBEDDING_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            Box::new(OllamaEmbeddingProvider::new(endpoint))
        }
        "anthropic" => {
            let key = std::env::var("DARSH_EMBEDDING_API_KEY").unwrap_or_default();
            let endpoint = std::env::var("DARSH_EMBEDDING_ENDPOINT").unwrap_or_default();
            if key.is_empty() || endpoint.is_empty() {
                warn!(
                    "DARSH_EMBEDDING_PROVIDER=anthropic requires DARSH_EMBEDDING_API_KEY \
                     and DARSH_EMBEDDING_ENDPOINT; falling back to NoneProvider"
                );
                return Box::new(NoneProvider::new());
            }
            let model = std::env::var("DARSH_EMBEDDING_MODEL")
                .unwrap_or_else(|_| "voyage-3".to_string());
            let dims = std::env::var("DARSH_EMBEDDING_DIMENSIONS")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1024);
            Box::new(AnthropicEmbeddingProvider::new(key, endpoint, model, dims))
        }
        "none" | "" => Box::new(NoneProvider::new()),
        other => {
            warn!(
                provider = other,
                "unknown DARSH_EMBEDDING_PROVIDER; falling back to NoneProvider"
            );
            Box::new(NoneProvider::new())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn none_provider_returns_1536_zeros_per_input() {
        let provider = NoneProvider::new();
        assert_eq!(provider.dimensions(), 1536);
        assert_eq!(provider.model(), "none-zero");

        let out = provider
            .embed(vec!["one".to_string(), "two".to_string(), "three".to_string()])
            .await
            .expect("none provider never errors");

        assert_eq!(out.len(), 3);
        for vec in out {
            assert_eq!(vec.len(), 1536, "each output must be 1536-wide");
            assert!(vec.iter().all(|x| *x == 0.0), "all zeros");
        }
    }

    #[tokio::test]
    async fn none_provider_empty_input_empty_output() {
        let provider = NoneProvider::new();
        let out = provider.embed(Vec::new()).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn none_provider_with_custom_dimensions() {
        let provider = NoneProvider::with_dimensions(768);
        let out = provider.embed(vec!["x".to_string()]).await.unwrap();
        assert_eq!(out[0].len(), 768);
    }

    // These tests mutate process env. We guard them with a single mutex so
    // parallel cargo test runs do not race on DARSH_EMBEDDING_PROVIDER.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    fn clear_env() {
        // SAFETY: tests take env_lock() before calling, serializing access.
        unsafe {
            std::env::remove_var("DARSH_EMBEDDING_PROVIDER");
            std::env::remove_var("DARSH_EMBEDDING_API_KEY");
            std::env::remove_var("DARSH_EMBEDDING_ENDPOINT");
            std::env::remove_var("DARSH_EMBEDDING_MODEL");
            std::env::remove_var("DARSH_EMBEDDING_DIMENSIONS");
        }
    }

    #[tokio::test]
    async fn factory_defaults_to_none_when_env_unset() {
        let _g = env_lock().lock().unwrap();
        clear_env();
        let provider = from_env();
        assert_eq!(provider.model(), "none-zero");
        assert_eq!(provider.dimensions(), 1536);
    }

    #[tokio::test]
    async fn factory_picks_none_for_explicit_none() {
        let _g = env_lock().lock().unwrap();
        clear_env();
        // SAFETY: serialized by env_lock.
        unsafe { std::env::set_var("DARSH_EMBEDDING_PROVIDER", "none") };
        let provider = from_env();
        assert_eq!(provider.model(), "none-zero");
    }

    #[tokio::test]
    async fn factory_picks_openai_when_key_present() {
        let _g = env_lock().lock().unwrap();
        clear_env();
        // SAFETY: serialized by env_lock.
        unsafe {
            std::env::set_var("DARSH_EMBEDDING_PROVIDER", "openai");
            std::env::set_var("DARSH_EMBEDDING_API_KEY", "sk-test-ignored");
        }
        let provider = from_env();
        assert_eq!(provider.model(), "text-embedding-3-small");
        assert_eq!(provider.dimensions(), 1536);
        clear_env();
    }

    #[tokio::test]
    async fn factory_falls_back_to_none_for_openai_without_key() {
        let _g = env_lock().lock().unwrap();
        clear_env();
        // SAFETY: serialized by env_lock.
        unsafe { std::env::set_var("DARSH_EMBEDDING_PROVIDER", "openai") };
        let provider = from_env();
        assert_eq!(provider.model(), "none-zero");
        clear_env();
    }

    #[tokio::test]
    async fn factory_picks_ollama_with_default_endpoint() {
        let _g = env_lock().lock().unwrap();
        clear_env();
        // SAFETY: serialized by env_lock.
        unsafe { std::env::set_var("DARSH_EMBEDDING_PROVIDER", "ollama") };
        let provider = from_env();
        assert_eq!(provider.model(), "nomic-embed-text");
        clear_env();
    }

    #[tokio::test]
    async fn factory_unknown_falls_back_to_none() {
        let _g = env_lock().lock().unwrap();
        clear_env();
        // SAFETY: serialized by env_lock.
        unsafe { std::env::set_var("DARSH_EMBEDDING_PROVIDER", "bogus") };
        let provider = from_env();
        assert_eq!(provider.model(), "none-zero");
        clear_env();
    }
}
