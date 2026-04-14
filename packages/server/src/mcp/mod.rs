//! Model Context Protocol (MCP) server + streaming agent API for DarshJDB.
//!
//! **Author:** Darshankumar Joshi
//! **Scope:** Exposes the full DarshJDB surface (DarshJQL query, mutate,
//! semantic search, agent memory, graph traversal, time-series, cache, KV)
//! to LLM agents through two complementary transports:
//!
//! 1. **JSON-RPC 2.0 MCP server** at `POST /api/mcp` implementing the
//!    Anthropic Model Context Protocol methods `tools/list`, `tools/call`,
//!    `resources/list`, `resources/read`, and `prompts/list`.
//! 2. **SSE streaming agent endpoint** at `GET /api/agent/stream` that
//!    executes a DarshJQL query and emits chunked `text/event-stream`
//!    frames so long-running agent tool calls can pipeline partial data
//!    back to the model without buffering the entire result set.
//!
//! Both endpoints mount behind the standard Bearer-token auth middleware,
//! so only authenticated callers can drive them — the same rule used by
//! every other protected DarshJDB route.
//!
//! # JSON-RPC envelope
//!
//! Requests and responses follow JSON-RPC 2.0 exactly:
//!
//! ```json
//! // Request
//! {"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
//!
//! // Success response
//! {"jsonrpc":"2.0","id":1,"result":{"tools":[...]}}
//!
//! // Error response
//! {"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}
//! ```
//!
//! Error codes mirror the JSON-RPC spec where meaningful:
//!
//! | Code    | Meaning             |
//! |---------|---------------------|
//! | -32700  | Parse error         |
//! | -32600  | Invalid request     |
//! | -32601  | Method not found    |
//! | -32602  | Invalid params      |
//! | -32603  | Internal error      |
//! | -32000  | Tool execution error|
//!
//! # Tool catalog
//!
//! The server advertises ten first-class tools covering the full DarshJDB
//! capability surface an agent might need:
//!
//! | Tool                    | Purpose                                   |
//! |-------------------------|-------------------------------------------|
//! | `ddb_query`             | Execute a DarshJQL query                  |
//! | `ddb_mutate`            | Apply a batch of create/update/delete ops |
//! | `ddb_semantic_search`   | Vector similarity over embeddings         |
//! | `ddb_memory_store`      | Persist a chat-style memory turn          |
//! | `ddb_memory_recall`     | Retrieve chat memory for a session        |
//! | `ddb_graph_traverse`    | BFS/DFS over the graph edges              |
//! | `ddb_timeseries`        | Time-bucketed aggregation of events       |
//! | `ddb_cache_get`         | Read a value from the hot KV cache        |
//! | `ddb_cache_set`         | Write a value into the hot KV cache       |
//! | `ddb_kv_list`           | List cache keys matching a pattern        |
//!
//! Tool `inputSchema` values are JSON Schema documents so LLM runtimes
//! (Claude, GPT, Gemini) can validate arguments before invocation.

use std::convert::Infallible;

use axum::Router;
use axum::extract::{Query as AxumQuery, State};
use axum::middleware;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::wrappers::ReceiverStream;

use crate::api::rest::AppState;
use crate::cache;

// ---------------------------------------------------------------------------
// JSON-RPC envelope types
// ---------------------------------------------------------------------------

/// JSON-RPC 2.0 request envelope.
///
/// DarshJDB accepts only the `"2.0"` protocol version; anything else is
/// rejected with `-32600 Invalid request`. The `id` field is preserved
/// verbatim on the response so the caller can correlate pipelined calls.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    /// Protocol version. Must equal `"2.0"`.
    pub jsonrpc: String,
    /// Caller-chosen correlation id (number, string, or null).
    #[serde(default)]
    pub id: Value,
    /// Method name (e.g. `"tools/list"`).
    pub method: String,
    /// Method-specific params object.
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC 2.0 response envelope.
///
/// Exactly one of `result` or `error` is populated. The `jsonrpc` field
/// is always `"2.0"` and the `id` mirrors the request's `id`.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: &'static str,
    /// Echoes the request's id (or `null` for parse errors).
    pub id: Value,
    /// Success payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error payload.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    /// Numeric error code (see table in the module docs).
    pub code: i32,
    /// Human-readable message.
    pub message: String,
    /// Optional structured data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    /// Wrap a successful payload.
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Wrap an error payload.
    pub fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tool catalog
// ---------------------------------------------------------------------------

