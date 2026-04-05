//! Auto-embedding pipeline for DarshanDB.
//!
//! Generates vector embeddings when text triples are written, enabling
//! semantic search out of the box via pgvector. The pipeline listens on
//! the existing [`ChangeEvent`] broadcast channel and spawns background
//! tasks to generate and store embeddings without blocking mutations.
//!
//! # Providers
//!
//! - **OpenAI** — `text-embedding-ada-002` or any `/v1/embeddings` model.
//! - **Ollama** — Local model via `POST /api/embeddings`.
//! - **None** — Disabled (manual embedding via REST API only).
//!
//! # Configuration (environment variables)
//!
//! | Variable                           | Default                    | Description                                        |
//! |------------------------------------|----------------------------|----------------------------------------------------|
//! | `DARSHAN_EMBEDDING_PROVIDER`       | `none`                     | `openai`, `ollama`, or `none`                      |
//! | `DARSHAN_OPENAI_API_KEY`           | —                          | Required when provider is `openai`                 |
//! | `DARSHAN_EMBEDDING_MODEL`          | `text-embedding-ada-002`   | Model name for the chosen provider                 |
//! | `DARSHAN_OLLAMA_URL`               | `http://localhost:11434`   | Ollama server URL                                  |
//! | `DARSHAN_AUTO_EMBED_ATTRIBUTES`    | —                          | Comma-separated `type/attr` patterns to auto-embed |
//! | `DARSHAN_EMBEDDING_DIMENSIONS`     | `1536`                     | Vector dimensions (must match model output)        |

pub mod provider;

use std::sync::Arc;

use sqlx::PgPool;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::error::{DarshanError, Result};
use crate::sync::ChangeEvent;
use crate::triple_store::{PgTripleStore, TripleStore};

pub use provider::{EmbeddingProvider, OllamaProvider, OpenAIProvider};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the auto-embedding pipeline.
#[derive(Debug, Clone)]
pub struct EmbeddingConfig {
    /// Which entity attributes to auto-embed (e.g., `"articles/content"`, `"todos/title"`).
    ///
    /// Format: `"entity_type/attribute_name"`. When a triple is written whose
    /// `entity_type` + `attribute` matches an entry here, an embedding is
    /// generated and stored automatically.
    pub auto_embed_attributes: Vec<String>,

    /// Embedding vector dimensions. Must match the output of the configured model.
    pub dimensions: usize,

    /// The embedding provider to use.
    pub provider: ProviderConfig,
}

/// Provider-specific configuration.
#[derive(Debug, Clone)]
pub enum ProviderConfig {
    /// OpenAI embeddings API (`/v1/embeddings`).
    OpenAI { api_key: String, model: String },
    /// Local Ollama instance.
    Ollama { url: String, model: String },
    /// No auto-embedding. Embeddings can still be stored via REST API.
    None,
}

impl EmbeddingConfig {
    /// Build configuration from environment variables.
    ///
    /// Returns `None` if the provider is set to `none` (or unset) *and*
    /// no auto-embed attributes are configured — i.e., embedding is fully
    /// disabled.
    pub fn from_env() -> Option<Self> {
        let provider_str = std::env::var("DARSHAN_EMBEDDING_PROVIDER")
            .unwrap_or_else(|_| "none".to_string())
            .to_lowercase();

        let provider = match provider_str.as_str() {
            "openai" => {
                let api_key = match std::env::var("DARSHAN_OPENAI_API_KEY") {
                    Ok(key) if !key.is_empty() => key,
                    _ => {
                        warn!(
                            "DARSHAN_EMBEDDING_PROVIDER=openai but DARSHAN_OPENAI_API_KEY is not set"
                        );
                        return None;
                    }
                };
                let model = std::env::var("DARSHAN_EMBEDDING_MODEL")
                    .unwrap_or_else(|_| "text-embedding-ada-002".to_string());
                ProviderConfig::OpenAI { api_key, model }
            }
            "ollama" => {
                let url = std::env::var("DARSHAN_OLLAMA_URL")
                    .unwrap_or_else(|_| "http://localhost:11434".to_string());
                let model = std::env::var("DARSHAN_EMBEDDING_MODEL")
                    .unwrap_or_else(|_| "nomic-embed-text".to_string());
                ProviderConfig::Ollama { url, model }
            }
            _ => ProviderConfig::None,
        };

        if matches!(provider, ProviderConfig::None) {
            return None;
        }

        let auto_embed_attributes: Vec<String> = std::env::var("DARSHAN_AUTO_EMBED_ATTRIBUTES")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let dimensions: usize = std::env::var("DARSHAN_EMBEDDING_DIMENSIONS")
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(1536);

        Some(Self {
            auto_embed_attributes,
            dimensions,
            provider,
        })
    }

    /// Check whether a given `entity_type/attribute` pair should be auto-embedded.
    pub fn should_embed(&self, entity_type: &str, attribute: &str) -> bool {
        if self.auto_embed_attributes.is_empty() {
            // No filter means embed everything (when provider is active).
            return true;
        }
        let key = format!("{entity_type}/{attribute}");
        self.auto_embed_attributes.iter().any(|pattern| {
            pattern == &key
                || pattern == "*"
                || pattern == &format!("{entity_type}/*")
                || pattern == &format!("*/{attribute}")
        })
    }
}

