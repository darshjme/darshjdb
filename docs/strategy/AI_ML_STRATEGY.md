# DarshanDB AI/ML Strategy: Making a Self-Hosted BaaS AI-Native

> **Author:** Darsh Joshi  
> **Date:** 2026-04-05  
> **Status:** Strategic Proposal  
> **Scope:** v0.2 through v1.0

---

## Executive Summary

DarshanDB already has pgvector and a `$semantic` operator in its query language. This document lays out the plan to evolve that into a complete AI-native backend -- one where embeddings are generated automatically on insert, server functions have first-class AI primitives, queries understand natural language, mutations are guarded by ML-powered middleware, and the entire system speaks MCP so any AI agent can use it as a tool.

The thesis is simple: every BaaS will need AI capabilities within 18 months. Firebase added Vertex AI extensions as an afterthought. Supabase bolted on pgvector but left orchestration to the user. DarshanDB can make AI a first-class citizen from the data layer up -- not an extension, not a plugin, but part of the core.

---

## Table of Contents

1. [Vector Database Integration](#1-vector-database-integration)
2. [AI Server Functions](#2-ai-server-functions)
3. [Intelligent Queries](#3-intelligent-queries)
4. [AI Middleware](#4-ai-middleware)
5. [Agent-Friendly Design](#5-agent-friendly-design)
6. [Implementation Roadmap](#6-implementation-roadmap)
7. [Architecture Decisions](#7-architecture-decisions)
8. [Risk Analysis](#8-risk-analysis)

---

## 1. Vector Database Integration

### Current State

DarshanDB already ships with pgvector and exposes a `$semantic` query operator. What's missing is the pipeline around it: automatic embedding generation, chunking, hybrid search, and multi-model support.

### 1.1 Embedding Generation Pipeline

Every document stored in DarshanDB should be embeddable without the developer writing any pipeline code. The system auto-embeds on insert and re-embeds on update.

#### Schema Declaration

```typescript
// darshan.schema.ts
import { defineSchema, s } from '@darshan/server';

export default defineSchema({
  articles: {
    title: s.string(),
    body: s.string(),
    category: s.string(),
    
    // Declare which fields get embedded and how
    $embeddings: {
      // Embed title + body concatenated, using the project's default model
      content: {
        fields: ['title', 'body'],
        model: 'default',          // uses project-level config
        dimensions: 1536,
        chunking: 'paragraph',     // 'none' | 'paragraph' | 'sentence' | 'fixed'
        chunkSize: 512,            // max tokens per chunk
        chunkOverlap: 64,          // overlap between chunks
      },
      // Separate embedding for just the title (for title-similarity search)
      titleVec: {
        fields: ['title'],
        model: 'text-embedding-3-small',
        dimensions: 512,
      }
    }
  }
});
```

#### What Happens on Insert

```
Client calls db.transact(db.tx.articles[id].set({ title, body, category }))
    |
    v
Mutation Engine receives write
    |
    v
Embedding Interceptor checks $embeddings config for 'articles'
    |
    v
For each embedding declaration:
    1. Concatenate specified fields
    2. If chunking != 'none', split into chunks
    3. Enqueue embedding job (async, non-blocking to write path)
    |
    v
Write proceeds immediately (optimistic -- embedding arrives async)
    |
    v
Embedding Worker:
    1. Calls configured provider (OpenAI, Cohere, local ONNX, etc.)
    2. Stores vector(s) in pgvector column on the article row
    3. For chunked content: stores chunks in articles_chunks with FK
    4. Fires sync event so subscribed clients get updated data
```

#### Rust Implementation Sketch

```rust
// packages/server/src/ai/embeddings.rs

use pgvector::Vector;
use tokio::sync::mpsc;

pub struct EmbeddingJob {
    pub table: String,
    pub entity_id: Uuid,
    pub embedding_name: String,
    pub text: String,
    pub config: EmbeddingConfig,
}

pub struct EmbeddingConfig {
    pub model: ModelRef,
    pub dimensions: u32,
    pub chunking: ChunkStrategy,
    pub chunk_size: usize,
    pub chunk_overlap: usize,
}

pub enum ChunkStrategy {
    None,
    Paragraph,
    Sentence,
    Fixed,
    Semantic,  // uses embedding similarity to find natural break points
}

pub struct EmbeddingWorker {
    rx: mpsc::Receiver<EmbeddingJob>,
    providers: ProviderRegistry,
    db: PgPool,
}

impl EmbeddingWorker {
    pub async fn run(&mut self) {
        while let Some(job) = self.rx.recv().await {
            let chunks = self.chunk(&job.text, &job.config);
            let provider = self.providers.get(&job.config.model);
            
            // Batch embed all chunks in one API call
            let vectors = provider.embed_batch(&chunks).await?;
            
            // Store: single vector on the row, chunks in separate table
            if chunks.len() == 1 {
                self.store_single_vector(&job, &vectors[0]).await?;
            } else {
                self.store_chunked_vectors(&job, &chunks, &vectors).await?;
            }
            
            // Notify sync engine that this entity updated
            self.notify_sync(&job.table, &job.entity_id).await;
        }
    }
}
```

#### PostgreSQL Schema (Auto-Generated)

```sql
-- Auto-created when $embeddings declared on a table
ALTER TABLE articles 
    ADD COLUMN IF NOT EXISTS _emb_content vector(1536),
    ADD COLUMN IF NOT EXISTS _emb_titlevec vector(512);

-- For chunked embeddings
CREATE TABLE IF NOT EXISTS _darshan_chunks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_table TEXT NOT NULL,
    source_id UUID NOT NULL,
    embedding_name TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    chunk_text TEXT NOT NULL,
    embedding vector(1536),
    metadata JSONB DEFAULT '{}',
    created_at TIMESTAMPTZ DEFAULT now(),
    
    UNIQUE(source_table, source_id, embedding_name, chunk_index)
);

-- HNSW index for fast ANN search
CREATE INDEX IF NOT EXISTS idx_articles_emb_content 
    ON articles USING hnsw (_emb_content vector_cosine_ops);

CREATE INDEX IF NOT EXISTS idx_chunks_embedding
    ON _darshan_chunks USING hnsw (embedding vector_cosine_ops);
```

### 1.2 Semantic Search API

#### DarshanQL -- Enhanced `$semantic` Operator

```typescript
// Simple semantic search
const { data } = db.useQuery({
  articles: {
    $semantic: {
      query: "how to deploy a rust application",
      field: 'content',        // which embedding to search against
      limit: 10,
      threshold: 0.7,          // minimum cosine similarity
    }
  }
});

// Semantic search with metadata filtering (pre-filter, not post-filter)
const { data } = db.useQuery({
  articles: {
    $semantic: {
      query: "kubernetes deployment strategies",
      field: 'content',
      limit: 20,
    },
    $where: {
      category: 'devops',
      createdAt: { $gt: '2025-01-01' }
    }
  }
});

// Hybrid search: combine full-text + semantic with RRF fusion
const { data } = db.useQuery({
  articles: {
    $hybrid: {
      query: "rust async runtime",
      semantic: { field: 'content', weight: 0.7 },
      fulltext: { fields: ['title', 'body'], weight: 0.3 },
      fusion: 'rrf',           // Reciprocal Rank Fusion
      limit: 10,
    }
  }
});

// RAG-ready: search chunks, return with parent document context
const { data } = db.useQuery({
  articles: {
    $semantic: {
      query: "error handling patterns",
      field: 'content',
      mode: 'chunks',          // search at chunk level
      expandContext: 1,         // include 1 neighboring chunk on each side
      limit: 5,
    },
    // Still fetch parent document fields
    $select: ['title', 'category', 'author'],
    author: {}                 // traverse to related user
  }
});
```

#### Generated SQL for Hybrid Search

```sql
WITH semantic_results AS (
    SELECT id, 
           1 - (_emb_content <=> $1::vector) AS semantic_score,
           ROW_NUMBER() OVER (ORDER BY _emb_content <=> $1::vector) AS semantic_rank
    FROM articles
    WHERE category = 'devops'                          -- pre-filter
      AND 1 - (_emb_content <=> $1::vector) > 0.7     -- threshold
    ORDER BY _emb_content <=> $1::vector
    LIMIT 100
),
fulltext_results AS (
    SELECT id,
           ts_rank(to_tsvector('english', title || ' ' || body), plainto_tsquery($2)) AS ft_score,
           ROW_NUMBER() OVER (
               ORDER BY ts_rank(to_tsvector('english', title || ' ' || body), plainto_tsquery($2)) DESC
           ) AS ft_rank
    FROM articles
    WHERE to_tsvector('english', title || ' ' || body) @@ plainto_tsquery($2)
      AND category = 'devops'
    LIMIT 100
),
fused AS (
    -- Reciprocal Rank Fusion
    SELECT COALESCE(s.id, f.id) AS id,
           COALESCE(0.7 * (1.0 / (60 + s.semantic_rank)), 0) +
           COALESCE(0.3 * (1.0 / (60 + f.ft_rank)), 0) AS rrf_score
    FROM semantic_results s
    FULL OUTER JOIN fulltext_results f ON s.id = f.id
)
SELECT a.*, fused.rrf_score
FROM fused
JOIN articles a ON a.id = fused.id
ORDER BY fused.rrf_score DESC
LIMIT 10;
```

### 1.3 Multi-Model Support

```toml
# darshan.config.toml

[ai]
default_embedding_model = "openai/text-embedding-3-small"

[ai.providers.openai]
api_key = "${OPENAI_API_KEY}"

[ai.providers.anthropic]
api_key = "${ANTHROPIC_API_KEY}"

[ai.providers.cohere]
api_key = "${COHERE_API_KEY}"

[ai.providers.ollama]
base_url = "http://localhost:11434"

[ai.providers.local]
# ONNX Runtime -- runs embedding models locally, zero API calls
runtime = "onnx"
model_path = "./models/all-MiniLM-L6-v2.onnx"
tokenizer_path = "./models/tokenizer.json"
```

#### Provider Abstraction in Rust

```rust
// packages/server/src/ai/providers/mod.rs

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> u32;
    fn model_name(&self) -> &str;
    fn max_tokens(&self) -> usize;
}

#[async_trait]
pub trait CompletionProvider: Send + Sync {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
    async fn complete_stream(&self, request: CompletionRequest) -> Result<CompletionStream>;
    fn model_name(&self) -> &str;
    fn max_context(&self) -> usize;
}

pub struct ProviderRegistry {
    embedding: HashMap<String, Arc<dyn EmbeddingProvider>>,
    completion: HashMap<String, Arc<dyn CompletionProvider>>,
}

impl ProviderRegistry {
    pub fn get_embedder(&self, model_ref: &str) -> Result<Arc<dyn EmbeddingProvider>> {
        // "openai/text-embedding-3-small" -> provider=openai, model=text-embedding-3-small
        let (provider, model) = model_ref.split_once('/').ok_or(Error::InvalidModelRef)?;
        self.embedding.get(provider).ok_or(Error::ProviderNotConfigured)
    }
}
```

#### Local ONNX Runtime (Zero External Dependencies)

```rust
// packages/server/src/ai/providers/onnx.rs

use ort::{Session, SessionBuilder, Value};
use tokenizers::Tokenizer;

pub struct OnnxEmbeddingProvider {
    session: Session,
    tokenizer: Tokenizer,
    dimensions: u32,
}

impl OnnxEmbeddingProvider {
    pub fn load(model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
        let session = SessionBuilder::new()?
            .with_optimization_level(ort::GraphOptimizationLevel::Level3)?
            .with_intra_threads(4)?
            .commit_from_file(model_path)?;
        
        let tokenizer = Tokenizer::from_file(tokenizer_path)?;
        let dimensions = 384; // MiniLM-L6-v2
        
        Ok(Self { session, tokenizer, dimensions })
    }
}

#[async_trait]
impl EmbeddingProvider for OnnxEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self.tokenizer.encode(text, true)?;
        let input_ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
        let attention_mask: Vec<i64> = encoding.get_attention_mask().iter().map(|&m| m as i64).collect();
        
        let outputs = self.session.run(ort::inputs![
            "input_ids" => Value::from_array(([1, input_ids.len()], &input_ids))?,
            "attention_mask" => Value::from_array(([1, attention_mask.len()], &attention_mask))?,
        ]?)?;
        
        // Mean pooling over token embeddings
        let embeddings = outputs[0].extract_tensor::<f32>()?;
        let pooled = mean_pool(&embeddings, &attention_mask);
        Ok(normalize(&pooled))
    }
    
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        // Batch inference with dynamic padding
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
    
    fn dimensions(&self) -> u32 { self.dimensions }
    fn model_name(&self) -> &str { "onnx/all-MiniLM-L6-v2" }
    fn max_tokens(&self) -> usize { 256 }
}
```

---

## 2. AI Server Functions

Server functions are DarshanDB's answer to Convex functions -- sandboxed TypeScript/JavaScript executed in V8 isolates on the server. The AI extension gives every function access to embedding, completion, classification, and summarization primitives through the `ctx.ai` namespace.

### 2.1 API Surface

```typescript
// darshan/functions/ai-helpers.ts
import { action } from '@darshan/server';

// ctx.ai.embed -- Generate embeddings
export const getRelatedArticles = action(async (ctx, { articleId }) => {
  const article = await ctx.db.get('articles', articleId);
  
  // Generate embedding for the article's content
  const embedding = await ctx.ai.embed(article.title + ' ' + article.body);
  
  // Find similar articles using the raw vector
  const similar = await ctx.db.query({
    articles: {
      $vector: {
        field: 'content',
        vector: embedding,
        limit: 5,
        $where: { id: { $ne: articleId } }
      }
    }
  });
  
  return similar;
});

// ctx.ai.complete -- LLM completion
export const generateSummary = action(async (ctx, { articleId }) => {
  const article = await ctx.db.get('articles', articleId);
  
  const summary = await ctx.ai.complete({
    model: 'anthropic/claude-sonnet-4-20250514',   // or 'openai/gpt-4o', 'ollama/llama3'
    system: 'You are a technical writer. Summarize the following article in 2-3 sentences.',
    prompt: article.body,
    maxTokens: 200,
    temperature: 0.3,
  });
  
  // Store the summary back
  await ctx.db.patch('articles', articleId, { summary: summary.text });
  return summary.text;
});

// ctx.ai.complete with streaming
export const streamExplanation = action(async (ctx, { question, context }) => {
  const stream = await ctx.ai.complete({
    model: 'anthropic/claude-sonnet-4-20250514',
    system: 'You are a helpful assistant for our documentation.',
    prompt: `Context:\n${context}\n\nQuestion: ${question}`,
    stream: true,
  });
  
  // Return a streaming response to the client
  return ctx.stream(stream);
});

// ctx.ai.classify -- Zero-shot classification
export const categorizeTicket = action(async (ctx, { ticketId }) => {
  const ticket = await ctx.db.get('support_tickets', ticketId);
  
  const result = await ctx.ai.classify(ticket.description, {
    labels: ['billing', 'technical', 'account', 'feature-request', 'bug-report'],
    model: 'default',    // uses project default or local classifier
    multiLabel: false,
  });
  
  // result = { label: 'technical', confidence: 0.92, scores: { billing: 0.03, ... } }
  await ctx.db.patch('support_tickets', ticketId, {
    category: result.label,
    categoryConfidence: result.confidence,
  });
  
  return result;
});

// ctx.ai.summarize -- Purpose-built summarization
export const summarizeThread = action(async (ctx, { threadId }) => {
  const messages = await ctx.db.query({
    messages: {
      $where: { threadId },
      $order: { createdAt: 'asc' },
    }
  });
  
  const text = messages.map(m => `${m.author}: ${m.body}`).join('\n');
  
  const summary = await ctx.ai.summarize(text, {
    style: 'bullets',       // 'paragraph' | 'bullets' | 'headline'
    maxLength: 500,          // max characters
    focus: 'decisions',      // optional: what to emphasize
  });
  
  return summary;
});
```

### 2.2 RAG Pipeline as a First-Class Primitive

```typescript
// darshan/functions/rag.ts
import { action } from '@darshan/server';

export const askDocs = action(async (ctx, { question }) => {
  // One-liner RAG: search, retrieve context, generate answer
  const answer = await ctx.ai.rag({
    question,
    collection: 'documentation',         // which table to search
    embeddingField: 'content',            // which embedding to use
    topK: 5,                              // how many chunks to retrieve
    model: 'anthropic/claude-sonnet-4-20250514',    // which LLM for generation
    system: 'Answer based only on the provided context. If unsure, say so.',
    includesSources: true,                // return source references
  });
  
  // answer = {
  //   text: "To deploy with Docker, you can...",
  //   sources: [
  //     { id: "doc-123", title: "Docker Guide", chunk: "...", score: 0.94 },
  //     { id: "doc-456", title: "Production Setup", chunk: "...", score: 0.87 },
  //   ],
  //   usage: { promptTokens: 1200, completionTokens: 150 }
  // }
  
  return answer;
});
```

### 2.3 Provider Abstraction -- Unified Interface

```typescript
// How providers resolve at runtime:
// 1. "anthropic/claude-sonnet-4-20250514" -> AnthropicProvider with model claude-sonnet-4-20250514
// 2. "openai/gpt-4o"            -> OpenAIProvider with model gpt-4o
// 3. "ollama/llama3"            -> OllamaProvider at configured base_url
// 4. "local/classifier"         -> ONNX runtime with local model
// 5. "default"                  -> project-level default from darshan.config.toml

// Fallback chains:
// darshan.config.toml
// [ai.fallback]
// chain = ["anthropic/claude-sonnet-4-20250514", "openai/gpt-4o-mini", "ollama/llama3"]
// on_error = "next"       # try next provider on failure
// on_timeout = "next"     # try next if provider doesn't respond in 30s
// on_rate_limit = "next"  # try next if rate limited
```

### 2.4 Rust Implementation -- V8 Bindings

```rust
// packages/server/src/functions/ai_bindings.rs

use deno_core::op2;

/// Exposed to V8 as ctx.ai.embed()
#[op2(async)]
#[serde]
async fn op_ai_embed(
    state: &mut OpState,
    #[string] text: String,
    #[serde] options: Option<EmbedOptions>,
) -> Result<Vec<f32>, AnyError> {
    let registry = state.borrow::<Arc<ProviderRegistry>>();
    let model = options
        .as_ref()
        .and_then(|o| o.model.as_deref())
        .unwrap_or("default");
    
    let provider = registry.get_embedder(model)?;
    
    // Enforce sandbox limits: max text length, rate limiting per function
    let limits = state.borrow::<FunctionLimits>();
    limits.check_ai_budget(provider.model_name(), 1)?;
    
    let vector = provider.embed(&text).await?;
    Ok(vector)
}

/// Exposed to V8 as ctx.ai.complete()
#[op2(async)]
#[serde]
async fn op_ai_complete(
    state: &mut OpState,
    #[serde] request: CompletionRequest,
) -> Result<CompletionResponse, AnyError> {
    let registry = state.borrow::<Arc<ProviderRegistry>>();
    let provider = registry.get_completer(&request.model)?;
    
    let limits = state.borrow::<FunctionLimits>();
    limits.check_ai_budget(&request.model, estimate_tokens(&request))?;
    
    if request.stream {
        // Return a streaming resource handle
        let stream = provider.complete_stream(request).await?;
        let rid = state.resource_table.add(AiStreamResource::new(stream));
        Ok(CompletionResponse::Stream { resource_id: rid })
    } else {
        let response = provider.complete(request).await?;
        Ok(response)
    }
}

/// Register AI ops in the V8 runtime
pub fn ai_ops() -> Vec<deno_core::OpDecl> {
    vec![
        op_ai_embed(),
        op_ai_complete(),
        op_ai_classify(),
        op_ai_summarize(),
        op_ai_rag(),
    ]
}
```

### 2.5 Cost and Rate Limiting

```toml
# darshan.config.toml

[ai.limits]
# Per-function invocation limits
max_embedding_calls = 50         # max embed() calls per function run
max_completion_calls = 5         # max complete() calls per function run
max_tokens_per_call = 8000       # max tokens per single completion
max_total_tokens = 50000         # max tokens across all calls in one function run

# Per-project monthly limits (self-hosted users set their own)
monthly_embedding_budget = 10000000   # 10M tokens
monthly_completion_budget = 5000000   # 5M tokens

[ai.cache]
# Cache identical embedding requests
embedding_cache = true
embedding_cache_ttl = "7d"
# Cache completion responses for identical prompts (opt-in)
completion_cache = false
```

---

## 3. Intelligent Queries

### 3.1 Natural Language to DarshanQL Translation

Allow developers to expose a natural-language query interface to their end users. DarshanDB translates NL to DarshanQL using the project's schema as context.

```typescript
// darshan/functions/natural-query.ts
import { action } from '@darshan/server';

export const naturalQuery = action(async (ctx, { question }) => {
  // ctx.ai.toQuery() knows the project schema, permissions, and indexes
  const query = await ctx.ai.toQuery(question, {
    allowedTables: ['articles', 'users', 'comments'],  // scope for safety
    maxComplexity: 3,       // max join depth
    dryRun: false,          // if true, returns the DarshanQL without executing
  });
  
  // query.dql = {
  //   articles: {
  //     $where: { category: 'devops' },
  //     $order: { createdAt: 'desc' },
  //     $limit: 10,
  //     author: { $select: ['name', 'avatar'] }
  //   }
  // }
  // query.explanation = "Fetching the 10 most recent devops articles with author info"
  
  return { results: query.results, explanation: query.explanation, dql: query.dql };
});
```

#### Client SDK

```typescript
// In a React component
const { data, dql, explanation } = db.useNaturalQuery(
  "show me the top 10 devops articles with their authors",
  { allowedTables: ['articles', 'users'] }
);

// dql shows the generated query for transparency
// explanation is human-readable description of what was queried
```

### 3.2 Auto-Indexing Recommendations

DarshanDB's query engine already sees every query. The AI layer analyzes patterns and recommends indexes.

```rust
// packages/server/src/ai/auto_index.rs

pub struct QueryAnalyzer {
    /// Ring buffer of recent query patterns
    patterns: RwLock<VecDeque<QueryPattern>>,
    /// Frequency map: (table, field_combo) -> count
    frequency: DashMap<(String, Vec<String>), AtomicU64>,
}

impl QueryAnalyzer {
    /// Called by the query engine on every query execution
    pub fn record(&self, query: &QueryAST, execution_time: Duration) {
        let pattern = QueryPattern {
            tables: query.tables(),
            where_fields: query.where_fields(),
            order_fields: query.order_fields(),
            join_paths: query.join_paths(),
            execution_time,
            timestamp: Utc::now(),
        };
        
        // Update frequency map
        for (table, fields) in pattern.index_candidates() {
            self.frequency
                .entry((table, fields))
                .or_default()
                .fetch_add(1, Ordering::Relaxed);
        }
        
        self.patterns.write().push_back(pattern);
    }
    
    /// Generate recommendations (called periodically or on-demand)
    pub async fn recommend(&self) -> Vec<IndexRecommendation> {
        let mut recommendations = Vec::new();
        
        for entry in self.frequency.iter() {
            let ((table, fields), count) = entry.pair();
            let count = count.load(Ordering::Relaxed);
            
            if count > 100 {  // threshold: queried more than 100 times
                // Check if index already exists
                if !self.index_exists(&table, &fields).await {
                    // Estimate improvement using EXPLAIN ANALYZE sampling
                    let improvement = self.estimate_improvement(&table, &fields).await;
                    
                    if improvement.speedup_factor > 2.0 {
                        recommendations.push(IndexRecommendation {
                            table: table.clone(),
                            fields: fields.clone(),
                            index_type: self.recommend_type(&fields),
                            estimated_speedup: improvement.speedup_factor,
                            query_count: count,
                            sql: format!(
                                "CREATE INDEX idx_{}_{} ON {} ({})",
                                table, fields.join("_"),
                                table, fields.join(", ")
                            ),
                        });
                    }
                }
            }
        }
        
        recommendations.sort_by(|a, b| b.estimated_speedup.partial_cmp(&a.estimated_speedup).unwrap());
        recommendations
    }
}
```

#### Dashboard Integration

```
darshan index:suggest

  Recommended Indexes (based on 48h query analysis):
  
  1. articles(category, created_at)     -- 340 queries, ~4.2x speedup
     CREATE INDEX idx_articles_category_created ON articles(category, created_at);
     
  2. comments(article_id)               -- 890 queries, ~3.1x speedup  
     CREATE INDEX idx_comments_article_id ON comments(article_id);

  3. articles._emb_content (HNSW)       -- 120 semantic queries, ~8x speedup
     Already using ivfflat, recommend upgrading to HNSW for <10ms latency.

  Apply all? [y/N]
```

### 3.3 Query Result Caching with Semantic Similarity

```rust
// packages/server/src/ai/query_cache.rs

pub struct SemanticQueryCache {
    /// Store: (query_embedding, result_hash, result, timestamp)
    cache: Vec<(Vec<f32>, u64, CachedResult, Instant)>,
    embedder: Arc<dyn EmbeddingProvider>,
    similarity_threshold: f32,   // 0.95 = very similar queries get cache hit
    ttl: Duration,
}

impl SemanticQueryCache {
    /// Check if a semantically similar query was recently executed
    pub async fn get(&self, query_text: &str) -> Option<CachedResult> {
        let query_vec = self.embedder.embed(query_text).await.ok()?;
        
        for (cached_vec, _, result, timestamp) in &self.cache {
            if timestamp.elapsed() > self.ttl { continue; }
            
            let similarity = cosine_similarity(&query_vec, cached_vec);
            if similarity >= self.similarity_threshold {
                return Some(result.clone());
            }
        }
        None
    }
}
```

This is opt-in and primarily useful for natural language query endpoints where users phrase the same question differently.

---

## 4. AI Middleware

Middleware functions that run automatically on mutations, applying ML-powered validation, moderation, and protection.

### 4.1 Content Moderation

```typescript
// darshan/middleware/moderation.ts
import { middleware } from '@darshan/server';

export const contentModeration = middleware({
  // Run on every insert/update to these tables
  tables: ['comments', 'posts', 'messages'],
  trigger: 'before_write',
  
  async handler(ctx, { table, operation, data }) {
    // Check text fields for toxic content
    const textFields = ctx.schema.getTextFields(table);
    
    for (const field of textFields) {
      if (data[field]) {
        const result = await ctx.ai.classify(data[field], {
          labels: ['safe', 'toxic', 'spam', 'harassment', 'hate_speech'],
          model: 'local/moderation',  // fast local model
        });
        
        if (result.label !== 'safe' && result.confidence > 0.8) {
          // Option 1: Block the write
          // throw new DarshanError('CONTENT_MODERATED', `Content flagged: ${result.label}`);
          
          // Option 2: Allow write but flag for review
          data._moderation = {
            flagged: true,
            label: result.label,
            confidence: result.confidence,
            reviewedAt: null,
          };
          data._visible = false;  // hide until reviewed
        }
      }
    }
    
    return data;  // return (possibly modified) data to proceed
  }
});
```

### 4.2 PII Detection and Auto-Redaction

```typescript
// darshan/middleware/pii.ts
import { middleware } from '@darshan/server';

export const piiProtection = middleware({
  tables: ['*'],   // all tables
  trigger: 'before_write',
  
  async handler(ctx, { table, operation, data }) {
    const piiConfig = ctx.schema.getPiiConfig(table);
    if (!piiConfig) return data;
    
    for (const [field, config] of Object.entries(piiConfig)) {
      if (!data[field]) continue;
      
      const detected = await ctx.ai.detectPii(data[field], {
        types: ['email', 'phone', 'ssn', 'credit_card', 'address', 'name'],
      });
      
      if (detected.length > 0) {
        switch (config.action) {
          case 'redact':
            // Replace PII with tokens: "Call me at 555-1234" -> "Call me at [PHONE]"
            data[field] = redact(data[field], detected);
            break;
            
          case 'encrypt':
            // Store original encrypted, searchable version redacted
            data[`${field}_encrypted`] = await ctx.crypto.encrypt(data[field]);
            data[field] = redact(data[field], detected);
            break;
            
          case 'hash':
            // One-way hash the PII values for deduplication without storage
            data[`${field}_pii_hashes`] = detected.map(d => ctx.crypto.hash(d.value));
            data[field] = redact(data[field], detected);
            break;
            
          case 'block':
            throw new DarshanError('PII_DETECTED', `PII found in field ${field}`);
        }
      }
    }
    
    return data;
  }
});
```

#### Schema-Level PII Configuration

```typescript
// darshan.schema.ts
export default defineSchema({
  support_tickets: {
    subject: s.string(),
    description: s.string(),
    customerEmail: s.string(),
    
    $pii: {
      description: { action: 'redact', types: ['ssn', 'credit_card'] },
      customerEmail: { action: 'encrypt' },
    }
  }
});
```

### 4.3 Smart Data Validation -- Anomaly Detection

```typescript
// darshan/middleware/anomaly.ts
import { middleware } from '@darshan/server';

export const anomalyDetection = middleware({
  tables: ['transactions', 'orders'],
  trigger: 'before_write',
  
  async handler(ctx, { table, operation, data }) {
    if (operation !== 'insert') return data;
    
    // Get statistical profile for this table (computed periodically)
    const profile = await ctx.ai.getTableProfile(table);
    
    const anomalies = [];
    
    for (const [field, value] of Object.entries(data)) {
      const fieldProfile = profile[field];
      if (!fieldProfile || fieldProfile.type !== 'numeric') continue;
      
      // Z-score check: flag values >3 standard deviations from mean
      const zScore = Math.abs((value - fieldProfile.mean) / fieldProfile.stddev);
      if (zScore > 3) {
        anomalies.push({
          field,
          value,
          expected: `${fieldProfile.mean} +/- ${fieldProfile.stddev}`,
          zScore,
        });
      }
    }
    
    if (anomalies.length > 0) {
      // Log but don't block (configurable)
      await ctx.db.insert('_darshan_anomaly_log', {
        table,
        entityId: data.id,
        anomalies,
        timestamp: new Date(),
      });
      
      // Optionally notify via webhook
      if (ctx.config.anomaly?.webhook) {
        await ctx.fetch(ctx.config.anomaly.webhook, {
          method: 'POST',
          body: JSON.stringify({ table, anomalies, data }),
        });
      }
    }
    
    return data;
  }
});
```

---

## 5. Agent-Friendly Design

### 5.1 MCP (Model Context Protocol) Server

DarshanDB ships with a built-in MCP server, making it directly usable as a tool by Claude, GPT, and any MCP-compatible agent.

```rust
// packages/server/src/mcp/server.rs

use darshan_mcp::{McpServer, Tool, Resource, Prompt};

pub fn register_mcp_tools(server: &mut McpServer, db: &DarshanCore) {
    // Tool: Query data
    server.add_tool(Tool {
        name: "darshan_query",
        description: "Query data from DarshanDB using DarshanQL",
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "object",
                    "description": "DarshanQL query object"
                },
                "namespace": {
                    "type": "string",
                    "description": "Optional namespace for multi-tenant queries"
                }
            },
            "required": ["query"]
        }),
        handler: Arc::new(move |params| {
            let query = params["query"].clone();
            let result = db.query(query).await?;
            Ok(json!({ "data": result }))
        }),
    });
    
    // Tool: Mutate data
    server.add_tool(Tool {
        name: "darshan_transact",
        description: "Insert, update, or delete data in DarshanDB",
        input_schema: json!({
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": { "enum": ["set", "update", "delete"] },
                            "table": { "type": "string" },
                            "id": { "type": "string" },
                            "data": { "type": "object" }
                        }
                    }
                }
            },
            "required": ["operations"]
        }),
        handler: Arc::new(move |params| {
            let ops = params["operations"].as_array().unwrap();
            let result = db.transact(ops).await?;
            Ok(json!({ "success": true, "tx": result.tx_id }))
        }),
    });
    
    // Tool: Semantic search
    server.add_tool(Tool {
        name: "darshan_search",
        description: "Semantic search across any table with embeddings",
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Natural language search query" },
                "table": { "type": "string" },
                "limit": { "type": "integer", "default": 10 },
                "filters": { "type": "object", "description": "Additional WHERE filters" }
            },
            "required": ["query", "table"]
        }),
        handler: Arc::new(move |params| {
            let results = db.semantic_search(params).await?;
            Ok(json!({ "results": results }))
        }),
    });
    
    // Tool: Execute server function
    server.add_tool(Tool {
        name: "darshan_call",
        description: "Call a server function (query, mutation, or action)",
        input_schema: json!({
            "type": "object",
            "properties": {
                "function": { "type": "string", "description": "Function name (e.g., 'generateSummary')" },
                "args": { "type": "object", "description": "Arguments to pass to the function" }
            },
            "required": ["function"]
        }),
        handler: Arc::new(move |params| {
            let result = db.call_function(&params["function"], &params["args"]).await?;
            Ok(result)
        }),
    });
    
    // Resource: Schema introspection
    server.add_resource(Resource {
        uri: "darshan://schema",
        name: "Database Schema",
        description: "Current DarshanDB schema with all tables, fields, types, and relationships",
        mime_type: "application/json",
        handler: Arc::new(move || {
            Ok(db.schema().to_json())
        }),
    });
    
    // Resource: Table data preview
    server.add_resource_template(ResourceTemplate {
        uri_template: "darshan://tables/{table}/preview",
        name: "Table Preview",
        description: "Preview first 20 rows of a table",
        handler: Arc::new(move |table| {
            let data = db.query(json!({ table: { "$limit": 20 } })).await?;
            Ok(json!({ "table": table, "rows": data, "count": data.len() }))
        }),
    });
}
```

#### MCP Configuration

```toml
# darshan.config.toml

[mcp]
enabled = true
transport = "stdio"          # "stdio" | "sse" | "websocket"
auth = "api_key"             # MCP connections must authenticate
allowed_tools = ["darshan_query", "darshan_search", "darshan_call"]
# Restrict which functions agents can call
allowed_functions = ["askDocs", "naturalQuery", "generateSummary"]
# Rate limits specific to MCP (agents can be aggressive)
rate_limit = { rpm: 60, rpd: 10000 }
```

#### Agent Usage Example (Claude Desktop)

```json
{
  "mcpServers": {
    "darshandb": {
      "command": "darshan",
      "args": ["mcp", "--project", "/path/to/my-app"],
      "env": {
        "DARSHAN_API_KEY": "dsk_..."
      }
    }
  }
}
```

### 5.2 Tool-Use Ready REST Endpoints

For agents that don't support MCP, DarshanDB exposes the same capabilities through a tool-friendly REST API with OpenAPI 3.1 spec.

```
POST /api/v1/query          -- Execute DarshanQL query
POST /api/v1/transact       -- Execute mutations
POST /api/v1/search         -- Semantic search
POST /api/v1/functions/:name -- Call server function
GET  /api/v1/schema          -- Get schema (for agent context)
POST /api/v1/nl-query        -- Natural language query
GET  /api/v1/openapi.json    -- Full OpenAPI spec (agents can self-discover)
```

#### OpenAPI Spec Generation

```rust
// Auto-generate OpenAPI spec from schema + functions
// Agents can GET /api/v1/openapi.json to understand the full API surface

pub fn generate_openapi(schema: &Schema, functions: &[Function]) -> OpenApiSpec {
    let mut spec = OpenApiSpec::new("DarshanDB API", env!("CARGO_PKG_VERSION"));
    
    // Add schema-derived CRUD endpoints
    for table in schema.tables() {
        spec.add_path(&format!("/api/v1/tables/{}", table.name), /* ... */);
    }
    
    // Add function endpoints
    for func in functions {
        spec.add_path(
            &format!("/api/v1/functions/{}", func.name),
            PathItem {
                post: Some(Operation {
                    summary: func.description.clone(),
                    request_body: Some(func.args_schema()),
                    responses: func.return_schema(),
                    ..Default::default()
                }),
                ..Default::default()
            }
        );
    }
    
    spec
}
```

### 5.3 Session Management for Multi-Turn Agent Interactions

```typescript
// darshan/functions/agent-session.ts
import { action } from '@darshan/server';

export const agentChat = action(async (ctx, { sessionId, message }) => {
  // Get or create session
  let session = await ctx.db.get('_agent_sessions', sessionId);
  if (!session) {
    session = await ctx.db.insert('_agent_sessions', {
      id: sessionId,
      messages: [],
      context: {},
      createdAt: new Date(),
    });
  }
  
  // Append user message
  session.messages.push({ role: 'user', content: message });
  
  // Build context from recent relevant data
  const relevantData = await ctx.ai.rag({
    question: message,
    collection: 'documentation',
    topK: 3,
  });
  
  // Generate response with conversation history
  const response = await ctx.ai.complete({
    model: 'anthropic/claude-sonnet-4-20250514',
    system: `You are a helpful assistant. Use the following context:\n${relevantData.context}`,
    messages: session.messages,   // full conversation history
    maxTokens: 1000,
  });
  
  // Append assistant message
  session.messages.push({ role: 'assistant', content: response.text });
  
  // Update session
  await ctx.db.patch('_agent_sessions', sessionId, {
    messages: session.messages,
    lastActiveAt: new Date(),
  });
  
  return { text: response.text, sessionId };
});
```

### 5.4 Streaming Responses for LLM Outputs

```rust
// packages/server/src/api/streaming.rs

use axum::response::sse::{Event, Sse};
use futures::stream::Stream;

/// SSE endpoint for streaming LLM responses through DarshanDB
pub async fn stream_completion(
    State(state): State<AppState>,
    Json(request): Json<StreamRequest>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let provider = state.ai.get_completer(&request.model).unwrap();
    let stream = provider.complete_stream(request.into()).await.unwrap();
    
    Sse::new(stream.map(|chunk| {
        Ok(Event::default()
            .event("chunk")
            .data(serde_json::to_string(&chunk).unwrap()))
    }))
    .keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
    )
}
```

#### Client-Side Streaming

```typescript
// @darshan/react
function AiChat() {
  const [response, setResponse] = useState('');
  
  const ask = async (question: string) => {
    const stream = db.action.stream('streamExplanation', { question });
    
    for await (const chunk of stream) {
      setResponse(prev => prev + chunk.text);
    }
  };
  
  return <Chat onSend={ask} response={response} />;
}
```

---

## 6. Implementation Roadmap

### Phase 1: v0.2 -- "Vector Foundation" (8 weeks)

The goal: make DarshanDB the easiest self-hosted backend for building RAG applications.

| Week | Deliverable | Details |
|------|-------------|---------|
| 1-2 | Provider abstraction layer | `ProviderRegistry`, OpenAI + Ollama providers, config parsing |
| 2-3 | Embedding pipeline | Auto-embed on insert, chunking strategies, `_darshan_chunks` table |
| 3-4 | Enhanced `$semantic` operator | Threshold, pre-filtering, chunk-mode search |
| 4-5 | `$hybrid` operator | Full-text + semantic with RRF fusion |
| 5-6 | `ctx.ai.embed()` and `ctx.ai.complete()` | V8 bindings, sandbox limits, streaming |
| 6-7 | MCP server (basic) | `darshan_query`, `darshan_search`, `darshan_transact` tools |
| 7-8 | Testing, docs, examples | RAG example app, benchmark suite |

**Dependencies to add to Cargo.toml:**
```toml
# New AI-related dependencies
pgvector = "0.4"               # pgvector Rust types
ort = "2"                      # ONNX Runtime for local models
tokenizers = "0.20"            # HuggingFace tokenizers
async-openai = "0.25"          # OpenAI API client
# reqwest already in workspace for Anthropic/Cohere/Ollama HTTP calls
```

**Exit criteria:** A developer can define `$embeddings` on a table, insert documents, and run hybrid search queries with zero custom code. Claude can use DarshanDB as an MCP tool.

### Phase 2: v0.3 -- "Intelligence Layer" (8 weeks)

The goal: make DarshanDB smart about data flowing through it.

| Week | Deliverable | Details |
|------|-------------|---------|
| 1-2 | `ctx.ai.classify()` and `ctx.ai.summarize()` | V8 bindings, local classifier models |
| 2-3 | AI middleware framework | `before_write` / `after_write` hooks with `ctx.ai` access |
| 3-4 | Content moderation middleware | Built-in moderation, configurable per-table |
| 4-5 | PII detection and redaction | Regex + ML hybrid detector, encrypt/redact/block actions |
| 5-6 | `ctx.ai.rag()` primitive | One-liner RAG pipeline in server functions |
| 6-7 | NL-to-DarshanQL | Schema-aware query translation, `useNaturalQuery` hook |
| 7-8 | Auto-indexing recommendations | Query pattern analyzer, `darshan index:suggest` CLI |

**Exit criteria:** Content moderation works out of the box on any text field. Developers can build a chatbot over their data with a single `ctx.ai.rag()` call. PII is auto-detected.

### Phase 3: v1.0 -- "Agent Platform" (10 weeks)

The goal: make DarshanDB the backend of choice for AI agent applications.

| Week | Deliverable | Details |
|------|-------------|---------|
| 1-2 | Full MCP server | Resource templates, prompts, all tools, SSE + WebSocket transport |
| 2-3 | Agent session management | `_agent_sessions` table, conversation history, context windowing |
| 3-4 | Streaming responses | SSE streaming for completions, client SDK `stream()` support |
| 4-5 | Anomaly detection middleware | Statistical profiling, z-score detection, webhook alerts |
| 5-6 | Semantic query cache | Embedding-based cache deduplication for NL queries |
| 6-7 | Local ONNX runtime | Bundled `all-MiniLM-L6-v2`, zero-API-call embeddings |
| 7-8 | Provider fallback chains | Automatic failover between providers on error/timeout |
| 8-9 | Dashboard AI features | Embedding visualization, moderation queue, anomaly dashboard |
| 9-10 | Performance, security audit, docs | Benchmarks, penetration testing on AI endpoints, full docs |

**Exit criteria:** An agent framework (LangChain, CrewAI, Claude) can connect to DarshanDB via MCP, query data, mutate records, search semantically, and maintain multi-turn sessions -- all with proper auth and rate limiting. A developer can run the entire AI stack locally with zero API keys using ONNX models.

---

## 7. Architecture Decisions

### AD-1: Async Embedding Pipeline (Not Synchronous)

**Decision:** Embeddings are generated asynchronously after the write completes. The mutation returns immediately; the embedding arrives via a background worker.

**Rationale:** A synchronous embedding call on every insert adds 100-500ms latency (API round-trip) to every write. For bulk imports, this would be catastrophic. The async approach means writes stay at <5ms. The trade-off is that semantic search might miss very recently inserted documents (typically <2s gap).

**Mitigation:** Offer `$embeddings: { ... , sync: true }` for tables where immediate searchability matters more than write latency.

### AD-2: pgvector Over Dedicated Vector DB

**Decision:** Use pgvector inside the existing PostgreSQL instance rather than adding a separate vector database (Qdrant, Pinecone, Weaviate).

**Rationale:** DarshanDB's core promise is "one binary." Adding a separate vector database breaks that. pgvector handles millions of vectors with HNSW indexes and sub-10ms search. For the 99% use case (< 10M vectors per table), pgvector is sufficient. For the 1% who need billion-scale, they're not running on a $5 VPS anyway.

**Escape hatch:** The provider abstraction allows adding Qdrant/Pinecone as a vector storage backend in the future without changing the query API.

### AD-3: ONNX for Local Models

**Decision:** Use ONNX Runtime (via the `ort` crate) for local model inference instead of llama.cpp, candle, or other Rust-native ML runtimes.

**Rationale:** ONNX Runtime has the broadest model compatibility, hardware acceleration (CPU SIMD, CUDA, CoreML, DirectML), and production-grade stability. It supports embedding models (sentence-transformers), classifiers, and small sequence-to-sequence models. It does not support running large LLMs -- for those, users point to Ollama or an API provider.

### AD-4: Provider Abstraction with Fallback

**Decision:** All AI operations go through a provider abstraction with configurable fallback chains.

**Rationale:** Self-hosted users may have unreliable internet (or no internet). The fallback chain lets them configure `[anthropic -> ollama -> local/onnx]` so the system degrades gracefully. In air-gapped environments, the ONNX provider handles embeddings and classification without any network calls.

### AD-5: MCP as Primary Agent Interface

**Decision:** MCP (Model Context Protocol) is the primary way agents interact with DarshanDB, with REST as a fallback.

**Rationale:** MCP is becoming the standard for tool-use in AI agents (supported by Claude, adopted by OpenAI, integrated into VS Code, JetBrains, and others). By shipping an MCP server inside the DarshanDB binary, every DarshanDB instance becomes immediately usable by any MCP-compatible agent without additional setup.

---

## 8. Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| pgvector performance at scale (>10M vectors) | Medium | High | Benchmark early, document limits, plan Qdrant escape hatch |
| API key exposure in config | High | Critical | Environment variable resolution, encrypted config, never log keys |
| LLM hallucination in NL-to-DarshanQL | High | Medium | Always return generated DQL for inspection, allow `dryRun`, restrict allowed tables |
| ONNX model binary size bloating the binary | Medium | Medium | Models downloaded on first use, not bundled in binary |
| Provider API cost surprises | Medium | High | Per-function token budgets, monthly limits, dashboard cost tracking |
| Latency impact on write path | Low | High | Async pipeline (AD-1), sync opt-in only for specific tables |
| Security: prompt injection through stored data | Medium | High | Sanitize data before including in LLM prompts, use system prompts to instruct against injection |

---

## Appendix: Full `ctx.ai` API Reference

```typescript
interface DarshanAI {
  /** Generate embedding vector for text */
  embed(text: string, options?: {
    model?: string;           // default: project default
  }): Promise<number[]>;
  
  /** Generate embedding vectors for multiple texts */
  embedBatch(texts: string[], options?: {
    model?: string;
  }): Promise<number[][]>;
  
  /** LLM completion (single-turn or multi-turn) */
  complete(options: {
    model: string;
    prompt?: string;            // single-turn
    messages?: Message[];       // multi-turn
    system?: string;
    maxTokens?: number;
    temperature?: number;
    stream?: boolean;
    tools?: ToolDefinition[];   // function calling
  }): Promise<CompletionResponse | AsyncIterable<CompletionChunk>>;
  
  /** Zero-shot text classification */
  classify(text: string, options: {
    labels: string[];
    model?: string;
    multiLabel?: boolean;
    threshold?: number;         // confidence threshold for multiLabel
  }): Promise<ClassificationResult>;
  
  /** Text summarization */
  summarize(text: string, options?: {
    style?: 'paragraph' | 'bullets' | 'headline';
    maxLength?: number;
    focus?: string;
    model?: string;
  }): Promise<string>;
  
  /** One-liner RAG pipeline */
  rag(options: {
    question: string;
    collection: string;         // table name
    embeddingField?: string;    // which embedding to search
    topK?: number;
    model?: string;             // LLM for generation
    system?: string;
    includesSources?: boolean;
    filters?: Record<string, any>;
  }): Promise<RagResponse>;
  
  /** Translate natural language to DarshanQL */
  toQuery(question: string, options?: {
    allowedTables?: string[];
    maxComplexity?: number;
    dryRun?: boolean;
    model?: string;
  }): Promise<NaturalQueryResult>;
  
  /** Detect PII in text */
  detectPii(text: string, options?: {
    types?: PiiType[];
    model?: string;
  }): Promise<PiiDetection[]>;
  
  /** Get statistical profile for a table (for anomaly detection) */
  getTableProfile(table: string): Promise<TableProfile>;
}

// Response types

interface CompletionResponse {
  text: string;
  usage: { promptTokens: number; completionTokens: number; totalTokens: number };
  model: string;
  finishReason: 'stop' | 'max_tokens' | 'tool_use';
  toolCalls?: ToolCall[];
}

interface ClassificationResult {
  label: string;
  confidence: number;
  scores: Record<string, number>;
}

interface RagResponse {
  text: string;
  sources: Array<{
    id: string;
    table: string;
    chunk: string;
    score: number;
    metadata: Record<string, any>;
  }>;
  usage: { promptTokens: number; completionTokens: number };
}

interface NaturalQueryResult {
  dql: object;                // generated DarshanQL
  explanation: string;        // human-readable explanation
  results?: any;              // query results (if dryRun=false)
  confidence: number;         // how confident the model is in the translation
}

interface PiiDetection {
  type: PiiType;
  value: string;
  start: number;
  end: number;
  confidence: number;
}

type PiiType = 'email' | 'phone' | 'ssn' | 'credit_card' | 'address' | 'name' | 'dob' | 'passport';
```

---

*This document is a living strategy. Each phase will produce its own detailed technical spec before implementation begins.*