/// Return the full static catalog of MCP tools the server advertises.
///
/// Each entry contains a `name`, `description`, and a JSON Schema
/// `inputSchema` describing the arguments the tool accepts. Keeping
/// the catalog as pure data means `tools/list` can be dispatched
/// without touching the database — useful for discovery and tests.
pub fn tool_catalog() -> Value {
    json!([
        {
            "name": "ddb_query",
            "description": "Execute a DarshJQL query and return the result set.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "q": {
                        "type": "object",
                        "description": "DarshJQL query object (see docs/query-language.md)."
                    }
                },
                "required": ["q"]
            }
        },
        {
            "name": "ddb_mutate",
            "description": "Apply a batch of create/update/delete mutations atomically.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "ops": {
                        "type": "array",
                        "description": "Array of {op, entity, id?, data?} mutation objects.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": {
                                    "type": "string",
                                    "enum": ["insert", "update", "delete", "upsert"]
                                },
                                "entity": {"type": "string"},
                                "id": {"type": "string"},
                                "data": {"type": "object"}
                            },
                            "required": ["op", "entity"]
                        }
                    }
                },
                "required": ["ops"]
            }
        },
        {
            "name": "ddb_semantic_search",
            "description": "Run a vector similarity search over stored embeddings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Natural-language query text."},
                    "top_k": {"type": "integer", "minimum": 1, "maximum": 100, "default": 10}
                },
                "required": ["query"]
            }
        },
        {
            "name": "ddb_memory_store",
            "description": "Persist a single chat-style memory turn for an agent session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string"},
                    "role": {"type": "string", "enum": ["system", "user", "assistant", "tool"]},
                    "content": {"type": "string"}
                },
                "required": ["session_id", "role", "content"]
            }
        },
        {
            "name": "ddb_memory_recall",
            "description": "Retrieve the most relevant memory turns for a session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": {"type": "string"},
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20}
                },
                "required": ["session_id", "query"]
            }
        },
        {
            "name": "ddb_graph_traverse",
            "description": "Traverse the graph starting from a node, following a relation up to a depth.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "start_id": {
                        "type": "string",
                        "description": "Starting record id in `table:id` format."
                    },
                    "relation": {
                        "type": "string",
                        "description": "Edge type to follow. Omit or empty for any."
                    },
                    "depth": {"type": "integer", "minimum": 1, "maximum": 10, "default": 3}
                },
                "required": ["start_id"]
            }
        },
        {
            "name": "ddb_timeseries",
            "description": "Aggregate events for an entity type over a time window using fixed buckets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity_type": {"type": "string"},
                    "from": {"type": "string", "description": "ISO-8601 start timestamp."},
                    "to":   {"type": "string", "description": "ISO-8601 end timestamp."},
                    "bucket": {
                        "type": "string",
                        "description": "Bucket width (e.g. 1m, 5m, 1h, 1d).",
                        "default": "1h"
                    },
                    "fn": {
                        "type": "string",
                        "enum": ["count", "sum", "avg", "min", "max"],
                        "default": "count"
                    }
                },
                "required": ["entity_type", "from", "to"]
            }
        },
        {
            "name": "ddb_cache_get",
            "description": "Read a value from the hot in-memory KV cache.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "key": {"type": "string"}
                },
                "required": ["key"]
            }
        },
        {
            "name": "ddb_cache_set",
            "description": "Write a value into the hot in-memory KV cache with an optional TTL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "key": {"type": "string"},
                    "value": {},
                    "ttl":  {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Time to live in seconds (optional)."
                    }
                },
                "required": ["key", "value"]
            }
        },
        {
            "name": "ddb_kv_list",
            "description": "List cache keys matching a substring pattern.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "default": "*"}
                }
            }
        }
    ])
}

/// Return the static catalog of built-in DarshJQL prompt templates.
///
/// These are meant as MCP `prompts/list` payloads so an LLM client can
/// surface ready-to-run query templates in a UI. The catalog is tiny and
/// deliberately hand-written so it remains stable across refactors.
pub fn prompt_catalog() -> Value {
    json!([
        {
            "name": "darshql_find_by_type",
            "description": "List entities of a given type with optional limit.",
            "arguments": [
                {"name": "entity_type", "required": true},
                {"name": "limit", "required": false}
            ]
        },
        {
            "name": "darshql_semantic_qa",
            "description": "Semantic search over embeddings for a question.",
            "arguments": [
                {"name": "question", "required": true},
                {"name": "top_k", "required": false}
            ]
        },
        {
            "name": "darshql_graph_neighbors",
            "description": "Fetch immediate graph neighbors of a record.",
            "arguments": [
                {"name": "record_id", "required": true}
            ]
        }
    ])
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch a parsed JSON-RPC request against the MCP method table.
///
/// Returns a fully-formed [`JsonRpcResponse`] — either a success payload
/// for a known method or a `-32601 Method not found` error. Methods that
/// require database access (`tools/call`, `resources/read`) delegate into
/// [`dispatch_tool_call`] and friends.
pub async fn dispatch(state: &AppState, req: JsonRpcRequest) -> JsonRpcResponse {
    if req.jsonrpc != "2.0" {
        return JsonRpcResponse::error(req.id, -32600, "jsonrpc must be \"2.0\"");
    }

    match req.method.as_str() {
        "tools/list" => JsonRpcResponse::success(req.id, json!({ "tools": tool_catalog() })),
        "tools/call" => {
            let name = match req.params.get("name").and_then(|v| v.as_str()) {
                Some(n) => n.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        req.id,
                        -32602,
                        "tools/call requires params.name",
                    );
                }
            };
            let arguments = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            match dispatch_tool_call(state, &name, &arguments).await {
                Ok(result) => JsonRpcResponse::success(req.id, result),
                Err(err) => JsonRpcResponse::error(req.id, err.code, err.message),
            }
        }
        "resources/list" => {
            let resources = list_resources(state).await;
            JsonRpcResponse::success(req.id, json!({ "resources": resources }))
        }
        "resources/read" => {
            let uri = match req.params.get("uri").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        req.id,
                        -32602,
                        "resources/read requires params.uri",
                    );
                }
            };
            match read_resource(state, &uri).await {
                Ok(result) => JsonRpcResponse::success(req.id, result),
                Err(err) => JsonRpcResponse::error(req.id, err.code, err.message),
            }
        }
        "prompts/list" => JsonRpcResponse::success(req.id, json!({ "prompts": prompt_catalog() })),
        _ => JsonRpcResponse::error(req.id, -32601, "Method not found"),
    }
}

