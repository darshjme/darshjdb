//! GraphQL API layer for DarshJDB.
//!
//! Provides a full GraphQL endpoint matching Supabase's pg_graphql feature set:
//! query entities by type, get single entities, inspect schema, and perform
//! mutations (create, update, delete) — all with the same permission checks
//! as the REST API.
//!
//! # Endpoints
//!
//! ```text
//! POST /api/graphql    Execute GraphQL queries and mutations
//! GET  /api/graphql    GraphQL Playground (interactive explorer)
//! ```

use std::sync::Arc;

use async_graphql::{Context, EmptySubscription, Object, Schema, SimpleObject};
use axum::http::HeaderMap;
use serde_json::Value;
use uuid::Uuid;

use crate::auth::{
    AuthContext, Operation, PermissionEngine, SessionManager, evaluate_rule_public,
    get_rule_with_fallback,
};
use crate::query;
use crate::triple_store::{PgTripleStore, TripleInput, TripleStore};

// ---------------------------------------------------------------------------
// GraphQL context (injected into every resolver)
// ---------------------------------------------------------------------------

/// Shared data available to all GraphQL resolvers via `ctx.data()`.
pub struct GqlContext {
    pub triple_store: Arc<PgTripleStore>,
    pub pool: sqlx::PgPool,
    pub session_manager: Arc<SessionManager>,
    pub permissions: Arc<PermissionEngine>,
    pub auth: Option<AuthContext>,
    pub dev_mode: bool,
}

// ---------------------------------------------------------------------------
// Error helper
// ---------------------------------------------------------------------------

/// Convert internal errors into async-graphql field errors.
fn gql_err(msg: impl std::fmt::Display) -> async_graphql::Error {
    async_graphql::Error::new(msg.to_string())
}

/// Check permission for a GraphQL operation, mirroring the REST layer.
fn check_permission(
    auth: &AuthContext,
    entity_type: &str,
    operation: Operation,
    engine: &PermissionEngine,
) -> async_graphql::Result<crate::auth::PermissionResult> {
    let rule = get_rule_with_fallback(engine, entity_type, operation).ok_or_else(|| {
        gql_err(format!(
            "No permission rule configured for {entity_type}.{operation:?}"
        ))
    })?;

    let result = evaluate_rule_public(auth, rule);

    if !result.allowed {
        let reason = result
            .denial_reason
            .as_deref()
            .unwrap_or("permission denied");
        return Err(gql_err(format!(
            "Access denied for {entity_type}.{operation:?}: {reason}"
        )));
    }

    Ok(result)
}

/// Extract the authenticated user from the GraphQL context.
/// Returns an error if no valid auth context is present.
fn require_auth(ctx: &Context<'_>) -> async_graphql::Result<AuthContext> {
    let gql_ctx = ctx.data::<GqlContext>()?;
    gql_ctx
        .auth
        .clone()
        .ok_or_else(|| gql_err("Authentication required"))
}

// ---------------------------------------------------------------------------
// Helper: build JSON object from entity triples
// ---------------------------------------------------------------------------

