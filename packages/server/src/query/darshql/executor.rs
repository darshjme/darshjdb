//! Execute DarshQL statements against the configured `Store` backend.
//!
//! Translates the DarshQL AST into operations against DarshJDB's triple
//! store. Graph traversals follow edge relationships stored as triples
//! with a `:db/edge` attribute pattern. LIVE SELECT returns subscription
//! IDs through the existing reactive dependency tracker.
//!
//! # v0.3.2.1 — ExecutorContext + dialect gating
//!
//! Every executor entry point now takes an [`ExecutorContext`] holding
//! the Postgres pool (for the legacy raw-SQL paths that have not been
//! ported yet), an `Arc<dyn Store>` (for portable triple operations),
//! and an `Arc<dyn SqlDialect>` (for capability gating). Statement
//! types whose Pg implementation depends on Postgres-only features —
//! DEFINE TABLE / DEFINE FIELD (DDL stored as triples with Pg-flavoured
//! UPDATEs), graph traversal (recursive Pg subqueries on `:edge/in` /
//! `:edge/out`) — check the dialect capability up front and refuse with
//! `InvalidQuery` on dialects that don't support them. The portable
//! rewrite of those paths is tracked for v0.3.3.
//!
//! For the v0.3.2.1 sprint the Pg call sites still go through
//! `ctx.pool` directly. The portable hookup for SELECT / CREATE /
//! INSERT / RETRACT through `ctx.store` lands as the planner gains the
//! needed shape (v0.3.3); today the gates are the safety net.

use std::sync::Arc;

use serde_json::{Map, Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};
use crate::query::dialect::{PgDialect, SqlDialect};
use crate::store::Store;

use super::ast::*;

/// Context handed to every executor function.
///
/// Holds the three handles the executor needs to run a statement:
/// - `pool`: the Postgres pool used by the legacy raw-SQL paths that
///   have not been ported through the dialect/Store boundary yet.
/// - `store`: the object-safe triple store. Portable paths (the v0.3.3
///   target) call this instead of touching the pool directly.
/// - `dialect`: the SQL dialect, used to gate Pg-only statement types
///   at dispatch time.
///
/// Construction is cheap (`Arc` clones); the executor takes a `&` so
/// the caller still owns the handles for the rest of the request.
#[derive(Clone)]
pub struct ExecutorContext {
    /// Postgres pool for legacy raw-SQL paths.
    pub pool: PgPool,
    /// Object-safe triple store handle for portable triple operations.
    pub store: Arc<dyn Store>,
    /// SQL dialect, used for capability gating in `execute_one`.
    pub dialect: Arc<dyn SqlDialect>,
}

impl ExecutorContext {
    /// Backwards-compatible constructor used by the HTTP entry point.
    ///
    /// Wraps the pool in a `PgStore` adapter so the `Arc<dyn Store>`
    /// surface is populated even though the request path is still
    /// Pg-only. Callers that already hold a `PgTripleStore` should use
    /// [`ExecutorContext::new`] to avoid the extra triple-store
    /// construction.
    pub fn from_pool(pool: PgPool) -> Self {
        // The PgStore adapter wants a PgTripleStore. The HTTP request
        // path has already migrated the schema at boot, so we use the
        // lighter-weight `new_lazy` constructor which skips re-running
        // ensure_schema on every executor call.
        let triple_store = crate::triple_store::PgTripleStore::new_lazy(pool.clone());
        Self {
            pool,
            store: Arc::new(crate::store::pg::PgStore::new(triple_store)),
            dialect: Arc::new(PgDialect),
        }
    }

    /// Direct constructor for callers that already have the three
    /// handles materialised (tests, future portable call sites).
    pub fn new(
        pool: PgPool,
        store: Arc<dyn Store>,
        dialect: Arc<dyn SqlDialect>,
    ) -> Self {
        Self {
            pool,
            store,
            dialect,
        }
    }
}

/// Result of executing a DarshQL statement.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status")]
pub enum ExecResult {
    /// Rows returned from a SELECT.
    #[serde(rename = "OK")]
    Rows { result: Vec<Value>, time: String },
    /// A single record was created / updated.
    #[serde(rename = "OK")]
    Record { result: Value, time: String },
    /// Records deleted.
    #[serde(rename = "OK")]
    Deleted { count: u64, time: String },
    /// An edge was created via RELATE.
    #[serde(rename = "OK")]
    Related { result: Value, time: String },
    /// A live query subscription was started.
    #[serde(rename = "OK")]
    LiveQuery {
        subscription_id: String,
        time: String,
    },
    /// Schema definition applied.
    #[serde(rename = "OK")]
    Defined { info: String, time: String },
    /// Schema info returned.
    #[serde(rename = "OK")]
    Info { result: Value, time: String },
    /// Rows inserted.
    #[serde(rename = "OK")]
    Inserted { count: u64, time: String },
}