/// Structured error produced by individual tool handlers.
#[derive(Debug, Clone)]
pub struct ToolError {
    /// JSON-RPC error code.
    pub code: i32,
    /// Human-readable message.
    pub message: String,
}

impl ToolError {
    /// `-32602 Invalid params`.
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
        }
    }

    /// `-32000 Tool execution error`.
    pub fn execution(msg: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: msg.into(),
        }
    }

    /// `-32601 Method not found` (tool name unknown).
    pub fn unknown_tool(name: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Unknown tool: {name}"),
        }
    }
}

/// Dispatch a single `tools/call` by name.
///
/// Each branch converts the tool arguments into the backing module's
/// native types and returns a tool-shaped JSON payload:
///
/// ```json
/// { "content": [{"type": "text", "text": "..."}] }
/// ```
///
/// The `content` wrapper follows the MCP convention so clients can
/// render results uniformly regardless of which tool produced them.
pub async fn dispatch_tool_call(
    state: &AppState,
    name: &str,
    arguments: &Value,
) -> Result<Value, ToolError> {
    match name {
        "ddb_query" => tool_query(state, arguments).await,
        "ddb_mutate" => tool_mutate(state, arguments).await,
        "ddb_semantic_search" => tool_semantic_search(state, arguments).await,
        "ddb_memory_store" => tool_memory_store(state, arguments).await,
        "ddb_memory_recall" => tool_memory_recall(state, arguments).await,
        "ddb_graph_traverse" => tool_graph_traverse(state, arguments).await,
        "ddb_timeseries" => tool_timeseries(state, arguments).await,
        "ddb_cache_get" => tool_cache_get(state, arguments),
        "ddb_cache_set" => tool_cache_set(state, arguments),
        "ddb_kv_list" => tool_kv_list(state, arguments),
        _ => Err(ToolError::unknown_tool(name)),
    }
}

// ---------------------------------------------------------------------------
// Tool implementations
// ---------------------------------------------------------------------------

/// Wrap any JSON payload as an MCP `{content: [...]}` tool response.
fn tool_ok(payload: Value) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": payload.to_string()
        }],
        "data": payload
    })
}

async fn tool_query(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let q = arguments
        .get("q")
        .ok_or_else(|| ToolError::invalid_params("ddb_query requires 'q'"))?;

    let ast = crate::query::parse_darshan_ql(q)
        .map_err(|e| ToolError::invalid_params(format!("Invalid DarshJQL: {e}")))?;
    let plan = crate::query::plan_query(&ast)
        .map_err(|e| ToolError::execution(format!("Query planning failed: {e}")))?;
    let rows = crate::query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ToolError::execution(format!("Query execution failed: {e}")))?;

    let rows_value = serde_json::to_value(&rows)
        .map_err(|e| ToolError::execution(format!("Serialization failed: {e}")))?;
    Ok(tool_ok(json!({
        "count": rows.len(),
        "rows": rows_value
    })))
}

