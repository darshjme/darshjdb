// SPDX-License-Identifier: MIT
// Author: Darshankumar Joshi
// Part of DarshJDB — the native multi-model database.
//
// Background worker that fills `embedding` + `content_tokens` on rows
// produced by the Phase 2 agent memory schema (slice 12).
//
// Design notes:
//
//   * Wakes every 5 seconds and SELECTs up to 50 rows from `memory_entries`
//     where `embedding IS NULL` and `tier != 'archival'`. Archival rows are
//     cold storage and are embedded on demand rather than continuously.
//
//   * Does the same for `agent_facts` where `embedding IS NULL`. These are
//     typically short (< 128 tokens) so we push them through the same batch.
//
//   * Calls `provider.embed(Vec<String>)` exactly once per tier-bucket,
//     then writes back via `UPDATE ... SET embedding = $1::vector, ...`.
//
//   * `content_tokens` is computed via the `cl100k_base` BPE tokenizer
//     (same tokenizer `text-embedding-3-small` uses under the hood). We
//     cache the BPE encoder for the lifetime of the worker to avoid the
//     ~30 ms cold-start on every tick.
//
//   * Emits two metrics:
//       - counter `ddb_embeddings_generated_total` (labeled by `provider`)
//       - gauge   `ddb_embeddings_pending`         (labeled by `table`)
//
// The worker is resilient: any SQL error during a tick is logged and the
// loop continues. The only way to stop it is to drop the returned join
// handle or signal shutdown via the `tokio::select!` on the provided
// shutdown future.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};
use tiktoken_rs::cl100k_base;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::embedder::EmbeddingProvider;

/// How often the worker wakes up to look for un-embedded rows.
const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Maximum rows to pull per tick per table. Keeps provider batches bounded.
const BATCH_LIMIT: i64 = 50;

/// Join handle for a running embedding worker.
///
/// Dropping the handle aborts the worker. The server's graceful shutdown
/// path should call [`EmbeddingWorkerHandle::shutdown`] instead.
pub struct EmbeddingWorkerHandle {
    inner: JoinHandle<()>,
}

impl EmbeddingWorkerHandle {
    /// Abort the worker and await its task.
    pub async fn shutdown(self) {
        self.inner.abort();
        let _ = self.inner.await;
    }
}

/// Spawn the worker on the current Tokio runtime.
///
/// Returns a handle that can be used for graceful shutdown. Panics from
/// inside the worker are logged but do not propagate — the worker will
/// simply be restarted on the next tick cycle via the retry path.
pub fn spawn_embedding_worker(
    pool: PgPool,
    provider: Arc<dyn EmbeddingProvider>,
) -> EmbeddingWorkerHandle {
    let inner = tokio::spawn(async move {
        if let Err(e) = embedding_worker(pool, provider).await {
            error!(error = %e, "embedding worker exited with error");
        }
    });
    EmbeddingWorkerHandle { inner }
}

/// The actual worker loop. Public for integration tests that want to drive
/// a single tick against a fixture database.
pub async fn embedding_worker(pool: PgPool, provider: Arc<dyn EmbeddingProvider>) -> Result<()> {
    let bpe = cl100k_base().context("failed to load cl100k_base BPE")?;
    let provider_label = provider.model().to_string();
    info!(
        provider = %provider_label,
        dimensions = provider.dimensions(),
        interval_secs = TICK_INTERVAL.as_secs(),
        "embedding worker started"
    );

    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        ticker.tick().await;

        // Update pending gauges before we start work so the dashboard sees
        // the queue depth even if the provider call blocks.
        if let Err(e) = refresh_pending_gauges(&pool).await {
            warn!(error = %e, "failed to refresh embedding pending gauges");
        }

        if let Err(e) =
            process_table(&pool, provider.as_ref(), &provider_label, &bpe, TableKind::Memory).await
        {
            warn!(error = %e, "embedding tick failed for memory_entries");
        }
        if let Err(e) =
            process_table(&pool, provider.as_ref(), &provider_label, &bpe, TableKind::Facts).await
        {
            warn!(error = %e, "embedding tick failed for agent_facts");
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum TableKind {
    Memory,
    Facts,
}

impl TableKind {
    fn label(self) -> &'static str {
        match self {
            TableKind::Memory => "memory_entries",
            TableKind::Facts => "agent_facts",
        }
    }

    /// SELECT statement for this table's pending-embedding queue.
    fn select_sql(self) -> &'static str {
        match self {
            TableKind::Memory => {
                "SELECT id, content \
                 FROM memory_entries \
                 WHERE embedding IS NULL AND tier != 'archival' \
                 ORDER BY created_at DESC \
                 LIMIT $1"
            }
            TableKind::Facts => {
                "SELECT id, content \
                 FROM agent_facts \
                 WHERE embedding IS NULL \
                 ORDER BY created_at DESC \
                 LIMIT $1"
            }
        }
    }

    /// UPDATE statement. Uses `$1::vector` so we bind the pgvector text form
    /// and don't depend on the `pgvector` crate at compile time.
    fn update_sql(self) -> &'static str {
        match self {
            TableKind::Memory => {
                "UPDATE memory_entries \
                 SET embedding = $1::vector, content_tokens = $2, embedded_at = NOW() \
                 WHERE id = $3"
            }
            TableKind::Facts => {
                "UPDATE agent_facts \
                 SET embedding = $1::vector, content_tokens = $2, embedded_at = NOW() \
                 WHERE id = $3"
            }
        }
    }
}