fn triples_to_json(entity_type: &str, entity_id: Uuid, triples: &[crate::triple_store::Triple]) -> Value {
    let mut attrs = serde_json::Map::new();
    attrs.insert("id".to_string(), Value::String(entity_id.to_string()));

    for t in triples {
        if t.attribute == ":db/type" {
            continue;
        }
        let key = t
            .attribute
            .strip_prefix(&format!("{entity_type}/"))
            .unwrap_or(&t.attribute)
            .to_string();
        attrs.entry(key).or_insert_with(|| t.value.clone());
    }

    Value::Object(attrs)
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
// Query root
// ---------------------------------------------------------------------------

/// Root query type for the DarshJDB GraphQL API.
pub struct QueryRoot;

#[Object]
impl QueryRoot {
    /// Query entities by type with optional filters and pagination.
    ///
    /// Returns a list of JSON objects matching the given entity type.
    /// Applies the same row-level security rules as the REST `GET /api/data/:entity`.
    async fn entities(
        &self,
        ctx: &Context<'_>,
        entity_type: String,
        filter: Option<Value>,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> async_graphql::Result<Vec<Value>> {
        let auth = require_auth(ctx)?;
        let gql_ctx = ctx.data::<GqlContext>()?;

        let _perm = check_permission(&auth, &entity_type, Operation::Read, &gql_ctx.permissions)?;

        let effective_limit = limit.unwrap_or(50).min(1000) as u32;

        // Build a DarshJQL query.
        let mut query_json = serde_json::json!({
            "type": entity_type,
            "$limit": effective_limit,
        });

        if let Some(off) = offset {
            query_json["$offset"] = Value::Number(serde_json::Number::from(off));
        }

        // Merge filter keys into the query.
        if let Some(Value::Object(f)) = filter {
            if let Value::Object(ref mut q) = query_json {
                for (k, v) in f {
                    q.insert(k, v);
                }
            }
        }

        let ast = query::parse_darshan_ql(&query_json)
            .map_err(|e| gql_err(format!("Query parse error: {e}")))?;
        let plan = query::plan_query(&ast)
            .map_err(|e| gql_err(format!("Query plan error: {e}")))?;
        let results = query::execute_query(&gql_ctx.pool, &plan)
            .await
            .map_err(|e| gql_err(format!("Query execution error: {e}")))?;

        // Convert query result rows into JSON values.
        let entities: Vec<Value> = results
            .into_iter()
            .map(|row| {
                let mut obj = serde_json::Map::new();
                obj.insert("id".to_string(), Value::String(row.entity_id.to_string()));
                if let Value::Object(attrs) = row.attributes {
                    for (k, v) in attrs {
                        obj.insert(k, v);
                    }
                }
                Value::Object(obj)
            })
            .collect();

        Ok(entities)
    }

    /// Get a single entity by its UUID.
    ///
    /// Returns the entity as a JSON object with all its attributes,
    /// or null if not found.
    async fn entity(
        &self,
        ctx: &Context<'_>,
        id: String,
        entity_type: Option<String>,
    ) -> async_graphql::Result<Option<Value>> {
        let auth = require_auth(ctx)?;
        let gql_ctx = ctx.data::<GqlContext>()?;

        let entity_id =
            Uuid::parse_str(&id).map_err(|_| gql_err(format!("Invalid UUID: {id}")))?;

        let triples = gql_ctx
            .triple_store
            .get_entity(entity_id)
            .await
            .map_err(|e| gql_err(format!("Failed to fetch entity: {e}")))?;

        if triples.is_empty() {
            return Ok(None);
        }

        // Determine entity type from :db/type triple.
        let etype = triples
            .iter()
            .find(|t| t.attribute == ":db/type")
            .and_then(|t| t.value.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        // If caller specified entity_type, verify it matches.
        if let Some(ref expected) = entity_type {
            if &etype != expected {
                return Ok(None);
            }
        }

        // Permission check with discovered entity type.
        if !etype.is_empty() {
            let perm = check_permission(&auth, &etype, Operation::Read, &gql_ctx.permissions)?;

            // Row-level security: check ownership if WHERE clauses exist.
            if !perm.where_clauses.is_empty() {
                let owner_id = triples
                    .iter()
                    .find(|t| t.attribute.ends_with("/owner_id"))
                    .and_then(|t| t.value.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());

                let entity_owner = if etype == "users" {
                    Some(entity_id)
                } else {
                    owner_id
                };

                if let Some(owner) = entity_owner {
                    if owner != auth.user_id {
                        return Err(gql_err(format!(
                            "Access denied: you do not own this {etype}"
                        )));
                    }
                }
            }
        }

        Ok(Some(triples_to_json(&etype, entity_id, &triples)))
    }

    /// Introspect the database schema.
    ///
    /// Returns the inferred schema containing all entity types and their
    /// attributes. Equivalent to `GET /api/admin/schema`.
    async fn schema(&self, ctx: &Context<'_>) -> async_graphql::Result<Value> {
        let _auth = require_auth(ctx)?;
        let gql_ctx = ctx.data::<GqlContext>()?;

        let schema = gql_ctx
            .triple_store
            .get_schema()
            .await
            .map_err(|e| gql_err(format!("Failed to infer schema: {e}")))?;

        Ok(serde_json::to_value(schema).unwrap_or(Value::Null))
    }

    /// Health check for the GraphQL endpoint.
    async fn health(&self) -> &str {
        "ok"
    }
}

// ---------------------------------------------------------------------------
// Mutation root
// ---------------------------------------------------------------------------

/// Root mutation type for the DarshJDB GraphQL API.
pub struct MutationRoot;

/// Result of a create or update mutation.
#[derive(SimpleObject)]
struct MutationResult {
    /// Whether the operation succeeded.
    success: bool,
    /// The entity ID (UUID string).
    id: String,
    /// The transaction ID assigned by the triple store.
    tx_id: i64,
}

#[Object]
impl MutationRoot {
    /// Create a new entity of the given type.
    ///
    /// The `data` argument is a JSON object of attribute key-value pairs.
    /// Returns the created entity's ID and transaction ID.
    async fn create_entity(
        &self,
        ctx: &Context<'_>,
        entity_type: String,
        data: Value,
    ) -> async_graphql::Result<Value> {
        let auth = require_auth(ctx)?;
        let gql_ctx = ctx.data::<GqlContext>()?;

        let _perm =
            check_permission(&auth, &entity_type, Operation::Create, &gql_ctx.permissions)?;

        let obj = data
            .as_object()
            .ok_or_else(|| gql_err("data must be a JSON object"))?;

        let id = Uuid::new_v4();

        // Extract optional TTL.
        let ttl_seconds: Option<i64> = obj.get("$ttl").and_then(|v| v.as_i64());

        // Build triples: :db/type + one per data field.
        let mut triples = vec![TripleInput {
            entity_id: id,
            attribute: ":db/type".to_string(),
            value: Value::String(entity_type.clone()),
            value_type: 0,
            ttl_seconds,
        }];

        for (key, value) in obj {
            if key.starts_with('$') {
                continue;
            }
            triples.push(TripleInput {
                entity_id: id,
                attribute: format!("{entity_type}/{key}"),
                value: value.clone(),
                value_type: infer_value_type(value),
                ttl_seconds,
            });
        }

        let tx_id = gql_ctx
            .triple_store
            .set_triples(&triples)
            .await
            .map_err(|e| gql_err(format!("Failed to create entity: {e}")))?;

        // Return the created entity.
        let mut result = serde_json::Map::new();
        result.insert("id".to_string(), Value::String(id.to_string()));
        result.insert("tx_id".to_string(), Value::Number(serde_json::Number::from(tx_id)));
        for (key, value) in obj {
            if !key.starts_with('$') {
                result.insert(key.clone(), value.clone());
            }
        }

        Ok(Value::Object(result))
    }

    /// Update an existing entity by ID.
    ///
    /// Merges the provided `data` fields into the entity. Existing attributes
    /// not in `data` are left unchanged (retract + re-assert pattern).
    async fn update_entity(
        &self,
        ctx: &Context<'_>,
        id: String,
        entity_type: String,
        data: Value,
    ) -> async_graphql::Result<Value> {
        let auth = require_auth(ctx)?;
        let gql_ctx = ctx.data::<GqlContext>()?;

        let entity_id =
            Uuid::parse_str(&id).map_err(|_| gql_err(format!("Invalid UUID: {id}")))?;

        let perm =
            check_permission(&auth, &entity_type, Operation::Update, &gql_ctx.permissions)?;

        // Verify entity exists.
        let existing = gql_ctx
            .triple_store
            .get_entity(entity_id)
            .await
            .map_err(|e| gql_err(format!("Failed to fetch entity: {e}")))?;

        if existing.is_empty() {
            return Err(gql_err(format!(
                "{entity_type} with id {id} not found"
            )));
        }

        // Row-level security: verify ownership.
        if !perm.where_clauses.is_empty() {
            let owner_id = existing
                .iter()
                .find(|t| t.attribute.ends_with("/owner_id"))
                .and_then(|t| t.value.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());

            let entity_owner = if entity_type == "users" {
                Some(entity_id)
            } else {
                owner_id
            };

            if let Some(owner) = entity_owner {
                if owner != auth.user_id {
                    return Err(gql_err(format!(
                        "Access denied: you do not own this {entity_type}"
                    )));
                }
            }
        }

        let obj = data
            .as_object()
            .ok_or_else(|| gql_err("data must be a JSON object"))?;

        // Retract old values then set new ones for each updated field.
        for (key, value) in obj {
            if key.starts_with('$') {
                continue;
            }
            let attr = format!("{entity_type}/{key}");

            // Retract existing attribute value.
            let _ = gql_ctx.triple_store.retract(entity_id, &attr).await;

            // Assert new value.
            let triple = TripleInput {
                entity_id,
                attribute: attr,
                value: value.clone(),
                value_type: infer_value_type(value),
                ttl_seconds: None,
            };
            gql_ctx
                .triple_store
                .set_triples(&[triple])
                .await
                .map_err(|e| gql_err(format!("Failed to update attribute {key}: {e}")))?;
        }

        // Fetch updated entity and return.
        let updated = gql_ctx
            .triple_store
            .get_entity(entity_id)
            .await
            .map_err(|e| gql_err(format!("Failed to fetch updated entity: {e}")))?;

        Ok(triples_to_json(&entity_type, entity_id, &updated))
    }

    /// Delete an entity by ID.
    ///
    /// Retracts all triples for the entity. Returns `true` on success.
    async fn delete_entity(
        &self,
        ctx: &Context<'_>,
        id: String,
        entity_type: String,
    ) -> async_graphql::Result<bool> {
        let auth = require_auth(ctx)?;
        let gql_ctx = ctx.data::<GqlContext>()?;

        let entity_id =
            Uuid::parse_str(&id).map_err(|_| gql_err(format!("Invalid UUID: {id}")))?;

        let perm =
            check_permission(&auth, &entity_type, Operation::Delete, &gql_ctx.permissions)?;

        let existing = gql_ctx
            .triple_store
            .get_entity(entity_id)
            .await
            .map_err(|e| gql_err(format!("Failed to fetch entity: {e}")))?;

        if existing.is_empty() {
            return Err(gql_err(format!(
                "{entity_type} with id {id} not found"
            )));
        }

        // Row-level security: verify ownership.
        if !perm.where_clauses.is_empty() {
            let owner_id = existing
                .iter()
                .find(|t| t.attribute.ends_with("/owner_id"))
                .and_then(|t| t.value.as_str())
                .and_then(|s| Uuid::parse_str(s).ok());

            let entity_owner = if entity_type == "users" {
                Some(entity_id)
            } else {
                owner_id
            };

            if let Some(owner) = entity_owner {
                if owner != auth.user_id {
                    return Err(gql_err(format!(
                        "Access denied: you do not own this {entity_type}"
                    )));
                }
            }
        }

        // Retract all triples for this entity.
        for triple in &existing {
            gql_ctx
                .triple_store
                .retract(entity_id, &triple.attribute)
                .await
                .map_err(|e| gql_err(format!("Failed to retract {}: {e}", triple.attribute)))?;
        }

        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// Schema type alias
// ---------------------------------------------------------------------------

/// The fully-built DarshJDB GraphQL schema.
pub type DarshJDBSchema = Schema<QueryRoot, MutationRoot, EmptySubscription>;

/// Build the async-graphql schema with default configuration.
pub fn build_schema() -> DarshJDBSchema {
    Schema::build(QueryRoot, MutationRoot, EmptySubscription)
        .enable_federation()
        .finish()
}

// ---------------------------------------------------------------------------
// Axum route handlers
// ---------------------------------------------------------------------------

/// POST /api/graphql — Execute a GraphQL query or mutation.
pub async fn graphql_handler(
    headers: HeaderMap,
    axum::extract::State(state): axum::extract::State<super::rest::AppState>,
    request: async_graphql_axum::GraphQLRequest,
) -> async_graphql_axum::GraphQLResponse {
    let schema = build_schema();

    // Extract auth context from Bearer token (optional — some queries may allow it).
    let auth = extract_auth_from_headers(&headers, &state);

    let gql_ctx = GqlContext {
        triple_store: state.triple_store.clone(),
        pool: state.pool.clone(),
        session_manager: state.session_manager.clone(),
        permissions: state.permissions.clone(),
        auth,
        dev_mode: state.dev_mode,
    };

    let request = request.into_inner().data(gql_ctx);
    schema.execute(request).await.into()
}

/// GET /api/graphql — Serve the GraphQL Playground UI.
pub async fn graphql_playground() -> axum::response::Html<String> {
    axum::response::Html(
        async_graphql::http::playground_source(
            async_graphql::http::GraphQLPlaygroundConfig::new("/api/graphql")
                .title("DarshJDB GraphQL Playground"),
        ),
    )
}

/// Extract an optional `AuthContext` from request headers.
/// Returns `None` instead of erroring if no token is present —
/// resolvers that require auth will fail at the resolver level.
fn extract_auth_from_headers(
    headers: &HeaderMap,
    state: &super::rest::AppState,
) -> Option<AuthContext> {
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t.trim())?;

    if token.is_empty() {
        return None;
    }

    // Dev mode shortcut.
    if state.dev_mode && token == "dev" {
        return Some(AuthContext {
            user_id: Uuid::nil(),
            session_id: Uuid::nil(),
            roles: vec!["admin".into(), "user".into()],
            ip: "127.0.0.1".into(),
            user_agent: "dev-mode".into(),
            device_fingerprint: "dev".into(),
        });
    }

    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let ua = headers
        .get(http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");
    let dfp = headers
        .get("x-device-fingerprint")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    state.session_manager.validate_token(token, ip, ua, dfp).ok()
}