// ---------------------------------------------------------------------------
// EmbeddingService
// ---------------------------------------------------------------------------

/// Core embedding service: generates vectors and persists them via pgvector.
pub struct EmbeddingService {
    config: EmbeddingConfig,
    pool: PgPool,
    provider: Box<dyn EmbeddingProvider>,
    triple_store: Arc<PgTripleStore>,
}

impl EmbeddingService {
    /// Create a new embedding service.
    pub fn new(config: EmbeddingConfig, pool: PgPool, triple_store: Arc<PgTripleStore>) -> Self {
        let provider: Box<dyn EmbeddingProvider> = match &config.provider {
            ProviderConfig::OpenAI { api_key, model } => {
                Box::new(OpenAIProvider::new(api_key.clone(), model.clone()))
            }
            ProviderConfig::Ollama { url, model } => {
                Box::new(OllamaProvider::new(url.clone(), model.clone()))
            }
            ProviderConfig::None => {
                // Should not reach here if config is built correctly,
                // but provide a no-op fallback.
                Box::new(provider::NoopProvider)
            }
        };

        Self {
            config,
            pool,
            provider,
            triple_store,
        }
    }

    /// Generate an embedding vector for the given text.
    pub async fn embed_text(&self, text: &str) -> Result<Vec<f32>> {
        self.provider
            .embed(text.to_string())
            .await
            .map_err(|e| DarshanError::Internal(format!("embedding generation failed: {e}")))
    }

    /// Store an embedding for a specific entity + attribute pair.
    ///
    /// Uses pgvector's `vector` type. The `entity_embeddings` table is
    /// created during schema migration.
    pub async fn store_embedding(
        &self,
        entity_id: Uuid,
        attribute: &str,
        embedding: &[f32],
    ) -> Result<()> {
        // Convert Vec<f32> to a pgvector-compatible string: '[0.1,0.2,...]'
        let vec_str = format!(
            "[{}]",
            embedding
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );

        sqlx::query(
            r#"
            INSERT INTO entity_embeddings (entity_id, attribute, embedding, dimensions, updated_at)
            VALUES ($1, $2, $3::vector, $4, NOW())
            ON CONFLICT (entity_id, attribute)
            DO UPDATE SET embedding = $3::vector, dimensions = $4, updated_at = NOW()
            "#,
        )
        .bind(entity_id)
        .bind(attribute)
        .bind(&vec_str)
        .bind(self.config.dimensions as i32)
        .execute(&self.pool)
        .await
        .map_err(|e| DarshanError::Database(e))?;

        Ok(())
    }

    /// Called when a triple is written. Checks if the attribute matches the
    /// auto-embed filter and, if so, generates + stores the embedding.
    pub async fn on_triple_written(
        &self,
        entity_id: Uuid,
        entity_type: &str,
        attribute: &str,
        value: &str,
    ) -> Result<()> {
        if !self.config.should_embed(entity_type, attribute) {
            return Ok(());
        }

        if value.trim().is_empty() {
            debug!(
                entity_id = %entity_id,
                attribute = %attribute,
                "skipping embedding for empty value"
            );
            return Ok(());
        }

        let embedding = self.embed_text(value).await?;

        if embedding.len() != self.config.dimensions {
            return Err(DarshanError::Internal(format!(
                "embedding dimension mismatch: expected {}, got {}",
                self.config.dimensions,
                embedding.len()
            )));
        }

        self.store_embedding(entity_id, attribute, &embedding)
            .await?;

        debug!(
            entity_id = %entity_id,
            attribute = %attribute,
            dimensions = self.config.dimensions,
            "auto-embedded attribute"
        );

        Ok(())
    }