/// Execute a list of DarshQL statements against the supplied Postgres
/// pool.
///
/// Backwards-compatible entry point kept for the HTTP request handler
/// in `api/rest.rs`. Internally constructs an [`ExecutorContext`] from
/// the pool and forwards to [`execute_with_context`]. New call sites
/// that already hold a `Store + SqlDialect` pair should use
/// [`execute_with_context`] directly.
pub async fn execute(pool: &PgPool, statements: Vec<Statement>) -> Result<Vec<ExecResult>> {
    let ctx = ExecutorContext::from_pool(pool.clone());
    execute_with_context(&ctx, statements).await
}

/// Execute a list of DarshQL statements against the supplied executor
/// context.
///
/// This is the v0.3.2.1 hookup point — every function in the executor
/// takes `&ExecutorContext` so capability gates and (eventually) the
/// portable Store hookup share the same plumbing.
pub async fn execute_with_context(
    ctx: &ExecutorContext,
    statements: Vec<Statement>,
) -> Result<Vec<ExecResult>> {
    let mut results = Vec::with_capacity(statements.len());
    for stmt in statements {
        let start = std::time::Instant::now();
        let result = execute_one(ctx, &stmt, start).await?;
        results.push(result);
    }
    Ok(results)
}

/// Refuse a Tier 2 statement type on a dialect that does not support it.
///
/// Centralises the error message so every gate site reads identically
/// and the v0.3.3 unblock note is in one place.
fn refuse_unsupported(dialect_name: &'static str, feature: &str) -> DarshJError {
    DarshJError::InvalidQuery(format!(
        "{feature} is not supported on the {dialect_name} dialect yet \
         (tracked for v0.3.3); use the postgres backend for this statement"
    ))
}

async fn execute_one(
    ctx: &ExecutorContext,
    stmt: &Statement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    match stmt {
        Statement::Select(s) => exec_select(ctx, s, start).await,
        Statement::Create(c) => exec_create(ctx, c, start).await,
        Statement::Update(u) => exec_update(ctx, u, start).await,
        Statement::Delete(d) => exec_delete(ctx, d, start).await,
        Statement::Insert(i) => exec_insert(ctx, i, start).await,
        Statement::Relate(r) => {
            // RELATE creates a graph edge — refuse on dialects that
            // don't support graph traversal because the read path
            // (->edge) won't be runnable anyway.
            if !ctx.dialect.supports_graph_traversal() {
                return Err(refuse_unsupported(ctx.dialect.name(), "RELATE / graph edges"));
            }
            exec_relate(ctx, r, start).await
        }
        Statement::LiveSelect(ls) => exec_live_select(ls, start).await,
        Statement::DefineTable(dt) => {
            if !ctx.dialect.supports_ddl() {
                return Err(refuse_unsupported(ctx.dialect.name(), "DEFINE TABLE"));
            }
            exec_define_table(ctx, dt, start).await
        }
        Statement::DefineField(df) => {
            if !ctx.dialect.supports_ddl() {
                return Err(refuse_unsupported(ctx.dialect.name(), "DEFINE FIELD"));
            }
            exec_define_field(ctx, df, start).await
        }
        Statement::InfoFor(info) => exec_info(ctx, info, start).await,
    }
}

fn elapsed(start: std::time::Instant) -> String {
    let d = start.elapsed();
    if d.as_millis() > 0 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{}us", d.as_micros())
    }
}

// ── SELECT ─────────────────────────────────────────────────────────

