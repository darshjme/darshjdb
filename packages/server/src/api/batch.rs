//! Redis-style command pipelining and batch operations for DarshanDB.
//!
//! Provides `POST /api/batch` for executing multiple operations (queries,
//! mutations, function calls) in a single HTTP round-trip. All mutations
//! within a batch share a single Postgres transaction for atomicity.
//!
//! # Design
//!
//! Operations are executed **sequentially** within the batch to preserve
//! ordering guarantees (a mutation in op N is visible to a query in op N+1).
//! Despite sequential execution, the single-round-trip design eliminates
//! N-1 network round trips, achieving Redis-pipelining-class throughput
//! for batched workloads.
//!
//! # Protocol
//!
//! ```json
//! POST /api/batch
//! {
//!   "ops": [
//!     { "type": "query",  "id": "q1", "body": { "users": { "$where": { "active": true } } } },
//!     { "type": "mutate", "id": "m1", "body": { "mutations": [...] } },
//!     { "type": "fn",     "id": "f1", "name": "hello", "args": { "name": "Darsh" } }
//!   ]
//! }
//! ```

use std::time::Instant;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::error::ApiError;
use super::rest::AppState;
use crate::query::{self, QueryResultRow};
use crate::triple_store::{PgTripleStore, TripleInput};

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Top-level batch request body.
#[derive(Debug, Deserialize)]
pub struct BatchRequest {
    /// Ordered list of operations to execute.
    pub ops: Vec<BatchOp>,
}

/// A single operation within a batch.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BatchOp {
    /// Execute a DarshanQL query.
    Query { id: String, body: Value },
    /// Execute one or more mutations (insert/update/delete/upsert).
    Mutate { id: String, body: Value },
    /// Invoke a server-side function.
    Fn {
        id: String,
        name: String,
        #[serde(default)]
        args: Value,
    },
}