    /// Ensure the `entity_embeddings` table and pgvector extension exist.
    pub async fn ensure_schema(&self) -> Result<()> {
        // Enable pgvector extension (idempotent).
        sqlx::query("CREATE EXTENSION IF NOT EXISTS vector")
            .execute(&self.pool)
            .await
            .map_err(|e| {
                DarshanError::Internal(format!(
                    "failed to enable pgvector extension: {e}. \
                     Install pgvector: https://github.com/pgvector/pgvector"
                ))
            })?;

        // Create the embeddings table.
        sqlx::query(&format!(
            r#"
            CREATE TABLE IF NOT EXISTS entity_embeddings (
                entity_id   UUID        NOT NULL,
                attribute   TEXT        NOT NULL,
                embedding   vector({dim}) NOT NULL,
                dimensions  INT         NOT NULL DEFAULT {dim},
                updated_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (entity_id, attribute)
            )
            "#,
            dim = self.config.dimensions,
        ))
        .execute(&self.pool)
        .await
        .map_err(DarshanError::Database)?;

        // Create an IVFFlat index for approximate nearest-neighbor search.
        // Uses cosine distance by default (most common for text embeddings).
        sqlx::query(
            r#"
            CREATE INDEX IF NOT EXISTS idx_entity_embeddings_cosine
            ON entity_embeddings
            USING ivfflat (embedding vector_cosine_ops)
            WITH (lists = 100)
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(DarshanError::Database)?;

        info!(
            dimensions = self.config.dimensions,
            "entity_embeddings schema ensured (pgvector)"
        );

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// EmbeddingManager — bridges ChangeEvent broadcast to EmbeddingService
// ---------------------------------------------------------------------------

/// Manages the embedding pipeline lifecycle: listens on the change broadcast
/// channel and spawns background embedding tasks.
pub struct EmbeddingManager {
    service: Arc<EmbeddingService>,
}

impl EmbeddingManager {
    /// Create a new manager wrapping the given service.
    pub fn new(service: EmbeddingService) -> Self {
        Self {
            service: Arc::new(service),
        }
    }

    /// Run the embedding listener loop.
    ///
    /// Subscribes to [`ChangeEvent`]s and spawns a background task for each
    /// event that might require embedding generation. This method runs
    /// forever and should be spawned with `tokio::spawn`.
    pub async fn run(self: Arc<Self>, mut change_rx: broadcast::Receiver<ChangeEvent>) {
        info!("embedding manager started, listening for change events");

        loop {
            let event = match change_rx.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(
                        skipped = n,
                        "embedding manager lagged behind; some embeddings may be stale"
                    );
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("change broadcast channel closed, embedding manager shutting down");
                    return;
                }
            };

            // Spawn a background task so we don't block the broadcast loop.
            let service = self.service.clone();
            tokio::spawn(async move {
                if let Err(e) = Self::handle_change_event(&service, event).await {
                    error!(error = %e, "embedding generation failed");
                }
            });
        }
    }

    /// Process a single change event: for each affected entity + attribute,
    /// check if auto-embedding applies and generate the embedding.
    async fn handle_change_event(service: &EmbeddingService, event: ChangeEvent) -> Result<()> {
        let entity_type = event.entity_type.clone().unwrap_or_default();

        for entity_id_str in &event.entity_ids {
            let entity_id = match Uuid::parse_str(entity_id_str) {
                Ok(id) => id,
                Err(_) => continue,
            };

            // Fetch current triples for this entity to get the text values.
            let triples = match service.triple_store.get_entity(entity_id).await {
                Ok(t) => t,
                Err(e) => {
                    error!(
                        entity_id = %entity_id,
                        error = %e,
                        "failed to fetch entity for embedding"
                    );
                    continue;
                }
            };

            // Only process attributes that were actually changed.
            for attr_name in &event.attributes {
                if !service.config.should_embed(&entity_type, attr_name) {
                    continue;
                }

                // Find the current value for this attribute.
                let value = triples
                    .iter()
                    .find(|t| t.attribute == *attr_name && !t.retracted)
                    .and_then(|t| t.value.as_str().map(|s| s.to_string()));

                if let Some(text) = value {
                    if let Err(e) = service
                        .on_triple_written(entity_id, &entity_type, attr_name, &text)
                        .await
                    {
                        error!(
                            entity_id = %entity_id,
                            attribute = %attr_name,
                            error = %e,
                            "auto-embedding failed for attribute"
                        );
                    }
                }
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_embed_exact_match() {
        let config = EmbeddingConfig {
            auto_embed_attributes: vec!["articles/content".to_string()],
            dimensions: 1536,
            provider: ProviderConfig::None,
        };
        assert!(config.should_embed("articles", "content"));
        assert!(!config.should_embed("articles", "title"));
        assert!(!config.should_embed("users", "content"));
    }

    #[test]
    fn test_should_embed_wildcard_type() {
        let config = EmbeddingConfig {
            auto_embed_attributes: vec!["articles/*".to_string()],
            dimensions: 1536,
            provider: ProviderConfig::None,
        };
        assert!(config.should_embed("articles", "content"));
        assert!(config.should_embed("articles", "title"));
        assert!(!config.should_embed("users", "content"));
    }

    #[test]
    fn test_should_embed_wildcard_attribute() {
        let config = EmbeddingConfig {
            auto_embed_attributes: vec!["*/content".to_string()],
            dimensions: 1536,
            provider: ProviderConfig::None,
        };
        assert!(config.should_embed("articles", "content"));
        assert!(config.should_embed("posts", "content"));
        assert!(!config.should_embed("articles", "title"));
    }

    #[test]
    fn test_should_embed_global_wildcard() {
        let config = EmbeddingConfig {
            auto_embed_attributes: vec!["*".to_string()],
            dimensions: 1536,
            provider: ProviderConfig::None,
        };
        assert!(config.should_embed("anything", "everything"));
    }

    #[test]
    fn test_should_embed_empty_attributes_means_all() {
        let config = EmbeddingConfig {
            auto_embed_attributes: vec![],
            dimensions: 1536,
            provider: ProviderConfig::None,
        };
        assert!(config.should_embed("anything", "everything"));
    }
}