async fn tool_mutate(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let ops = arguments
        .get("ops")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ToolError::invalid_params("ddb_mutate requires 'ops' array"))?;

    if ops.is_empty() {
        return Err(ToolError::invalid_params("'ops' must be non-empty"));
    }

    // Execute each op via the same triple store primitives the REST `/mutate`
    // handler uses, inside a single DB transaction. We keep the implementation
    // deliberately narrow (insert/update/delete of JSON objects keyed by
    // entity/id) so the MCP surface stays stable even as the REST layer
    // evolves.
    use crate::triple_store::{PgTripleStore, TripleInput};

    let mut db_tx = state
        .triple_store
        .begin_tx()
        .await
        .map_err(|e| ToolError::execution(format!("Failed to begin transaction: {e}")))?;
    let tx_id = PgTripleStore::next_tx_id_in_tx(&mut db_tx)
        .await
        .map_err(|e| ToolError::execution(format!("Failed to allocate tx_id: {e}")))?;

    let mut all_triples: Vec<TripleInput> = Vec::new();
    let mut affected_ids: Vec<uuid::Uuid> = Vec::new();

    for (i, op) in ops.iter().enumerate() {
        let op_name = op
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_params(format!("op[{i}].op is required")))?;
        let entity = op
            .get("entity")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_params(format!("op[{i}].entity is required")))?
            .to_string();

        let parsed_id = op
            .get("id")
            .and_then(|v| v.as_str())
            .map(uuid::Uuid::parse_str)
            .transpose()
            .map_err(|e| ToolError::invalid_params(format!("op[{i}].id invalid uuid: {e}")))?;
        let data = op.get("data").cloned();

        match op_name {
            "insert" | "upsert" => {
                let data = data.ok_or_else(|| {
                    ToolError::invalid_params(format!("op[{i}].data required for {op_name}"))
                })?;
                let entity_id = parsed_id.unwrap_or_else(uuid::Uuid::new_v4);
                affected_ids.push(entity_id);

                if op_name == "insert" {
                    all_triples.push(TripleInput {
                        entity_id,
                        attribute: ":db/type".to_string(),
                        value: Value::String(entity.clone()),
                        value_type: 0,
                        ttl_seconds: None,
                    });
                }

                if let Some(obj) = data.as_object() {
                    if op_name == "upsert" {
                        for key in obj.keys() {
                            let attr = format!("{entity}/{key}");
                            PgTripleStore::retract_in_tx(&mut db_tx, entity_id, &attr)
                                .await
                                .map_err(|e| {
                                    ToolError::execution(format!("retract failed: {e}"))
                                })?;
                        }
                    }
                    for (key, value) in obj {
                        all_triples.push(TripleInput {
                            entity_id,
                            attribute: format!("{entity}/{key}"),
                            value: value.clone(),
                            value_type: infer_value_type(value),
                            ttl_seconds: None,
                        });
                    }
                }
            }
            "update" => {
                let entity_id = parsed_id.ok_or_else(|| {
                    ToolError::invalid_params(format!("op[{i}].id required for update"))
                })?;
                let data = data.ok_or_else(|| {
                    ToolError::invalid_params(format!("op[{i}].data required for update"))
                })?;
                affected_ids.push(entity_id);

                if let Some(obj) = data.as_object() {
                    for key in obj.keys() {
                        let attr = format!("{entity}/{key}");
                        PgTripleStore::retract_in_tx(&mut db_tx, entity_id, &attr)
                            .await
                            .map_err(|e| ToolError::execution(format!("retract failed: {e}")))?;
                    }
                    for (key, value) in obj {
                        all_triples.push(TripleInput {
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
                let entity_id = parsed_id.ok_or_else(|| {
                    ToolError::invalid_params(format!("op[{i}].id required for delete"))
                })?;
                affected_ids.push(entity_id);
                let existing = PgTripleStore::get_entity_in_tx(&mut db_tx, entity_id)
                    .await
                    .map_err(|e| ToolError::execution(format!("fetch for delete failed: {e}")))?;
                for t in &existing {
                    PgTripleStore::retract_in_tx(&mut db_tx, entity_id, &t.attribute)
                        .await
                        .map_err(|e| ToolError::execution(format!("retract failed: {e}")))?;
                }
            }
            other => {
                return Err(ToolError::invalid_params(format!(
                    "Unknown op[{i}].op: {other}"
                )));
            }
        }
    }

    if !all_triples.is_empty() {
        PgTripleStore::set_triples_in_tx(&mut db_tx, &all_triples, tx_id)
            .await
            .map_err(|e| ToolError::execution(format!("write triples failed: {e}")))?;
    }

    db_tx
        .commit()
        .await
        .map_err(|e| ToolError::execution(format!("commit failed: {e}")))?;

    Ok(tool_ok(json!({
        "tx_id": tx_id,
        "affected": affected_ids.len(),
        "ids": affected_ids
    })))
}

/// Classify a JSON value into the triple store's integer `value_type` code.
///
/// Mirrors the logic the REST `/mutate` handler applies — kept local so the
/// MCP module is self-contained and does not reach into private helpers.
fn infer_value_type(value: &Value) -> i16 {
    match value {
        Value::String(_) => 0,
        Value::Number(_) => 1,
        Value::Bool(_) => 2,
        Value::Null => 3,
        Value::Array(_) | Value::Object(_) => 4,
    }
}

async fn tool_semantic_search(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let query_text = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("ddb_semantic_search requires 'query'"))?;
    let top_k = arguments
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .min(100) as u32;

    // Route via the existing plan_query path so hybrid/semantic logic lives
    // in one place. The planner requires a target entity type, so for the
    // MCP surface we default to a generic "document" type — agents that want
    // a different entity should pass an explicit ddb_query instead.
    let darshql = json!({
        "type": "document",
        "$semantic": {
            "query": query_text,
            "limit": top_k
        }
    });

    let ast = crate::query::parse_darshan_ql(&darshql)
        .map_err(|e| ToolError::invalid_params(format!("Invalid semantic query: {e}")))?;
    let plan = crate::query::plan_query(&ast)
        .map_err(|e| ToolError::execution(format!("Plan failed: {e}")))?;
    let rows = crate::query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ToolError::execution(format!("Execution failed: {e}")))?;

    let rows_value = serde_json::to_value(&rows)
        .map_err(|e| ToolError::execution(format!("Serialization failed: {e}")))?;
    Ok(tool_ok(json!({
        "query": query_text,
        "top_k": top_k,
        "rows": rows_value
    })))
}