/// Result of a single operation within a batch.
#[derive(Debug, Serialize)]
pub struct BatchOpResult {
    pub id: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Top-level batch response.
#[derive(Debug, Serialize)]
pub struct BatchResponse {
    pub results: Vec<BatchOpResult>,
    pub duration_ms: f64,
}

// ---------------------------------------------------------------------------
// Batch operation limits
// ---------------------------------------------------------------------------

/// Maximum number of operations allowed in a single batch.
const MAX_BATCH_OPS: usize = 50;

/// Maximum total mutations across all mutate ops in a batch.
const MAX_BATCH_MUTATIONS: usize = 200;

// ---------------------------------------------------------------------------
// Batch handler
// ---------------------------------------------------------------------------

/// `POST /api/batch` -- Execute multiple operations in a single request.
///
/// Operations are executed sequentially to preserve ordering semantics.
/// All mutations share a single Postgres transaction for atomicity:
/// if any mutation fails, the entire mutation set is rolled back.
pub async fn batch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<BatchRequest>,
) -> Result<Response, ApiError> {
    let batch_start = Instant::now();

    // Validate batch size.
    if body.ops.is_empty() {
        return Err(ApiError::bad_request(
            "Batch must contain at least one operation",
        ));
    }
    if body.ops.len() > MAX_BATCH_OPS {
        return Err(ApiError::bad_request(format!(
            "Batch exceeds maximum of {MAX_BATCH_OPS} operations (got {})",
            body.ops.len()
        )));
    }

    // Pre-validate: count total mutations to enforce limit.
    let mut total_mutations = 0usize;
    for op in &body.ops {
        if let BatchOp::Mutate {
            body: mutate_body, ..
        } = op
            && let Some(mutations) = mutate_body.get("mutations").and_then(|m| m.as_array())
        {
            total_mutations += mutations.len();
        }
    }
    if total_mutations > MAX_BATCH_MUTATIONS {
        return Err(ApiError::bad_request(format!(
            "Batch exceeds maximum of {MAX_BATCH_MUTATIONS} total mutations (got {total_mutations})"
        )));
    }

    // Extract bearer token once for the entire batch.
    let token = extract_bearer_token_from_headers(&headers).ok();

    // If there are any mutations, open a shared Postgres transaction.
    let has_mutations = body
        .ops
        .iter()
        .any(|op| matches!(op, BatchOp::Mutate { .. }));

    let mut db_tx =
        if has_mutations {
            Some(state.triple_store.begin_tx().await.map_err(|e| {
                ApiError::internal(format!("Failed to begin batch transaction: {e}"))
            })?)
        } else {
            None
        };

    let mut tx_id: Option<i64> = None;
    let mut all_entity_ids: Vec<Uuid> = Vec::new();
    let mut all_entity_types: Vec<String> = Vec::new();
    let mut results: Vec<BatchOpResult> = Vec::with_capacity(body.ops.len());

    // Execute each operation sequentially.
    for op in &body.ops {
        let result = match op {
            BatchOp::Query {
                id,
                body: query_body,
            } => execute_batch_query(id, query_body, &state).await,
            BatchOp::Mutate {
                id,
                body: mutate_body,
            } => {
                execute_batch_mutate(
                    id,
                    mutate_body,
                    &state,
                    db_tx.as_mut().expect("tx must exist for mutate ops"),
                    &mut tx_id,
                    &mut all_entity_ids,
                    &mut all_entity_types,
                )
                .await
            }
            BatchOp::Fn { id, name, args } => {
                execute_batch_fn(id, name, args, &state, token.as_deref()).await
            }
        };
        results.push(result);
    }

    // Commit the shared transaction if we had mutations.
    if let Some(tx) = db_tx {
        // Check if any mutation op failed -- if so, the tx is already
        // in an error state. We attempt commit anyway; Postgres will
        // return an error which we propagate.
        let any_mutation_failed = results.iter().any(|r| {
            // Only check mutation results (they have tx_id in data).
            r.status >= 400 && r.error.is_some()
        });

        if any_mutation_failed {
            // Explicit rollback for clarity.
            let _ = tx.rollback().await;
        } else {
            tx.commit()
                .await
                .map_err(|e| ApiError::internal(format!("Batch transaction commit failed: {e}")))?;

            // Invalidate caches for affected entity types.
            all_entity_types.sort();
            all_entity_types.dedup();
            for et in &all_entity_types {
                state.query_cache.invalidate_by_entity_type(et);
            }

            // Emit change events for reactive subscriptions.
            if let Some(tid) = tx_id
                && tid > 0
            {
                all_entity_ids.dedup();
                let _ = state.change_tx.send(crate::sync::broadcaster::ChangeEvent {
                    tx_id: tid,
                    entity_ids: all_entity_ids.iter().map(|id| id.to_string()).collect(),
                    attributes: Vec::new(),
                    entity_type: all_entity_types.into_iter().next(),
                    actor_id: None,
                });
            }
        }
    }

    let duration_ms = batch_start.elapsed().as_secs_f64() * 1000.0;
    let response = BatchResponse {
        results,
        duration_ms,
    };

    Ok(super::rest::negotiate_response_pub(&headers, &response))
}

// ---------------------------------------------------------------------------
// Individual operation executors
// ---------------------------------------------------------------------------

/// Execute a query operation within a batch.
async fn execute_batch_query(id: &str, query_body: &Value, state: &AppState) -> BatchOpResult {
    let start = Instant::now();

    // Parse the DarshanQL JSON into an AST.
    let ast = match query::parse_darshan_ql(query_body) {
        Ok(ast) => ast,
        Err(e) => {
            return BatchOpResult {
                id: id.to_string(),
                status: 400,
                data: None,
                error: Some(format!("Invalid query: {e}")),
            };
        }
    };

    // Plan the query.
    let plan = match query::plan_query(&ast) {
        Ok(plan) => plan,
        Err(e) => {
            return BatchOpResult {
                id: id.to_string(),
                status: 400,
                data: None,
                error: Some(format!("Query planning failed: {e}")),
            };
        }
    };

    // Execute against Postgres.
    let results: Vec<QueryResultRow> = match query::execute_query(&state.pool, &plan).await {
        Ok(rows) => rows,
        Err(e) => {
            return BatchOpResult {
                id: id.to_string(),
                status: 500,
                data: None,
                error: Some(format!("Query execution failed: {e}")),
            };
        }
    };

    // Record latency.
    state.pool_stats.record(start.elapsed());

    let data = serde_json::json!({
        "data": results,
        "meta": {
            "count": results.len(),
            "duration_ms": start.elapsed().as_secs_f64() * 1000.0,
        }
    });

    BatchOpResult {
        id: id.to_string(),
        status: 200,
        data: Some(data),
        error: None,
    }
}