async fn exec_select(
    ctx: &ExecutorContext,
    stmt: &SelectStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    let table = stmt.from.table_name();
    let mut sql = String::with_capacity(512);
    let mut params: Vec<Value> = Vec::new();
    let mut param_idx = 1u32;

    // Base query: get all triples for entities of this type.
    sql.push_str(
        "SELECT t0.entity_id, t0.attribute, t0.value, t0.value_type, t0.tx_id, t0.created_at\n",
    );
    sql.push_str("FROM triples t0\n");
    sql.push_str("INNER JOIN triples t_type ON t_type.entity_id = t0.entity_id\n");
    sql.push_str("  AND t_type.attribute = ':db/type'\n");
    sql.push_str("  AND NOT t_type.retracted\n");
    sql.push_str(&format!(
        "  AND t_type.value = to_jsonb(${}::text)\n",
        param_idx
    ));
    params.push(Value::String(table.to_string()));
    param_idx += 1;

    // If targeting a specific record, filter by entity_id.
    if let Target::Record(ref rec) = stmt.from {
        // Try to parse as UUID; if not, use as-is for deterministic UUID generation.
        let entity_id = record_id_to_uuid(rec);
        sql.push_str(&format!(
            "INNER JOIN triples t_rid ON t_rid.entity_id = t0.entity_id\n  AND t_rid.entity_id = ${param_idx}::uuid\n"
        ));
        params.push(Value::String(entity_id.to_string()));
        param_idx += 1;
    }

    // WHERE clause translation.
    if let Some(ref cond) = stmt.condition {
        let where_sql = translate_where(cond, &mut params, &mut param_idx);
        sql.push_str(&where_sql);
    }

    sql.push_str("WHERE NOT t0.retracted\n");

    // ORDER BY.
    if !stmt.order.is_empty() {
        sql.push_str("ORDER BY ");
        for (i, o) in stmt.order.iter().enumerate() {
            if i > 0 {
                sql.push_str(", ");
            }
            let alias = format!("to{i}");
            sql.push_str(&format!(
                "(SELECT {alias}.value FROM triples {alias} WHERE {alias}.entity_id = t0.entity_id AND {alias}.attribute = ${param_idx} AND NOT {alias}.retracted ORDER BY {alias}.tx_id DESC LIMIT 1)",
            ));
            params.push(Value::String(o.field.clone()));
            param_idx += 1;

            match o.direction {
                SortDir::Asc => sql.push_str(" ASC"),
                SortDir::Desc => sql.push_str(" DESC"),
            }
        }
        sql.push('\n');
    }

    // Execute the query.
    let mut query =
        sqlx::query_as::<_, (Uuid, String, Value, i16, i64, chrono::DateTime<chrono::Utc>)>(&sql);

    for p in &params {
        query = bind_param(query, p);
    }

    let rows = query.fetch_all(pool).await?;

    // Group by entity_id.
    let mut entities: std::collections::HashMap<Uuid, Map<String, Value>> =
        std::collections::HashMap::new();

    for (entity_id, attribute, value, _vt, _tx, _ts) in &rows {
        let entry = entities.entry(*entity_id).or_default();
        entry
            .entry(attribute.clone())
            .or_insert_with(|| value.clone());
    }

    // Apply pagination.
    let mut entity_keys: Vec<Uuid> = entities.keys().copied().collect();
    entity_keys.sort();

    if let Some(offset) = stmt.start {
        let off = offset as usize;
        if off < entity_keys.len() {
            entity_keys = entity_keys.split_off(off);
        } else {
            entity_keys.clear();
        }
    }
    if let Some(limit) = stmt.limit {
        entity_keys.truncate(limit as usize);
    }

    // Build result objects.
    let mut result: Vec<Value> = Vec::with_capacity(entity_keys.len());
    for eid in &entity_keys {
        let attrs = &entities[eid];
        let mut obj = Map::new();
        obj.insert("id".to_string(), json!(format!("{}:{}", table, eid)));

        // Project fields.
        let project_all = stmt.fields.iter().any(|f| matches!(f, Field::All));
        if project_all {
            for (k, v) in attrs {
                if k != ":db/type" {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }

        for field in &stmt.fields {
            match field {
                Field::All => {} // already handled above
                Field::Attribute(name) => {
                    if let Some(v) = attrs.get(name) {
                        obj.insert(name.clone(), v.clone());
                    }
                }
                Field::Cast { cast_type, expr } => {
                    if let Field::Attribute(name) = expr.as_ref()
                        && let Some(v) = attrs.get(name)
                    {
                        obj.insert(name.clone(), cast_value(v, cast_type));
                    }
                }
                Field::Graph(trav) => {
                    // Graph traversal: follow edges. Pg-only today;
                    // refuse on dialects that don't support it so the
                    // SELECT fails cleanly with a useful message.
                    if !ctx.dialect.supports_graph_traversal() {
                        return Err(refuse_unsupported(
                            ctx.dialect.name(),
                            "SELECT with graph traversal",
                        ));
                    }
                    let traversed = exec_graph_traversal(ctx, *eid, trav).await?;
                    let key = format_graph_key(trav);
                    obj.insert(key, json!(traversed));
                }
                Field::Computed { func, args, alias } => {
                    let val = exec_computed(ctx, *eid, func, args).await?;
                    obj.insert(alias.clone(), val);
                }
            }
        }

        result.push(Value::Object(obj));
    }

    Ok(ExecResult::Rows {
        result,
        time: elapsed(start),
    })
}

// ── CREATE ─────────────────────────────────────────────────────────

async fn exec_create(
    ctx: &ExecutorContext,
    stmt: &CreateStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    let table = stmt.target.table_name();
    let entity_id = match &stmt.target {
        Target::Record(rec) => record_id_to_uuid(rec),
        Target::Table(_) => Uuid::new_v4(),
    };

    // Insert :db/type triple.
    insert_triple(pool, entity_id, ":db/type", &json!(table)).await?;

    // Insert data triples.
    let pairs = data_to_pairs(&stmt.data)?;
    for (key, val) in &pairs {
        let json_val = expr_to_json(val)?;
        insert_triple(pool, entity_id, key, &json_val).await?;
    }

    let mut result = Map::new();
    result.insert("id".to_string(), json!(format!("{}:{}", table, entity_id)));
    for (k, v) in &pairs {
        result.insert(k.clone(), expr_to_json(v)?);
    }

    Ok(ExecResult::Record {
        result: Value::Object(result),
        time: elapsed(start),
    })
}

// ── UPDATE ─────────────────────────────────────────────────────────

async fn exec_update(
    ctx: &ExecutorContext,
    stmt: &UpdateStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    let table = stmt.target.table_name();

    // Find matching entity ids.
    let entity_ids = find_entities(pool, table, stmt.condition.as_ref(), &stmt.target).await?;

    let pairs = data_to_pairs(&stmt.data)?;

    for eid in &entity_ids {
        for (key, val) in &pairs {
            let json_val = expr_to_json(val)?;
            // Retract old value, insert new.
            retract_attribute(pool, *eid, key).await?;
            insert_triple(pool, *eid, key, &json_val).await?;
        }
    }

    let mut result = Map::new();
    result.insert("updated".to_string(), json!(entity_ids.len()));
    result.insert("table".to_string(), json!(table));

    Ok(ExecResult::Record {
        result: Value::Object(result),
        time: elapsed(start),
    })
}

// ── DELETE ─────────────────────────────────────────────────────────

async fn exec_delete(
    ctx: &ExecutorContext,
    stmt: &DeleteStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    let table = stmt.target.table_name();
    let entity_ids = find_entities(pool, table, stmt.condition.as_ref(), &stmt.target).await?;

    for eid in &entity_ids {
        sqlx::query("UPDATE triples SET retracted = true WHERE entity_id = $1")
            .bind(eid)
            .execute(pool)
            .await?;
    }

    Ok(ExecResult::Deleted {
        count: entity_ids.len() as u64,
        time: elapsed(start),
    })
}

// ── INSERT ─────────────────────────────────────────────────────────

async fn exec_insert(
    ctx: &ExecutorContext,
    stmt: &InsertStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    let mut count = 0u64;

    for row_values in &stmt.values {
        let entity_id = Uuid::new_v4();
        insert_triple(pool, entity_id, ":db/type", &json!(stmt.table)).await?;

        for (field, value) in stmt.fields.iter().zip(row_values.iter()) {
            let json_val = expr_to_json(value)?;
            insert_triple(pool, entity_id, field, &json_val).await?;
        }
        count += 1;
    }

    Ok(ExecResult::Inserted {
        count,
        time: elapsed(start),
    })
}

// ── RELATE ─────────────────────────────────────────────────────────

async fn exec_relate(
    ctx: &ExecutorContext,
    stmt: &RelateStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    let edge_id = Uuid::new_v4();
    let from_id = record_id_to_uuid(&stmt.from);
    let to_id = record_id_to_uuid(&stmt.to);

    // Create edge entity.
    insert_triple(pool, edge_id, ":db/type", &json!(stmt.edge)).await?;
    insert_triple(pool, edge_id, ":edge/in", &json!(from_id.to_string())).await?;
    insert_triple(pool, edge_id, ":edge/out", &json!(to_id.to_string())).await?;

    // Insert edge data.
    if let Some(ref data) = stmt.data {
        let pairs = data_to_pairs(data)?;
        for (key, val) in &pairs {
            let json_val = expr_to_json(val)?;
            insert_triple(pool, edge_id, key, &json_val).await?;
        }
    }

    let mut result = Map::new();
    result.insert(
        "id".to_string(),
        json!(format!("{}:{}", stmt.edge, edge_id)),
    );
    result.insert("in".to_string(), json!(stmt.from.to_string()));
    result.insert("out".to_string(), json!(stmt.to.to_string()));

    Ok(ExecResult::Related {
        result: Value::Object(result),
        time: elapsed(start),
    })
}

// ── LIVE SELECT ────────────────────────────────────────────────────

async fn exec_live_select(
    stmt: &LiveSelectStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    // Generate a subscription ID. The actual subscription is managed
    // by the reactive dependency tracker in the SSE layer.
    let subscription_id = Uuid::new_v4().to_string();

    tracing::info!(
        subscription_id = %subscription_id,
        table = %stmt.from.table_name(),
        "LIVE SELECT registered — subscribe via SSE /api/subscribe?live_id={}",
        subscription_id
    );

    Ok(ExecResult::LiveQuery {
        subscription_id,
        time: elapsed(start),
    })
}

// ── DEFINE TABLE ───────────────────────────────────────────────────

async fn exec_define_table(
    ctx: &ExecutorContext,
    stmt: &DefineTableStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    // Store table definition as a schema triple.
    let schema_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("table:{}", stmt.name).as_bytes(),
    );

    // Retract any existing definition.
    sqlx::query(
        "UPDATE triples SET retracted = true WHERE entity_id = $1 AND attribute LIKE ':schema/%'",
    )
    .bind(schema_id)
    .execute(pool)
    .await?;

    insert_triple(pool, schema_id, ":db/type", &json!("__schema_table")).await?;
    insert_triple(pool, schema_id, ":schema/name", &json!(stmt.name)).await?;
    insert_triple(
        pool,
        schema_id,
        ":schema/mode",
        &json!(format!("{:?}", stmt.schema_mode)),
    )
    .await?;
    insert_triple(pool, schema_id, ":schema/drop", &json!(stmt.drop)).await?;

    Ok(ExecResult::Defined {
        info: format!(
            "Table '{}' defined ({:?}{})",
            stmt.name,
            stmt.schema_mode,
            if stmt.drop { ", DROP" } else { "" }
        ),
        time: elapsed(start),
    })
}

// ── DEFINE FIELD ───────────────────────────────────────────────────

async fn exec_define_field(
    ctx: &ExecutorContext,
    stmt: &DefineFieldStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    let field_id = Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("field:{}:{}", stmt.table, stmt.name).as_bytes(),
    );

    sqlx::query("UPDATE triples SET retracted = true WHERE entity_id = $1")
        .bind(field_id)
        .execute(pool)
        .await?;

    insert_triple(pool, field_id, ":db/type", &json!("__schema_field")).await?;
    insert_triple(pool, field_id, ":schema/field_name", &json!(stmt.name)).await?;
    insert_triple(pool, field_id, ":schema/table", &json!(stmt.table)).await?;
    if let Some(ref ft) = stmt.field_type {
        insert_triple(
            pool,
            field_id,
            ":schema/field_type",
            &json!(format!("{ft:?}")),
        )
        .await?;
    }

    Ok(ExecResult::Defined {
        info: format!(
            "Field '{}' on table '{}' defined (type: {:?})",
            stmt.name, stmt.table, stmt.field_type
        ),
        time: elapsed(start),
    })
}