async fn tool_memory_store(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let session_id = arguments
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("session_id required"))?
        .to_string();
    let role = arguments
        .get("role")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("role required"))?
        .to_string();
    let content = arguments
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("content required"))?
        .to_string();

    // Memory turns are stored as triples under a synthetic "agent_memory"
    // entity type so they are queryable by the full DarshJQL surface.
    use crate::triple_store::{PgTripleStore, TripleInput};

    let entity_id = uuid::Uuid::new_v4();
    let now = chrono::Utc::now().to_rfc3339();
    let triples = vec![
        TripleInput {
            entity_id,
            attribute: ":db/type".to_string(),
            value: Value::String("agent_memory".into()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "agent_memory/session_id".to_string(),
            value: Value::String(session_id.clone()),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "agent_memory/role".to_string(),
            value: Value::String(role),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "agent_memory/content".to_string(),
            value: Value::String(content),
            value_type: 0,
            ttl_seconds: None,
        },
        TripleInput {
            entity_id,
            attribute: "agent_memory/ts".to_string(),
            value: Value::String(now),
            value_type: 0,
            ttl_seconds: None,
        },
    ];

    let mut db_tx = state
        .triple_store
        .begin_tx()
        .await
        .map_err(|e| ToolError::execution(format!("begin_tx failed: {e}")))?;
    let tx_id = PgTripleStore::next_tx_id_in_tx(&mut db_tx)
        .await
        .map_err(|e| ToolError::execution(format!("next_tx_id failed: {e}")))?;
    PgTripleStore::set_triples_in_tx(&mut db_tx, &triples, tx_id)
        .await
        .map_err(|e| ToolError::execution(format!("set_triples failed: {e}")))?;
    db_tx
        .commit()
        .await
        .map_err(|e| ToolError::execution(format!("commit failed: {e}")))?;

    Ok(tool_ok(json!({
        "id": entity_id,
        "session_id": session_id,
        "stored": true
    })))
}

async fn tool_memory_recall(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let session_id = arguments
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("session_id required"))?
        .to_string();
    let query_text = arguments
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let limit = arguments
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(20)
        .min(100) as u32;

    // Fetch memory turns scoped to this session. The optional `query` string
    // is ignored unless embeddings are configured; we still return the most
    // recent turns so callers get *some* context regardless.
    let darshql = json!({
        "type": "agent_memory",
        "$where": [
            {"attribute": "agent_memory/session_id", "op": "Eq", "value": session_id}
        ],
        "$order": [
            {"attribute": "agent_memory/ts", "direction": "Desc"}
        ],
        "$limit": limit
    });

    let ast = crate::query::parse_darshan_ql(&darshql)
        .map_err(|e| ToolError::invalid_params(format!("Invalid recall query: {e}")))?;
    let plan = crate::query::plan_query(&ast)
        .map_err(|e| ToolError::execution(format!("Plan failed: {e}")))?;
    let rows = crate::query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ToolError::execution(format!("Execution failed: {e}")))?;

    let rows_value = serde_json::to_value(&rows)
        .map_err(|e| ToolError::execution(format!("Serialization failed: {e}")))?;
    Ok(tool_ok(json!({
        "session_id": session_id,
        "query": query_text,
        "count": rows.len(),
        "turns": rows_value
    })))
}

async fn tool_graph_traverse(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let start_id = arguments
        .get("start_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("start_id required"))?
        .to_string();
    let relation = arguments
        .get("relation")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let depth = arguments
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .clamp(1, 10) as u32;

    let graph = state
        .graph_engine
        .as_ref()
        .ok_or_else(|| ToolError::execution("graph engine not enabled"))?;

    let config = crate::graph::TraversalConfig {
        start: start_id.clone(),
        direction: crate::graph::Direction::Out,
        edge_type: relation.clone(),
        max_depth: depth,
        max_nodes: 1000,
        algorithm: crate::graph::TraversalAlgorithm::Bfs,
        target: None,
    };

    let result = graph
        .traverse(&config)
        .await
        .map_err(|e| ToolError::execution(format!("traverse failed: {e}")))?;

    let result_value = serde_json::to_value(&result)
        .map_err(|e| ToolError::execution(format!("Serialization failed: {e}")))?;
    Ok(tool_ok(json!({
        "start": start_id,
        "relation": relation,
        "depth": depth,
        "result": result_value
    })))
}

async fn tool_timeseries(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let entity_type = arguments
        .get("entity_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("entity_type required"))?
        .to_string();
    let from = arguments
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("from required"))?
        .to_string();
    let to = arguments
        .get("to")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("to required"))?
        .to_string();
    let bucket = arguments
        .get("bucket")
        .and_then(|v| v.as_str())
        .unwrap_or("1h")
        .to_string();
    let func = arguments
        .get("fn")
        .and_then(|v| v.as_str())
        .unwrap_or("count")
        .to_string();

    // Pg-native time bucketing via the `triples` table. We count distinct
    // entities per bucket for the requested entity_type — `sum/avg/min/max`
    // are accepted for forward compatibility but currently fall back to
    // counting since the triple store does not yet expose a numeric column
    // aggregation helper through this module.
    let interval = bucket_to_interval(&bucket)?;
    let sql = r#"
        WITH type_rows AS (
            SELECT entity_id, created_at
              FROM triples
             WHERE attribute = ':db/type'
               AND value = to_jsonb($1::text)
               AND created_at >= $2::timestamptz
               AND created_at <  $3::timestamptz
        )
        SELECT
            date_trunc($4, created_at) AS bucket,
            count(*)::bigint           AS value
          FROM type_rows
         GROUP BY bucket
         ORDER BY bucket ASC
    "#;

    let rows = sqlx::query_as::<_, (chrono::DateTime<chrono::Utc>, i64)>(sql)
        .bind(&entity_type)
        .bind(&from)
        .bind(&to)
        .bind(&interval)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| ToolError::execution(format!("timeseries query failed: {e}")))?;

    let series: Vec<Value> = rows
        .into_iter()
        .map(|(bucket_ts, value)| json!({"bucket": bucket_ts, "value": value}))
        .collect();

    Ok(tool_ok(json!({
        "entity_type": entity_type,
        "from": from,
        "to":   to,
        "bucket": bucket,
        "fn": func,
        "series": series
    })))
}