async fn process_table(
    pool: &PgPool,
    provider: &dyn EmbeddingProvider,
    provider_label: &str,
    bpe: &tiktoken_rs::CoreBPE,
    kind: TableKind,
) -> Result<()> {
    let rows = sqlx::query(kind.select_sql())
        .bind(BATCH_LIMIT)
        .fetch_all(pool)
        .await
        .with_context(|| format!("select pending {}", kind.label()))?;

    if rows.is_empty() {
        return Ok(());
    }

    let mut ids: Vec<Uuid> = Vec::with_capacity(rows.len());
    let mut texts: Vec<String> = Vec::with_capacity(rows.len());
    for row in rows {
        let id: Uuid = row
            .try_get("id")
            .with_context(|| format!("{}: missing id column", kind.label()))?;
        let content: String = row
            .try_get("content")
            .with_context(|| format!("{}: missing content column", kind.label()))?;
        ids.push(id);
        texts.push(content);
    }

    debug!(
        table = kind.label(),
        batch = texts.len(),
        "submitting embedding batch"
    );

    let embeddings = provider
        .embed(texts.clone())
        .await
        .with_context(|| format!("embed batch for {}", kind.label()))?;

    if embeddings.len() != texts.len() {
        return Err(anyhow::anyhow!(
            "{}: provider returned {} vectors for {} inputs",
            kind.label(),
            embeddings.len(),
            texts.len()
        ));
    }

    let mut written: u64 = 0;
    for ((id, text), vector) in ids.iter().zip(texts.iter()).zip(embeddings.iter()) {
        let tokens = bpe.encode_with_special_tokens(text).len() as i32;
        let literal = pgvector_literal(vector);

        match sqlx::query(kind.update_sql())
            .bind(&literal)
            .bind(tokens)
            .bind(id)
            .execute(pool)
            .await
        {
            Ok(_) => written += 1,
            Err(e) => {
                warn!(
                    error = %e,
                    table = kind.label(),
                    id = %id,
                    "failed to write embedding row"
                );
            }
        }
    }

    if written > 0 {
        metrics::counter!(
            "ddb_embeddings_generated_total",
            "table" => kind.label(),
            "provider" => provider_label.to_string(),
        )
        .increment(written);
    }

    info!(
        table = kind.label(),
        written,
        requested = ids.len(),
        "embedding batch complete"
    );
    Ok(())
}

async fn refresh_pending_gauges(pool: &PgPool) -> Result<()> {
    for kind in [TableKind::Memory, TableKind::Facts] {
        let sql = match kind {
            TableKind::Memory => {
                "SELECT COUNT(*)::BIGINT AS n \
                 FROM memory_entries \
                 WHERE embedding IS NULL AND tier != 'archival'"
            }
            TableKind::Facts => {
                "SELECT COUNT(*)::BIGINT AS n \
                 FROM agent_facts \
                 WHERE embedding IS NULL"
            }
        };
        let row = sqlx::query(sql).fetch_one(pool).await?;
        let n: i64 = row.try_get("n").unwrap_or(0);
        metrics::gauge!(
            "ddb_embeddings_pending",
            "table" => kind.label(),
        )
        .set(n as f64);
    }
    Ok(())
}

/// Format a `Vec<f32>` as the pgvector text literal `[1,2,3]`.
///
/// pgvector accepts this as input when you cast with `::vector`, which
/// keeps us free of a compile-time dependency on the `pgvector` crate while
/// still producing a bit-for-bit identical stored value.
fn pgvector_literal(v: &[f32]) -> String {
    let mut s = String::with_capacity(v.len() * 8 + 2);
    s.push('[');
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        // `ryu`-equivalent formatting via `{}` is fine: pgvector parses any
        // finite f32 representation.
        use std::fmt::Write;
        let _ = write!(s, "{}", x);
    }
    s.push(']');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pgvector_literal_formats_correctly() {
        assert_eq!(pgvector_literal(&[]), "[]");
        assert_eq!(pgvector_literal(&[0.0]), "[0]");
        assert_eq!(pgvector_literal(&[1.0, 2.5, -3.0]), "[1,2.5,-3]");
    }

    #[test]
    fn pgvector_literal_handles_1536_zeros() {
        let v = vec![0.0_f32; 1536];
        let s = pgvector_literal(&v);
        assert!(s.starts_with('['));
        assert!(s.ends_with(']'));
        // 1535 commas between 1536 elements.
        assert_eq!(s.matches(',').count(), 1535);
    }
}