// ── INFO FOR ───────────────────────────────────────────────────────

async fn exec_info(
    ctx: &ExecutorContext,
    stmt: &InfoForStatement,
    start: std::time::Instant,
) -> Result<ExecResult> {
    let pool = &ctx.pool;
    match &stmt.target {
        InfoTarget::Db => {
            // List all defined tables.
            let rows = sqlx::query_as::<_, (Value,)>(
                "SELECT DISTINCT value FROM triples WHERE attribute = ':schema/name' AND NOT retracted",
            )
            .fetch_all(pool)
            .await?;

            let tables: Vec<Value> = rows.into_iter().map(|(v,)| v).collect();
            let mut result = Map::new();
            result.insert("tables".to_string(), json!(tables));

            Ok(ExecResult::Info {
                result: Value::Object(result),
                time: elapsed(start),
            })
        }
        InfoTarget::Table(name) => {
            // List all fields for this table.
            let rows = sqlx::query_as::<_, (Uuid, String, Value)>(
                "SELECT t0.entity_id, t0.attribute, t0.value FROM triples t0 \
                 INNER JOIN triples t1 ON t1.entity_id = t0.entity_id \
                   AND t1.attribute = ':schema/table' \
                   AND t1.value = to_jsonb($1::text) \
                   AND NOT t1.retracted \
                 WHERE NOT t0.retracted AND t0.attribute LIKE ':schema/%'",
            )
            .bind(name)
            .fetch_all(pool)
            .await?;

            let mut fields: Vec<Map<String, Value>> = Vec::new();
            let mut current_id: Option<Uuid> = None;
            let mut current: Map<String, Value> = Map::new();

            for (eid, attr, val) in rows {
                if current_id != Some(eid) {
                    if !current.is_empty() {
                        fields.push(std::mem::take(&mut current));
                    }
                    current_id = Some(eid);
                }
                let short_attr = attr.strip_prefix(":schema/").unwrap_or(&attr);
                current.insert(short_attr.to_string(), val);
            }
            if !current.is_empty() {
                fields.push(current);
            }

            let mut result = Map::new();
            result.insert("table".to_string(), json!(name));
            result.insert("fields".to_string(), json!(fields));

            Ok(ExecResult::Info {
                result: Value::Object(result),
                time: elapsed(start),
            })
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Insert a single triple into the store.
async fn insert_triple(
    pool: &PgPool,
    entity_id: Uuid,
    attribute: &str,
    value: &Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO triples (entity_id, attribute, value, value_type, tx_id, retracted) \
         VALUES ($1, $2, $3, 0, nextval('tx_id_seq'), false)",
    )
    .bind(entity_id)
    .bind(attribute)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Retract all triples for an entity + attribute (soft delete before update).
async fn retract_attribute(pool: &PgPool, entity_id: Uuid, attribute: &str) -> Result<()> {
    sqlx::query(
        "UPDATE triples SET retracted = true WHERE entity_id = $1 AND attribute = $2 AND NOT retracted",
    )
    .bind(entity_id)
    .bind(attribute)
    .execute(pool)
    .await?;
    Ok(())
}

/// Find entity UUIDs matching a table and optional WHERE condition.
async fn find_entities(
    pool: &PgPool,
    table: &str,
    condition: Option<&Expr>,
    target: &Target,
) -> Result<Vec<Uuid>> {
    let mut sql = String::with_capacity(256);
    let mut params: Vec<Value> = Vec::new();
    let mut param_idx = 1u32;

    sql.push_str("SELECT DISTINCT t_type.entity_id FROM triples t_type\n");
    sql.push_str("WHERE t_type.attribute = ':db/type'\n");
    sql.push_str("  AND NOT t_type.retracted\n");
    sql.push_str(&format!(
        "  AND t_type.value = to_jsonb(${}::text)\n",
        param_idx
    ));
    params.push(Value::String(table.to_string()));
    param_idx += 1;

    // Specific record.
    if let Target::Record(rec) = target {
        let uid = record_id_to_uuid(rec);
        sql.push_str(&format!("  AND t_type.entity_id = ${}::uuid\n", param_idx));
        params.push(Value::String(uid.to_string()));
        param_idx += 1;
    }

    // WHERE condition joins.
    if let Some(cond) = condition {
        let cond_sql = translate_where_for_find(cond, &mut params, &mut param_idx);
        sql.push_str(&cond_sql);
    }

    let mut query = sqlx::query_as::<_, (Uuid,)>(&sql);
    for p in &params {
        query = bind_param_single(query, p);
    }

    let rows = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Translate a WHERE expression into JOIN clauses on the triples table.
fn translate_where(expr: &Expr, params: &mut Vec<Value>, idx: &mut u32) -> String {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            if let (Expr::Ident(attr), right_val) = (left.as_ref(), right.as_ref()) {
                let alias = format!("tw{}", *idx);
                let op_str = binop_to_sql(op);

                let is_string = matches!(right_val, Expr::Value(Value::String(_)));
                let jsonb_param = if is_string {
                    format!("to_jsonb(${}::text)", *idx + 1)
                } else {
                    format!("${}::jsonb", *idx + 1)
                };

                let mut sql = String::new();
                sql.push_str(&format!(
                    "INNER JOIN triples {alias} ON {alias}.entity_id = t0.entity_id\n"
                ));
                sql.push_str(&format!("  AND {alias}.attribute = ${}\n", *idx));
                params.push(Value::String(attr.clone()));
                *idx += 1;

                sql.push_str(&format!("  AND NOT {alias}.retracted\n"));
                sql.push_str(&format!("  AND {alias}.value {op_str} {jsonb_param}\n"));
                params.push(expr_to_json(right_val).unwrap_or(Value::Null));
                *idx += 1;

                sql
            } else {
                String::new()
            }
        }
        Expr::LogicalOp { left, op: _, right } => {
            // For AND, both sides produce JOINs (implicit AND).
            let mut sql = translate_where(left, params, idx);
            sql.push_str(&translate_where(right, params, idx));
            sql
        }
        _ => String::new(),
    }
}

/// Similar to translate_where but for the find_entities subquery.
fn translate_where_for_find(expr: &Expr, params: &mut Vec<Value>, idx: &mut u32) -> String {
    match expr {
        Expr::BinaryOp { left, op, right } => {
            if let (Expr::Ident(attr), right_val) = (left.as_ref(), right.as_ref()) {
                let op_str = binop_to_sql(op);
                let is_string = matches!(right_val, Expr::Value(Value::String(_)));
                let jsonb_param = if is_string {
                    format!("to_jsonb(${}::text)", *idx + 1)
                } else {
                    format!("${}::jsonb", *idx + 1)
                };

                let mut sql = String::new();
                sql.push_str(&format!(
                    "  AND t_type.entity_id IN (\
                     SELECT tw.entity_id FROM triples tw \
                     WHERE tw.attribute = ${} AND NOT tw.retracted \
                     AND tw.value {} {})\n",
                    *idx, op_str, jsonb_param
                ));
                params.push(Value::String(attr.clone()));
                *idx += 1;
                params.push(expr_to_json(right_val).unwrap_or(Value::Null));
                *idx += 1;

                sql
            } else {
                String::new()
            }
        }
        Expr::LogicalOp { left, op: _, right } => {
            let mut sql = translate_where_for_find(left, params, idx);
            sql.push_str(&translate_where_for_find(right, params, idx));
            sql
        }
        _ => String::new(),
    }
}

fn binop_to_sql(op: &BinOp) -> &'static str {
    match op {
        BinOp::Eq => "=",
        BinOp::Neq => "!=",
        BinOp::Gt => ">",
        BinOp::Gte => ">=",
        BinOp::Lt => "<",
        BinOp::Lte => "<=",
        BinOp::Like => "ILIKE",
        BinOp::Contains => "@>",
        BinOp::Is => "IS",
        BinOp::IsNot => "IS NOT",
    }
}

/// Execute a graph traversal from a starting entity.
///
/// Pg-only today — callers MUST gate on
/// `ctx.dialect.supports_graph_traversal()` first. The function still
/// reads from `ctx.pool` directly because the recursive subquery shape
/// has not been ported to the dialect/Store boundary yet (v0.3.3).
async fn exec_graph_traversal(
    ctx: &ExecutorContext,
    start_id: Uuid,
    traversal: &GraphTraversal,
) -> Result<Vec<Value>> {
    let pool = &ctx.pool;
    let mut current_ids = vec![start_id];

    for step in &traversal.steps {
        let mut next_ids = Vec::new();
        for id in &current_ids {
            let (attr_in, attr_out) = match step.direction {
                EdgeDirection::Out => (":edge/in", ":edge/out"),
                EdgeDirection::In => (":edge/out", ":edge/in"),
            };

            // Find edges of this type where the in-node matches.
            let rows = sqlx::query_as::<_, (Value,)>(&format!(
                "SELECT t_out.value FROM triples t_edge \
                     INNER JOIN triples t_in ON t_in.entity_id = t_edge.entity_id \
                       AND t_in.attribute = '{}' AND NOT t_in.retracted \
                       AND t_in.value = to_jsonb($1::text) \
                     INNER JOIN triples t_out ON t_out.entity_id = t_edge.entity_id \
                       AND t_out.attribute = '{}' AND NOT t_out.retracted \
                     WHERE t_edge.attribute = ':db/type' \
                       AND t_edge.value = to_jsonb($2::text) \
                       AND NOT t_edge.retracted",
                attr_in, attr_out
            ))
            .bind(id.to_string())
            .bind(&step.edge)
            .fetch_all(pool)
            .await?;

            for (val,) in rows {
                if let Some(uid_str) = val.as_str()
                    && let Ok(uid) = uid_str.parse::<Uuid>()
                {
                    next_ids.push(uid);
                }
            }
        }
        current_ids = next_ids;
    }

    // Fetch the final entities.
    let mut results = Vec::new();
    for id in &current_ids {
        let rows = sqlx::query_as::<_, (String, Value)>(
            "SELECT attribute, value FROM triples WHERE entity_id = $1 AND NOT retracted",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;

        let mut obj = Map::new();
        obj.insert("id".to_string(), json!(id.to_string()));
        for (attr, val) in rows {
            if attr != ":db/type" {
                obj.insert(attr, val);
            }
        }
        results.push(Value::Object(obj));
    }

    Ok(results)
}

/// Execute a computed field (e.g., count(->posts)).
async fn exec_computed(
    ctx: &ExecutorContext,
    entity_id: Uuid,
    func: &str,
    args: &[Field],
) -> Result<Value> {
    match func.to_lowercase().as_str() {
        "count" => {
            // count(->edge) — count outgoing edges of a type.
            if let Some(Field::Graph(trav)) = args.first() {
                if !ctx.dialect.supports_graph_traversal() {
                    return Err(refuse_unsupported(
                        ctx.dialect.name(),
                        "computed count() over graph traversal",
                    ));
                }
                let results = exec_graph_traversal(ctx, entity_id, trav).await?;
                Ok(json!(results.len()))
            } else {
                Ok(json!(0))
            }
        }
        "sum" | "avg" | "min" | "max" => {
            // Aggregate functions over attributes — not graph-based.
            // These would require more context; return placeholder.
            Ok(json!(null))
        }
        _ => Ok(json!(null)),
    }
}

fn format_graph_key(trav: &GraphTraversal) -> String {
    let mut key = String::new();
    for step in &trav.steps {
        match step.direction {
            EdgeDirection::Out => key.push_str("->"),
            EdgeDirection::In => key.push_str("<-"),
        }
        key.push_str(&step.edge);
    }
    key
}

/// Convert a RecordId to a deterministic UUID (v5).
fn record_id_to_uuid(rec: &RecordId) -> Uuid {
    // Try parsing as UUID first.
    if let Ok(uid) = rec.id.parse::<Uuid>() {
        return uid;
    }
    // Generate deterministic UUID from table:id.
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("{}:{}", rec.table, rec.id).as_bytes(),
    )
}

/// Convert SetOrContent to key-value pairs.
fn data_to_pairs(data: &SetOrContent) -> Result<Vec<(String, Expr)>> {
    match data {
        SetOrContent::Set(pairs) => Ok(pairs.clone()),
        SetOrContent::Content(obj) => {
            if let Some(map) = obj.as_object() {
                Ok(map
                    .iter()
                    .map(|(k, v)| (k.clone(), Expr::Value(v.clone())))
                    .collect())
            } else {
                Err(DarshJError::InvalidQuery(
                    "CONTENT must be a JSON object".into(),
                ))
            }
        }
    }
}

/// Convert an expression to a JSON value for storage.
fn expr_to_json(expr: &Expr) -> Result<Value> {
    match expr {
        Expr::Value(v) => Ok(v.clone()),
        Expr::RecordLink(rec) => Ok(json!(rec.to_string())),
        Expr::Ident(s) => Ok(json!(s)),
        Expr::FnCall { name, args } => {
            let json_args: Vec<Value> = args
                .iter()
                .map(|a| expr_to_json(a).unwrap_or(Value::Null))
                .collect();
            Ok(json!({ "fn": name, "args": json_args }))
        }
        Expr::Cast { expr, .. } => expr_to_json(expr),
        Expr::Paren(inner) => expr_to_json(inner),
        _ => Ok(Value::Null),
    }
}

/// Cast a JSON value to the specified DarshQL type.
fn cast_value(val: &Value, target: &DarshType) -> Value {
    match target {
        DarshType::Int => match val {
            Value::Number(n) => json!(n.as_i64().unwrap_or(0)),
            Value::String(s) => json!(s.parse::<i64>().unwrap_or(0)),
            Value::Bool(b) => json!(if *b { 1 } else { 0 }),
            _ => json!(0),
        },
        DarshType::Float => match val {
            Value::Number(n) => json!(n.as_f64().unwrap_or(0.0)),
            Value::String(s) => json!(s.parse::<f64>().unwrap_or(0.0)),
            _ => json!(0.0),
        },
        DarshType::String => match val {
            Value::String(_) => val.clone(),
            _ => json!(val.to_string()),
        },
        DarshType::Bool => match val {
            Value::Bool(_) => val.clone(),
            Value::Number(n) => json!(n.as_i64().unwrap_or(0) != 0),
            Value::String(s) => json!(s == "true" || s == "1"),
            _ => json!(false),
        },
        _ => val.clone(),
    }
}

/// Bind a JSON value as the appropriate sqlx parameter type (for multi-column queries).
type TripleRow = (Uuid, String, Value, i16, i64, chrono::DateTime<chrono::Utc>);

fn bind_param<'q>(
    query: sqlx::query::QueryAs<'q, sqlx::Postgres, TripleRow, sqlx::postgres::PgArguments>,
    param: &'q Value,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, TripleRow, sqlx::postgres::PgArguments> {
    match param {
        Value::String(s) => query.bind(s.as_str()),
        _ => query.bind(param),
    }
}

/// Bind for single-column queries.
fn bind_param_single<'q>(
    query: sqlx::query::QueryAs<'q, sqlx::Postgres, (Uuid,), sqlx::postgres::PgArguments>,
    param: &'q Value,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, (Uuid,), sqlx::postgres::PgArguments> {
    match param {
        Value::String(s) => query.bind(s.as_str()),
        _ => query.bind(param),
    }
}