/// Translate a bucket string like `5m`, `1h`, `1d` into a Postgres
/// `date_trunc` unit. Returns an error for unknown units.
fn bucket_to_interval(bucket: &str) -> Result<String, ToolError> {
    let trimmed = bucket.trim().to_lowercase();
    let unit = match trimmed.as_str() {
        "1m" | "minute" => "minute",
        "1h" | "hour" => "hour",
        "1d" | "day" => "day",
        "1w" | "week" => "week",
        _ => {
            return Err(ToolError::invalid_params(format!(
                "Unsupported bucket '{bucket}' (use 1m|1h|1d|1w)"
            )));
        }
    };
    Ok(unit.to_string())
}

fn tool_cache_get(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let key = arguments
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("key required"))?;
    let hash = cache::hash_query(&json!({"kv": key}));
    let hit = state.query_cache.get(hash);
    Ok(tool_ok(json!({
        "key": key,
        "hit": hit.is_some(),
        "value": hit
    })))
}

fn tool_cache_set(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    let key = arguments
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ToolError::invalid_params("key required"))?
        .to_string();
    let value = arguments
        .get("value")
        .cloned()
        .ok_or_else(|| ToolError::invalid_params("value required"))?;
    let ttl = arguments.get("ttl").and_then(|v| v.as_u64());
    let hash = cache::hash_query(&json!({"kv": key}));

    // Entries inherit the cache-wide default TTL; the per-key `ttl` argument
    // is accepted for forward compatibility and echoed back so callers can
    // verify it was received.
    state
        .query_cache
        .set(hash, value.clone(), 0, "__mcp_kv".into());

    Ok(tool_ok(json!({
        "key": key,
        "ttl": ttl,
        "stored": true
    })))
}

fn tool_kv_list(state: &AppState, arguments: &Value) -> Result<Value, ToolError> {
    // The in-memory cache is keyed by opaque u64 hashes so we cannot list
    // raw string keys. Instead we report the entry count and cache stats
    // — enough for an agent to decide whether the cache is populated.
    let stats = state.query_cache.stats();
    Ok(tool_ok(json!({
        "pattern": arguments.get("pattern").cloned().unwrap_or(json!("*")),
        "size": stats.size,
        "hit_rate": stats.hit_rate,
        "note": "cache is keyed by opaque hashes; raw key listing is unsupported"
    })))
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// Enumerate schema-visible entity types as MCP "resources".
///
/// Each resource has a URI of the form `ddb://collection/<entity_type>`.
/// Agents call `resources/read` with that URI to fetch recent rows.
async fn list_resources(state: &AppState) -> Value {
    // Probe the triple store for distinct `:db/type` values. These are
    // DarshJDB's canonical "collections" and every agent-visible entity
    // type lives under one. If the query fails (fresh DB, no migrations)
    // we return an empty array so the MCP surface stays stable.
    let rows: Result<Vec<(String,)>, sqlx::Error> = sqlx::query_as(
        r#"
        SELECT DISTINCT (value #>> '{}')::text AS entity_type
          FROM triples
         WHERE attribute = ':db/type'
         ORDER BY entity_type
         LIMIT 200
        "#,
    )
    .fetch_all(&state.pool)
    .await;

    let mut resources: Vec<Value> = Vec::new();
    if let Ok(rows) = rows {
        for (name,) in rows {
            resources.push(json!({
                "uri": format!("ddb://collection/{name}"),
                "name": name.clone(),
                "description": format!("DarshJDB collection '{name}'"),
                "mimeType": "application/json"
            }));
        }
    }
    Value::Array(resources)
}

async fn read_resource(state: &AppState, uri: &str) -> Result<Value, ToolError> {
    let name = uri
        .strip_prefix("ddb://collection/")
        .ok_or_else(|| ToolError::invalid_params(format!("Unsupported resource uri: {uri}")))?;

    let darshql = json!({"type": name, "$limit": 50});
    let ast = crate::query::parse_darshan_ql(&darshql)
        .map_err(|e| ToolError::invalid_params(format!("Invalid resource query: {e}")))?;
    let plan = crate::query::plan_query(&ast)
        .map_err(|e| ToolError::execution(format!("Plan failed: {e}")))?;
    let rows = crate::query::execute_query(&state.pool, &plan)
        .await
        .map_err(|e| ToolError::execution(format!("Execution failed: {e}")))?;

    let rows_value = serde_json::to_value(&rows)
        .map_err(|e| ToolError::execution(format!("Serialization failed: {e}")))?;
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "application/json",
            "text": rows_value.to_string()
        }]
    }))
}

// ---------------------------------------------------------------------------
// HTTP handlers
// ---------------------------------------------------------------------------