/// Execute a mutation operation within a batch, using the shared transaction.
async fn execute_batch_mutate(
    id: &str,
    mutate_body: &Value,
    state: &AppState,
    db_tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    shared_tx_id: &mut Option<i64>,
    all_entity_ids: &mut Vec<Uuid>,
    all_entity_types: &mut Vec<String>,
) -> BatchOpResult {
    // Parse mutations array from the body.
    let mutations = match mutate_body.get("mutations").and_then(|m| m.as_array()) {
        Some(arr) => arr,
        None => {
            return BatchOpResult {
                id: id.to_string(),
                status: 400,
                data: None,
                error: Some("Missing or invalid 'mutations' array".to_string()),
            };
        }
    };

    if mutations.is_empty() {
        return BatchOpResult {
            id: id.to_string(),
            status: 400,
            data: None,
            error: Some("At least one mutation is required".to_string()),
        };
    }

    // Allocate tx_id once for the entire batch transaction.
    if shared_tx_id.is_none() {
        match PgTripleStore::next_tx_id_in_tx(db_tx).await {
            Ok(tid) => *shared_tx_id = Some(tid),
            Err(e) => {
                return BatchOpResult {
                    id: id.to_string(),
                    status: 500,
                    data: None,
                    error: Some(format!("Failed to allocate tx_id: {e}")),
                };
            }
        }
    }
    let tx_id = shared_tx_id.unwrap();

    let mut triples: Vec<TripleInput> = Vec::new();
    let mut entity_ids: Vec<Uuid> = Vec::new();

    for (i, mutation) in mutations.iter().enumerate() {
        let op = match mutation.get("op").and_then(|o| o.as_str()) {
            Some(op) => op,
            None => {
                return BatchOpResult {
                    id: id.to_string(),
                    status: 400,
                    data: None,
                    error: Some(format!("Mutation {i}: missing 'op' field")),
                };
            }
        };

        let entity = match mutation.get("entity").and_then(|e| e.as_str()) {
            Some(e) => e,
            None => {
                return BatchOpResult {
                    id: id.to_string(),
                    status: 400,
                    data: None,
                    error: Some(format!("Mutation {i}: missing 'entity' field")),
                };
            }
        };

        all_entity_types.push(entity.to_string());

        match op {
            "insert" | "set" => {
                let entity_id = mutation
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .unwrap_or_else(Uuid::new_v4);
                entity_ids.push(entity_id);

                // Add type triple.
                triples.push(TripleInput {
                    entity_id,
                    attribute: ":db/type".to_string(),
                    value: Value::String(entity.to_string()),
                    value_type: 0,
                    ttl_seconds: None,
                });

                // Add data triples.
                if let Some(data) = mutation.get("data").and_then(|d| d.as_object()) {
                    for (key, value) in data {
                        triples.push(TripleInput {
                            entity_id,
                            attribute: format!("{entity}/{key}"),
                            value: value.clone(),
                            value_type: infer_value_type(value),
                            ttl_seconds: None,
                        });
                    }
                }
            }
            "update" | "upsert" => {
                let entity_id = match mutation
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    Some(id) => id,
                    None => {
                        return BatchOpResult {
                            id: id.to_string(),
                            status: 400,
                            data: None,
                            error: Some(format!("Mutation {i}: 'id' required for {op}")),
                        };
                    }
                };
                entity_ids.push(entity_id);

                if let Some(data) = mutation.get("data").and_then(|d| d.as_object()) {
                    // Retract old values then assert new ones.
                    for (key, _) in data {
                        let attr = format!("{entity}/{key}");
                        if let Err(e) = PgTripleStore::retract_in_tx(db_tx, entity_id, &attr).await
                        {
                            return BatchOpResult {
                                id: id.to_string(),
                                status: 500,
                                data: None,
                                error: Some(format!("Retract failed: {e}")),
                            };
                        }
                    }
                    for (key, value) in data {
                        triples.push(TripleInput {
                            entity_id,
                            attribute: format!("{entity}/{key}"),
                            value: value.clone(),
                            value_type: infer_value_type(value),
                            ttl_seconds: None,
                        });
                    }
                }
            }
            "delete" => {
                let entity_id = match mutation
                    .get("id")
                    .and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok())
                {
                    Some(id) => id,
                    None => {
                        return BatchOpResult {
                            id: id.to_string(),
                            status: 400,
                            data: None,
                            error: Some(format!("Mutation {i}: 'id' required for delete")),
                        };
                    }
                };
                entity_ids.push(entity_id);

                // Retract all triples for this entity.
                match PgTripleStore::get_entity_in_tx(db_tx, entity_id).await {
                    Ok(existing) => {
                        for triple in &existing {
                            if let Err(e) =
                                PgTripleStore::retract_in_tx(db_tx, entity_id, &triple.attribute)
                                    .await
                            {
                                return BatchOpResult {
                                    id: id.to_string(),
                                    status: 500,
                                    data: None,
                                    error: Some(format!("Delete retract failed: {e}")),
                                };
                            }
                        }
                    }
                    Err(e) => {
                        return BatchOpResult {
                            id: id.to_string(),
                            status: 500,
                            data: None,
                            error: Some(format!("Failed to fetch entity for deletion: {e}")),
                        };
                    }
                }
            }
            _ => {
                return BatchOpResult {
                    id: id.to_string(),
                    status: 400,
                    data: None,
                    error: Some(format!("Mutation {i}: unknown op '{op}'")),
                };
            }
        }
    }

    // Write all triples inside the shared transaction.
    if !triples.is_empty()
        && let Err(e) = PgTripleStore::set_triples_in_tx(db_tx, &triples, tx_id).await
    {
        return BatchOpResult {
            id: id.to_string(),
            status: 500,
            data: None,
            error: Some(format!("Failed to write triples: {e}")),
        };
    }

    // Run forward-chaining rules inside the same transaction.
    if !triples.is_empty()
        && let Some(ref rule_engine) = state.rule_engine
        && let Err(e) = rule_engine
            .evaluate_and_write_in_tx(db_tx, &triples, tx_id)
            .await
    {
        return BatchOpResult {
            id: id.to_string(),
            status: 500,
            data: None,
            error: Some(format!("Rule engine error: {e}")),
        };
    }

    all_entity_ids.extend_from_slice(&entity_ids);

    BatchOpResult {
        id: id.to_string(),
        status: 200,
        data: Some(serde_json::json!({
            "tx_id": tx_id,
            "affected": mutations.len(),
            "entity_ids": entity_ids,
        })),
        error: None,
    }
}

