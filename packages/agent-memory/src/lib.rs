// SPDX-License-Identifier: MIT
// Author: Darshankumar Joshi
// Part of DarshJDB — the native multi-model database.
//
// `ddb-agent-memory` — Phase 2.5 tiered agent memory layer.
//
// This crate hosts pluggable embedding providers and the background worker
// that fills `embedding` / `content_tokens` columns on `memory_entries` and
// `agent_facts` rows produced by the Phase 2 schema (see slice 12).
//
// The crate is intentionally framework-agnostic so it can be exercised in
// isolation from the main `ddb-server` binary. The server simply spawns the
// worker on startup when `DARSH_EMBEDDING_PROVIDER` is set to a non-`none`
// value.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

//! DarshJDB agent memory — pluggable embeddings + background worker.

/// Pluggable embedding providers (OpenAI, Ollama, Anthropic, None).
pub mod embedder;

/// Background worker that fills `embedding` + `content_tokens` columns.
pub mod worker;

pub use embedder::{
    AnthropicEmbeddingProvider, EmbeddingProvider, NoneProvider, OllamaEmbeddingProvider,
    OpenAIEmbeddingProvider, from_env,
};
pub use worker::{EmbeddingWorkerHandle, embedding_worker, spawn_embedding_worker};