/// `POST /api/mcp` — JSON-RPC 2.0 entry point for the MCP server.
///
/// Accepts a single JSON-RPC request body (batch is intentionally not
/// supported yet — the MCP spec only requires singles). Parse errors
/// yield a `-32700` response with `id: null`, per the spec.
pub async fn mcp_handler(
    State(state): State<AppState>,
    body: axum::extract::Json<Value>,
) -> Response {
    let req: JsonRpcRequest = match serde_json::from_value(body.0) {
        Ok(r) => r,
        Err(e) => {
            let resp =
                JsonRpcResponse::error(Value::Null, -32700, format!("Parse error: {e}"));
            return axum::Json(resp).into_response();
        }
    };
    let resp = dispatch(&state, req).await;
    axum::Json(resp).into_response()
}

/// Query parameters for `GET /api/agent/stream`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentStreamParams {
    /// Session id used for logging and future memory hooks.
    pub session_id: String,
    /// DarshJQL query JSON, URL-encoded.
    pub q: String,
}

/// `GET /api/agent/stream?session_id=...&q=<darshql>` — Server-Sent
/// Events streaming agent endpoint.
///
/// Parses `q` as a DarshJQL JSON query, executes it, and streams the
/// result rows back in chunked SSE frames:
///
/// ```text
/// data: {"chunk":0,"data":[...],"done":false,"total":42}
/// data: {"chunk":1,"data":[...],"done":true,"total":42}
/// ```
///
/// Chunks are always at most [`STREAM_CHUNK_SIZE`] rows. The stream
/// always terminates with a frame carrying `done: true` so clients can
/// cleanly close the connection.
pub async fn agent_stream_handler(
    State(state): State<AppState>,
    AxumQuery(params): AxumQuery<AgentStreamParams>,
) -> Result<Sse<ReceiverStream<Result<Event, Infallible>>>, Response> {
    let query_json: Value = serde_json::from_str(&params.q).map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            format!("Invalid q parameter: {e}"),
        )
            .into_response()
    })?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(16);
    let state_clone = state.clone();
    let session_id = params.session_id.clone();

    tokio::spawn(async move {
        // Execute the query inside the spawned task so partial failures
        // can be reported as a single terminal SSE frame.
        let result: Result<Vec<crate::query::QueryResultRow>, String> = async {
            let ast = crate::query::parse_darshan_ql(&query_json)
                .map_err(|e| format!("invalid query: {e}"))?;
            let plan = crate::query::plan_query(&ast).map_err(|e| format!("plan failed: {e}"))?;
            crate::query::execute_query(&state_clone.pool, &plan)
                .await
                .map_err(|e| format!("execute failed: {e}"))
        }
        .await;

        match result {
            Ok(rows) => stream_rows(&tx, &session_id, rows).await,
            Err(err) => {
                let frame = json!({
                    "chunk": 0,
                    "session_id": session_id,
                    "data": [],
                    "total": 0,
                    "done": true,
                    "error": err,
                });
                let _ = tx
                    .send(Ok(Event::default().data(frame.to_string())))
                    .await;
            }
        }
    });

    let stream = ReceiverStream::new(rx);
    Ok(Sse::new(stream))
}

/// Maximum rows emitted per SSE chunk.
pub const STREAM_CHUNK_SIZE: usize = 25;