/// Execute a function invocation within a batch.
async fn execute_batch_fn(
    id: &str,
    name: &str,
    args: &Value,
    state: &AppState,
    token: Option<&str>,
) -> BatchOpResult {
    // Validate function name.
    if name.is_empty() {
        return BatchOpResult {
            id: id.to_string(),
            status: 400,
            data: None,
            error: Some("Function name is required".to_string()),
        };
    }

    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-' || c == ':' || c == '/')
    {
        return BatchOpResult {
            id: id.to_string(),
            status: 400,
            data: None,
            error: Some("Function name contains invalid characters".to_string()),
        };
    }

    let registry = match state.function_registry.as_ref() {
        Some(r) => r,
        None => {
            return BatchOpResult {
                id: id.to_string(),
                status: 500,
                data: None,
                error: Some("Function registry not initialized".to_string()),
            };
        }
    };

    let runtime = match state.function_runtime.as_ref() {
        Some(r) => r,
        None => {
            return BatchOpResult {
                id: id.to_string(),
                status: 500,
                data: None,
                error: Some("Function runtime not initialized".to_string()),
            };
        }
    };

    // Look up the function.
    let function_def = match registry.get(name).await {
        Some(def) => def,
        None => {
            let all = registry.list().await;
            match all
                .into_iter()
                .find(|f| f.export_name == name || f.name.ends_with(&format!(":{name}")))
            {
                Some(def) => def,
                None => {
                    return BatchOpResult {
                        id: id.to_string(),
                        status: 404,
                        data: None,
                        error: Some(format!("Function `{name}` not found")),
                    };
                }
            }
        }
    };

    // Execute the function.
    match runtime
        .execute(&function_def, args.clone(), token.map(|s| s.to_string()))
        .await
    {
        Ok(result) => BatchOpResult {
            id: id.to_string(),
            status: 200,
            data: Some(serde_json::json!({
                "result": result.value,
                "duration_ms": result.duration_ms,
                "logs": result.logs,
            })),
            error: None,
        },
        Err(e) => BatchOpResult {
            id: id.to_string(),
            status: 500,
            data: None,
            error: Some(format!("Function execution failed: {e}")),
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract Bearer token from headers (batch-local helper to avoid
/// circular dependency on rest.rs private function).
fn extract_bearer_token_from_headers(headers: &HeaderMap) -> Result<String, ()> {
    headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or(())
}

/// Infer the triple store value_type discriminator from a JSON value.
fn infer_value_type(value: &Value) -> i16 {
    match value {
        Value::String(s) => {
            if s.len() == 36 && Uuid::parse_str(s).is_ok() {
                5 // Reference
            } else {
                0 // String
            }
        }
        Value::Number(n) => {
            if n.is_f64() && !n.is_i64() && !n.is_u64() {
                2 // Float
            } else {
                1 // Integer
            }
        }
        Value::Bool(_) => 3,
        Value::Object(_) | Value::Array(_) => 6,
        Value::Null => 0,
    }
}

// ---------------------------------------------------------------------------
// Parallel batch handler (Solana-inspired wave execution)
// ---------------------------------------------------------------------------

/// `POST /api/batch/parallel` -- Execute operations with Solana-inspired parallelism.
///
/// Analyzes each operation for the entity types it touches, groups non-conflicting
/// operations into waves, and executes each wave concurrently. Waves are processed
/// sequentially to preserve causal ordering between conflicting operations.
///
/// **Conflict model:**
/// - Two reads on the same entity type do NOT conflict (parallel).
/// - A read and a write on the same entity type DO conflict (sequential).
/// - Two writes on the same entity type DO conflict (sequential).
/// - Operations on different entity types never conflict (parallel).
///
/// Falls back to sequential execution for batches with mutations that share
/// a Postgres transaction (atomicity requires serialized writes).
pub async fn parallel_batch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<BatchRequest>,
) -> Result<Response, ApiError> {
    use crate::query::parallel::{compute_stats, profile_op, schedule_waves};

    let batch_start = Instant::now();

    // Validate batch size.
    if body.ops.is_empty() {
        return Err(ApiError::bad_request(
            "Batch must contain at least one operation",
        ));
    }
    if body.ops.len() > MAX_BATCH_OPS {
        return Err(ApiError::bad_request(format!(
            "Batch exceeds maximum of {MAX_BATCH_OPS} operations (got {})",
            body.ops.len()
        )));
    }

    // Pre-validate mutation count.
    let mut total_mutations = 0usize;
    for op in &body.ops {
        if let BatchOp::Mutate {
            body: mutate_body, ..
        } = op
            && let Some(mutations) = mutate_body.get("mutations").and_then(|m| m.as_array())
        {
            total_mutations += mutations.len();
        }
    }
    if total_mutations > MAX_BATCH_MUTATIONS {
        return Err(ApiError::bad_request(format!(
            "Batch exceeds maximum of {MAX_BATCH_MUTATIONS} total mutations (got {total_mutations})"
        )));
    }

    let has_mutations = body
        .ops
        .iter()
        .any(|op| matches!(op, BatchOp::Mutate { .. }));

    // If the batch contains mutations, fall back to sequential execution
    // because mutations share a single Postgres transaction for atomicity.
    if has_mutations {
        tracing::debug!(
            "parallel_batch: batch contains mutations, falling back to sequential execution"
        );
        return batch_handler(State(state), headers, axum::Json(body)).await;
    }

    // Profile each operation and schedule into waves.
    let profiles: Vec<_> = body
        .ops
        .iter()
        .enumerate()
        .map(|(i, op)| profile_op(i, op))
        .collect();

    let waves = schedule_waves(&profiles);
    let stats = compute_stats(&waves, body.ops.len());

    tracing::info!(
        total_ops = stats.total_ops,
        wave_count = stats.wave_count,
        parallel_ops = stats.parallel_ops,
        wave_sizes = ?stats.wave_sizes,
        "parallel_batch: scheduled {} ops into {} waves",
        stats.total_ops,
        stats.wave_count,
    );

    let token = extract_bearer_token_from_headers(&headers).ok();

    // Pre-allocate results vector (filled out of order, then sorted).
    let total_ops = body.ops.len();
    let mut results: Vec<Option<BatchOpResult>> = (0..total_ops).map(|_| None).collect();

    // Execute waves sequentially; within each wave, execute ops in parallel.
    for (wave_idx, wave) in waves.iter().enumerate() {
        if wave.op_indices.len() == 1 {
            // Single op in wave -- no need for tokio::join overhead.
            let idx = wave.op_indices[0];
            let result = execute_single_op(&body.ops[idx], &state, token.as_deref()).await;
            results[idx] = Some(result);
        } else {
            // Multiple ops in wave -- execute in parallel.
            let futs: Vec<_> = wave
                .op_indices
                .iter()
                .map(|&idx| {
                    let state_ref = &state;
                    let token_ref = token.as_deref();
                    let op = &body.ops[idx];
                    async move { (idx, execute_single_op(op, state_ref, token_ref).await) }
                })
                .collect();

            let wave_results = futures::future::join_all(futs).await;
            for (idx, result) in wave_results {
                results[idx] = Some(result);
            }
        }

        tracing::debug!(
            wave = wave_idx,
            ops = wave.op_indices.len(),
            "parallel_batch: completed wave {wave_idx}"
        );
    }

    // Unwrap results (all slots should be filled).
    let results: Vec<BatchOpResult> = results.into_iter().map(|r| r.unwrap()).collect();

    let duration_us = batch_start.elapsed().as_micros() as u64;
    let duration_ms = batch_start.elapsed().as_secs_f64() * 1000.0;

    // Record metrics.
    state.parallel_metrics.record_batch(
        stats.total_ops as u64,
        stats.parallel_ops as u64,
        stats.wave_count as u64,
        duration_us,
    );

    let response = ParallelBatchResponse {
        results,
        duration_ms,
        waves: stats.wave_count,
        parallel_ops: stats.parallel_ops,
        sequential_ops: stats.total_ops - stats.parallel_ops,
    };

    Ok(super::rest::negotiate_response_pub(&headers, &response))
}

/// Execute a single read-only operation (query or function call).
async fn execute_single_op(op: &BatchOp, state: &AppState, token: Option<&str>) -> BatchOpResult {
    match op {
        BatchOp::Query { id, body } => execute_batch_query(id, body, state).await,
        BatchOp::Fn { id, name, args } => execute_batch_fn(id, name, args, state, token).await,
        BatchOp::Mutate { id, .. } => {
            // Should not reach here in the parallel path, but handle gracefully.
            BatchOpResult {
                id: id.clone(),
                status: 400,
                data: None,
                error: Some("Mutations not supported in parallel batch mode".to_string()),
            }
        }
    }
}

/// Extended batch response that includes parallelism metrics.
#[derive(Debug, Serialize)]
pub struct ParallelBatchResponse {
    pub results: Vec<BatchOpResult>,
    pub duration_ms: f64,
    /// Number of execution waves (fewer waves = more parallelism).
    pub waves: usize,
    /// Number of operations that ran in parallel (wave size > 1).
    pub parallel_ops: usize,
    /// Number of operations that ran sequentially.
    pub sequential_ops: usize,
}

/// `GET /api/batch/metrics` -- Return parallel execution metrics.
pub async fn parallel_metrics_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let snapshot = state.parallel_metrics.snapshot();
    Ok(super::rest::negotiate_response_pub(&headers, &snapshot))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_op_deserializes_query() {
        let json = r#"{"type":"query","id":"q1","body":{"users":{"$where":{"active":true}}}}"#;
        let op: BatchOp = serde_json::from_str(json).unwrap();
        match op {
            BatchOp::Query { id, body } => {
                assert_eq!(id, "q1");
                assert!(body.get("users").is_some());
            }
            _ => panic!("expected Query variant"),
        }
    }

    #[test]
    fn batch_op_deserializes_mutate() {
        let json = r#"{"type":"mutate","id":"m1","body":{"mutations":[{"op":"set","entity":"todos","data":{"title":"New"}}]}}"#;
        let op: BatchOp = serde_json::from_str(json).unwrap();
        match op {
            BatchOp::Mutate { id, body } => {
                assert_eq!(id, "m1");
                let mutations = body["mutations"].as_array().unwrap();
                assert_eq!(mutations.len(), 1);
            }
            _ => panic!("expected Mutate variant"),
        }
    }

    #[test]
    fn batch_op_deserializes_fn() {
        let json = r#"{"type":"fn","id":"f1","name":"hello","args":{"name":"Darsh"}}"#;
        let op: BatchOp = serde_json::from_str(json).unwrap();
        match op {
            BatchOp::Fn { id, name, args } => {
                assert_eq!(id, "f1");
                assert_eq!(name, "hello");
                assert_eq!(args["name"], "Darsh");
            }
            _ => panic!("expected Fn variant"),
        }
    }

    #[test]
    fn batch_op_fn_default_args() {
        let json = r#"{"type":"fn","id":"f1","name":"ping"}"#;
        let op: BatchOp = serde_json::from_str(json).unwrap();
        match op {
            BatchOp::Fn { args, .. } => {
                assert!(args.is_null());
            }
            _ => panic!("expected Fn variant"),
        }
    }

    #[test]
    fn batch_request_deserializes_mixed_ops() {
        let json = r#"{
            "ops": [
                {"type":"query","id":"q1","body":{"users":{}}},
                {"type":"mutate","id":"m1","body":{"mutations":[]}},
                {"type":"fn","id":"f1","name":"greet","args":{}}
            ]
        }"#;
        let req: BatchRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.ops.len(), 3);
    }

    #[test]
    fn batch_response_serializes_correctly() {
        let resp = BatchResponse {
            results: vec![
                BatchOpResult {
                    id: "q1".to_string(),
                    status: 200,
                    data: Some(serde_json::json!({"data": []})),
                    error: None,
                },
                BatchOpResult {
                    id: "m1".to_string(),
                    status: 500,
                    data: None,
                    error: Some("tx failed".to_string()),
                },
            ],
            duration_ms: 12.5,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["results"].as_array().unwrap().len(), 2);
        assert_eq!(json["results"][0]["status"], 200);
        assert!(json["results"][0]["error"].is_null());
        assert_eq!(json["results"][1]["status"], 500);
        assert!(json["results"][1]["data"].is_null());
        assert_eq!(json["duration_ms"], 12.5);
    }

    #[test]
    fn infer_value_type_string() {
        assert_eq!(infer_value_type(&Value::String("hello".into())), 0);
    }

    #[test]
    fn infer_value_type_uuid_reference() {
        assert_eq!(
            infer_value_type(&Value::String(
                "550e8400-e29b-41d4-a716-446655440000".into()
            )),
            5
        );
    }

    #[test]
    fn infer_value_type_integer() {
        assert_eq!(infer_value_type(&serde_json::json!(42)), 1);
    }

    #[test]
    fn infer_value_type_float() {
        assert_eq!(infer_value_type(&serde_json::json!(2.78_f64)), 2);
    }

    #[test]
    fn infer_value_type_bool() {
        assert_eq!(infer_value_type(&serde_json::json!(true)), 3);
    }

    #[test]
    fn infer_value_type_object() {
        assert_eq!(infer_value_type(&serde_json::json!({"a": 1})), 6);
    }
}