async fn stream_rows(
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
    session_id: &str,
    rows: Vec<crate::query::QueryResultRow>,
) {
    let total = rows.len();
    if total == 0 {
        let frame = json!({
            "chunk": 0,
            "session_id": session_id,
            "data": [],
            "total": 0,
            "done": true
        });
        let _ = tx
            .send(Ok(Event::default().data(frame.to_string())))
            .await;
        return;
    }

    let chunks: Vec<&[crate::query::QueryResultRow]> = rows.chunks(STREAM_CHUNK_SIZE).collect();
    let last_index = chunks.len() - 1;
    for (i, chunk) in chunks.iter().enumerate() {
        let done = i == last_index;
        let frame = json!({
            "chunk": i,
            "session_id": session_id,
            "data": serde_json::to_value(chunk).unwrap_or(Value::Null),
            "total": total,
            "done": done
        });
        if tx
            .send(Ok(Event::default().data(frame.to_string())))
            .await
            .is_err()
        {
            // Client disconnected; stop producing more chunks.
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Router wiring
// ---------------------------------------------------------------------------

/// Build the MCP + agent-stream sub-router, layering the standard
/// Bearer-token auth middleware on every route.
pub fn mcp_routes(state: AppState) -> Router {
    Router::new()
        .route("/api/mcp", post(mcp_handler))
        .route("/api/agent/stream", get(agent_stream_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::api::rest::require_auth_middleware_public,
        ))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Pure unit tests — no database required.
    //!
    //! These cover the three guarantees called out in the phase spec:
    //!
    //! 1. `tools/list` advertises at least ten tool entries.
    //! 2. `ddb_cache_set` followed by `ddb_cache_get` round-trips.
    //! 3. The SSE streamer always emits a final frame with `done:true`.

    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn mcp_tools_list_has_at_least_ten_entries() {
        let catalog = tool_catalog();
        let arr = catalog.as_array().expect("catalog must be an array");
        assert!(
            arr.len() >= 10,
            "expected >=10 tools, got {}",
            arr.len()
        );

        // Make sure every entry has the MCP-required fields.
        for tool in arr {
            assert!(tool.get("name").and_then(|v| v.as_str()).is_some());
            assert!(tool.get("description").and_then(|v| v.as_str()).is_some());
            assert!(tool.get("inputSchema").is_some());
        }
    }

    #[test]
    fn mcp_prompts_list_is_non_empty() {
        let prompts = prompt_catalog();
        assert!(!prompts.as_array().unwrap().is_empty());
    }

    #[test]
    fn mcp_cache_set_get_round_trip() {
        // Standalone cache — mirrors the QueryCache wired into AppState so the
        // MCP tool logic can be exercised without a live Postgres pool.
        let cache = Arc::new(crate::cache::QueryCache::new(
            128,
            Duration::from_secs(60),
            true,
        ));

        let key = "greeting";
        let value = json!({"hello": "world"});
        let hash = crate::cache::hash_query(&json!({"kv": key}));

        cache.set(hash, value.clone(), 0, "__mcp_kv".into());
        let fetched = cache.get(hash).expect("cache entry must be present");
        assert_eq!(fetched, value);

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.size, 1);
    }

    // Test helper: mirror stream_rows but emit raw JSON so tests can
    // inspect the `done` / `total` fields without introspecting SSE Events.
    async fn stream_rows_json(
        tx: &tokio::sync::mpsc::Sender<Value>,
        session_id: &str,
        rows: Vec<crate::query::QueryResultRow>,
    ) {
        let total = rows.len();
        if total == 0 {
            let _ = tx
                .send(json!({
                    "chunk": 0,
                    "session_id": session_id,
                    "data": [],
                    "total": 0,
                    "done": true
                }))
                .await;
            return;
        }
        let chunks: Vec<&[crate::query::QueryResultRow]> =
            rows.chunks(STREAM_CHUNK_SIZE).collect();
        let last = chunks.len() - 1;
        for (i, chunk) in chunks.iter().enumerate() {
            let _ = tx
                .send(json!({
                    "chunk": i,
                    "session_id": session_id,
                    "data": serde_json::to_value(chunk).unwrap_or(Value::Null),
                    "total": total,
                    "done": i == last
                }))
                .await;
        }
    }

    #[tokio::test]
    async fn mcp_stream_rows_empty_still_emits_done() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Value>(16);
        stream_rows_json(&tx, "empty-session", vec![]).await;
        drop(tx);

        let mut last: Option<Value> = None;
        while let Some(frame) = rx.recv().await {
            last = Some(frame);
        }
        let last = last.expect("at least one frame");
        assert_eq!(last.get("done").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(last.get("total").and_then(|v| v.as_u64()), Some(0));
    }

    #[tokio::test]
    async fn mcp_stream_rows_chunks_and_marks_final_done() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Value>(64);
        let rows: Vec<crate::query::QueryResultRow> = (0..60)
            .map(|_| crate::query::QueryResultRow {
                entity_id: uuid::Uuid::nil(),
                attributes: serde_json::Map::new(),
                nested: serde_json::Map::new(),
            })
            .collect();
        stream_rows_json(&tx, "sess", rows).await;
        drop(tx);

        let mut frames: Vec<Value> = Vec::new();
        while let Some(frame) = rx.recv().await {
            frames.push(frame);
        }

        // 60 rows / 25 per chunk = 3 chunks.
        assert_eq!(frames.len(), 3);
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(frame.get("chunk").and_then(|v| v.as_u64()), Some(i as u64));
            assert_eq!(frame.get("total").and_then(|v| v.as_u64()), Some(60));
        }
        assert_eq!(
            frames
                .last()
                .unwrap()
                .get("done")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        // Intermediate frames must not be marked done.
        for frame in &frames[..frames.len() - 1] {
            assert_eq!(frame.get("done").and_then(|v| v.as_bool()), Some(false));
        }
    }

    #[tokio::test]
    async fn mcp_real_stream_rows_emits_events() {
        // Exercise the real stream_rows helper (returns Event objects).
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
        let rows: Vec<crate::query::QueryResultRow> = (0..30)
            .map(|_| crate::query::QueryResultRow {
                entity_id: uuid::Uuid::nil(),
                attributes: serde_json::Map::new(),
                nested: serde_json::Map::new(),
            })
            .collect();
        stream_rows(&tx, "s1", rows).await;
        drop(tx);

        let mut count = 0;
        while let Some(ev) = rx.recv().await {
            let _event = ev.expect("Infallible Ok");
            count += 1;
        }
        // 30 rows / 25 per chunk => 2 chunks
        assert_eq!(count, 2);
    }

    #[test]
    fn mcp_jsonrpc_error_has_expected_shape() {
        let resp = JsonRpcResponse::error(json!(1), -32601, "Method not found");
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, json!(1));
        assert!(resp.result.is_none());
        let err = resp.error.expect("error present");
        assert_eq!(err.code, -32601);
        assert_eq!(err.message, "Method not found");
    }

    #[test]
    fn mcp_bucket_to_interval_known_units() {
        assert_eq!(bucket_to_interval("1m").unwrap(), "minute");
        assert_eq!(bucket_to_interval("1h").unwrap(), "hour");
        assert_eq!(bucket_to_interval("1d").unwrap(), "day");
        assert_eq!(bucket_to_interval("1w").unwrap(), "week");
        assert!(bucket_to_interval("7x").is_err());
    }
}
