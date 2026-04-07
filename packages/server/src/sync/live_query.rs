//! LIVE SELECT query engine for DarshJDB.
//!
//! Provides SurrealDB-style `LIVE SELECT` functionality: clients register a
//! live query with a collection target and optional filter predicate. When any
//! mutation (INSERT, UPDATE, DELETE) touches that collection and satisfies the
//! filter, the change is pushed to all matching subscribers in real time.
//!
//! # Protocol
//!
//! ```text
//! Client → { "type": "live-select", "id": "<req>", "query": "LIVE SELECT * FROM users WHERE age > 18" }
//! Server → { "type": "live-select-ok", "id": "<req>", "live_id": "<uuid>" }
//!
//! // On matching change:
//! Server → { "type": "live-event", "live_id": "<uuid>", "action": "CREATE|UPDATE|DELETE",
//!            "result": { ... }, "tx_id": N }
//!
//! Client → { "type": "kill", "id": "<req>", "live_id": "<uuid>" }
//! Server → { "type": "kill-ok", "id": "<req>", "live_id": "<uuid>" }
//! ```
//!
//! # Architecture
//!
//! The [`LiveQueryManager`] maintains a global set of live queries indexed by
//! `LiveQueryId`. Each live query stores:
//!
//! - The target collection (e.g., `users`)
//! - A parsed filter predicate for evaluating changes
//! - The owning session ID for cleanup on disconnect
//!
//! When a [`ChangeEvent`] arrives, the manager:
//! 1. Filters to live queries targeting the same collection
//! 2. Evaluates each query's filter against the changed entity data
//! 3. Produces [`LiveEvent`] payloads for matching subscriptions

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

use super::broadcaster::ChangeEvent;
use super::session::SessionId;

/// Unique identifier for a live query subscription.
pub type LiveQueryId = uuid::Uuid;

// ---------------------------------------------------------------------------
// Filter predicate
// ---------------------------------------------------------------------------

/// A parsed WHERE-clause filter for live query evaluation.
///
/// Supports a subset of comparison operators applied to entity fields:
/// `=`, `!=`, `>`, `>=`, `<`, `<=`, `CONTAINS`, `IN`.
///
/// Compound filters use AND/OR with nested predicates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FilterPredicate {
    /// field = value
    Eq { field: String, value: Value },
    /// field != value
    Ne { field: String, value: Value },
    /// field > value (numeric comparison)
    Gt { field: String, value: Value },
    /// field >= value (numeric comparison)
    Gte { field: String, value: Value },
    /// field < value (numeric comparison)
    Lt { field: String, value: Value },
    /// field <= value (numeric comparison)
    Lte { field: String, value: Value },
    /// field CONTAINS value (string contains or array contains)
    Contains { field: String, value: Value },
    /// field IN [values...] (membership check)
    In { field: String, values: Vec<Value> },
    /// All sub-predicates must be true
    And { predicates: Vec<FilterPredicate> },
    /// At least one sub-predicate must be true
    Or { predicates: Vec<FilterPredicate> },
    /// Negation of a sub-predicate
    Not { predicate: Box<FilterPredicate> },
    /// Always matches (no WHERE clause)
    All,
}

impl FilterPredicate {
    /// Evaluate this predicate against an entity (a JSON object).
    pub fn matches(&self, entity: &Value) -> bool {
        match self {
            FilterPredicate::All => true,

            FilterPredicate::Eq { field, value } => {
                entity.get(field).map_or(false, |v| v == value)
            }

            FilterPredicate::Ne { field, value } => {
                entity.get(field).map_or(true, |v| v != value)
            }

            FilterPredicate::Gt { field, value } => {
                compare_numeric(entity.get(field), value, |a, b| a > b)
            }

            FilterPredicate::Gte { field, value } => {
                compare_numeric(entity.get(field), value, |a, b| a >= b)
            }

            FilterPredicate::Lt { field, value } => {
                compare_numeric(entity.get(field), value, |a, b| a < b)
            }

            FilterPredicate::Lte { field, value } => {
                compare_numeric(entity.get(field), value, |a, b| a <= b)
            }

            FilterPredicate::Contains { field, value } => {
                match entity.get(field) {
                    Some(Value::String(s)) => {
                        if let Some(needle) = value.as_str() {
                            s.contains(needle)
                        } else {
                            false
                        }
                    }
                    Some(Value::Array(arr)) => arr.contains(value),
                    _ => false,
                }
            }

            FilterPredicate::In { field, values } => {
                entity.get(field).map_or(false, |v| values.contains(v))
            }

            FilterPredicate::And { predicates } => {
                predicates.iter().all(|p| p.matches(entity))
            }

            FilterPredicate::Or { predicates } => {
                predicates.iter().any(|p| p.matches(entity))
            }

            FilterPredicate::Not { predicate } => !predicate.matches(entity),
        }
    }
}

/// Compare a field value against a threshold using a numeric comparator.
fn compare_numeric(field_val: Option<&Value>, threshold: &Value, cmp: fn(f64, f64) -> bool) -> bool {
    let a = field_val.and_then(as_f64);
    let b = as_f64(threshold);
    match (a, b) {
        (Some(a), Some(b)) => cmp(a, b),
        _ => false,
    }
}

/// Extract an f64 from a JSON value (numbers and numeric strings).
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Live query SQL-like parser
// ---------------------------------------------------------------------------

/// Parsed LIVE SELECT statement.
#[derive(Debug, Clone)]
pub struct ParsedLiveSelect {
    /// Target collection (e.g., `users`).
    pub collection: String,
    /// Optional SELECT fields (`*` means all).
    pub fields: LiveSelectFields,
    /// Optional WHERE filter.
    pub filter: FilterPredicate,
}

/// Selected fields in a LIVE SELECT.
#[derive(Debug, Clone)]
pub enum LiveSelectFields {
    /// `SELECT *`
    All,
    /// `SELECT field1, field2, ...`
    Named(Vec<String>),
}

/// Parse a `LIVE SELECT` statement string into structured components.
///
/// Supported syntax:
/// ```text
/// LIVE SELECT * FROM <collection>
/// LIVE SELECT * FROM <collection> WHERE <field> <op> <value>
/// LIVE SELECT field1, field2 FROM <collection> WHERE <field> <op> <value> AND <field> <op> <value>
/// ```
pub fn parse_live_select(input: &str) -> Result<ParsedLiveSelect, String> {
    let input = input.trim();

    // Case-insensitive prefix check.
    let upper = input.to_uppercase();
    if !upper.starts_with("LIVE SELECT") && !upper.starts_with("LIVE SELECT") {
        return Err("query must start with LIVE SELECT".into());
    }

    // Strip the "LIVE SELECT" prefix.
    let rest = input[11..].trim();

    // Split on FROM (case-insensitive).
    let from_pos = rest
        .to_uppercase()
        .find(" FROM ")
        .ok_or("missing FROM clause")?;

    let fields_str = rest[..from_pos].trim();
    let after_from = rest[from_pos + 6..].trim();

    // Parse fields.
    let fields = if fields_str == "*" {
        LiveSelectFields::All
    } else {
        LiveSelectFields::Named(
            fields_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        )
    };

    // Split on WHERE (case-insensitive).
    let (collection, where_clause) = match after_from.to_uppercase().find(" WHERE ") {
        Some(pos) => {
            let coll = after_from[..pos].trim().to_string();
            let where_str = after_from[pos + 7..].trim();
            (coll, Some(where_str.to_string()))
        }
        None => (after_from.trim().to_string(), None),
    };

    if collection.is_empty() {
        return Err("missing collection name after FROM".into());
    }

    // Parse WHERE clause into filter predicates.
    let filter = match where_clause {
        Some(clause) => parse_where_clause(&clause)?,
        None => FilterPredicate::All,
    };

    Ok(ParsedLiveSelect {
        collection,
        fields,
        filter,
    })
}

/// Parse a WHERE clause into a compound filter predicate.
///
/// Supports AND-chained simple conditions: `field op value [AND field op value]*`
fn parse_where_clause(clause: &str) -> Result<FilterPredicate, String> {
    let clause = clause.trim();
    if clause.is_empty() {
        return Ok(FilterPredicate::All);
    }

    // Split on OR first (lower precedence), then AND.
    let or_parts = split_preserving_strings(clause, " OR ");
    if or_parts.len() > 1 {
        let predicates: Result<Vec<_>, _> = or_parts.iter().map(|p| parse_where_clause(p)).collect();
        return Ok(FilterPredicate::Or {
            predicates: predicates?,
        });
    }

    // Split on AND.
    let and_parts = split_preserving_strings(clause, " AND ");
    if and_parts.len() > 1 {
        let predicates: Result<Vec<_>, _> = and_parts.iter().map(|p| parse_single_condition(p)).collect();
        return Ok(FilterPredicate::And {
            predicates: predicates?,
        });
    }

    parse_single_condition(clause)
}

/// Split a string on a delimiter, but only outside quoted strings.
fn split_preserving_strings(input: &str, delimiter: &str) -> Vec<String> {
    let upper = input.to_uppercase();
    let delim_upper = delimiter.to_uppercase();
    let mut parts = Vec::new();
    let mut last = 0;
    let mut in_quote = false;
    let mut quote_char = ' ';

    let bytes = input.as_bytes();
    let delim_bytes = delim_upper.as_bytes();

    let mut i = 0;
    while i < bytes.len() {
        let ch = bytes[i] as char;

        if in_quote {
            if ch == quote_char {
                in_quote = false;
            }
            i += 1;
            continue;
        }

        if ch == '\'' || ch == '"' {
            in_quote = true;
            quote_char = ch;
            i += 1;
            continue;
        }

        if i + delim_bytes.len() <= bytes.len() {
            let slice = &upper.as_bytes()[i..i + delim_bytes.len()];
            if slice == delim_bytes {
                parts.push(input[last..i].trim().to_string());
                i += delim_bytes.len();
                last = i;
                continue;
            }
        }

        i += 1;
    }

    parts.push(input[last..].trim().to_string());
    parts
}

/// Parse a single condition like `age > 18` or `name = 'Alice'`.
fn parse_single_condition(cond: &str) -> Result<FilterPredicate, String> {
    let cond = cond.trim();

    // Try two-character operators first, then single-character.
    let operators = ["!=", ">=", "<=", "CONTAINS", "IN", "=", ">", "<"];

    for op in &operators {
        let search = if *op == "CONTAINS" || *op == "IN" {
            // These need space boundaries.
            format!(" {} ", op)
        } else {
            op.to_string()
        };

        let upper_cond = cond.to_uppercase();
        let search_upper = search.to_uppercase();

        if let Some(pos) = upper_cond.find(&search_upper) {
            let field = cond[..pos].trim().to_string();
            let value_str = cond[pos + search.len()..].trim();

            if field.is_empty() {
                return Err(format!("missing field name before operator '{op}'"));
            }

            let value = parse_value_literal(value_str)?;

            return match *op {
                "=" => Ok(FilterPredicate::Eq { field, value }),
                "!=" => Ok(FilterPredicate::Ne { field, value }),
                ">" => Ok(FilterPredicate::Gt { field, value }),
                ">=" => Ok(FilterPredicate::Gte { field, value }),
                "<" => Ok(FilterPredicate::Lt { field, value }),
                "<=" => Ok(FilterPredicate::Lte { field, value }),
                "CONTAINS" => Ok(FilterPredicate::Contains { field, value }),
                "IN" => {
                    // Parse as array: [val1, val2, ...]
                    let values = parse_value_array(value_str)?;
                    Ok(FilterPredicate::In { field, values })
                }
                _ => Err(format!("unsupported operator: {op}")),
            };
        }
    }

    Err(format!("could not parse condition: {cond}"))
}

/// Parse a literal value from a WHERE clause.
fn parse_value_literal(s: &str) -> Result<Value, String> {
    let s = s.trim();

    // Quoted string.
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        return Ok(Value::String(s[1..s.len() - 1].to_string()));
    }

    // Boolean.
    if s.eq_ignore_ascii_case("true") {
        return Ok(Value::Bool(true));
    }
    if s.eq_ignore_ascii_case("false") {
        return Ok(Value::Bool(false));
    }

    // Null.
    if s.eq_ignore_ascii_case("null") || s.eq_ignore_ascii_case("none") {
        return Ok(Value::Null);
    }

    // Number (integer or float).
    if let Ok(n) = s.parse::<i64>() {
        return Ok(Value::Number(n.into()));
    }
    if let Ok(n) = s.parse::<f64>() {
        return Ok(Value::Number(
            serde_json::Number::from_f64(n).ok_or("invalid float")?,
        ));
    }

    // Fallback: treat as unquoted string.
    Ok(Value::String(s.to_string()))
}

/// Parse an array literal like `[1, 2, 3]` or `['a', 'b']`.
fn parse_value_array(s: &str) -> Result<Vec<Value>, String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return Err("IN values must be an array like [1, 2, 3]".into());
    }
    let inner = &s[1..s.len() - 1];
    inner
        .split(',')
        .map(|item| parse_value_literal(item.trim()))
        .collect()
}

// ---------------------------------------------------------------------------
// Live event
// ---------------------------------------------------------------------------

/// Action that triggered a live event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum LiveAction {
    Create,
    Update,
    Delete,
}

impl std::fmt::Display for LiveAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveAction::Create => write!(f, "CREATE"),
            LiveAction::Update => write!(f, "UPDATE"),
            LiveAction::Delete => write!(f, "DELETE"),
        }
    }
}

/// An event pushed to a live query subscriber.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveEvent {
    /// The live query ID this event belongs to.
    pub live_id: LiveQueryId,
    /// What mutation triggered this event.
    pub action: LiveAction,
    /// The entity data (post-mutation for CREATE/UPDATE, pre-mutation for DELETE).
    pub result: Value,
    /// Transaction ID.
    pub tx_id: i64,
}

// ---------------------------------------------------------------------------
// Live query registration
// ---------------------------------------------------------------------------

/// A registered live query.
#[derive(Debug, Clone)]
pub struct LiveQuery {
    /// Unique ID for this live query.
    pub id: LiveQueryId,
    /// Target collection (entity type).
    pub collection: String,
    /// Which fields to include in events.
    pub fields: LiveSelectFields,
    /// Filter predicate to evaluate against changed entities.
    pub filter: FilterPredicate,
    /// Owning session (for cleanup on disconnect).
    pub session_id: SessionId,
    /// Original query string for debugging.
    pub raw_query: String,
}

// ---------------------------------------------------------------------------
// LiveQueryManager
// ---------------------------------------------------------------------------

/// Thread-safe manager for all active LIVE SELECT subscriptions.
///
/// Tracks all live queries globally. When a mutation is committed, the
/// broadcaster calls [`process_change`] to evaluate which live queries
/// match the change and produce [`LiveEvent`] payloads.
pub struct LiveQueryManager {
    /// Active live queries keyed by their unique ID.
    queries: RwLock<HashMap<LiveQueryId, LiveQuery>>,
    /// Reverse index: session_id -> set of live query IDs, for fast cleanup.
    by_session: RwLock<HashMap<SessionId, HashSet<LiveQueryId>>>,
    /// Broadcast channel for live events (subscribers receive via rx).
    event_tx: tokio::sync::broadcast::Sender<LiveEvent>,
}

impl LiveQueryManager {
    /// Create a new live query manager with the given broadcast capacity.
    pub fn new(capacity: usize) -> (Arc<Self>, tokio::sync::broadcast::Receiver<LiveEvent>) {
        let (event_tx, event_rx) = tokio::sync::broadcast::channel(capacity);
        let mgr = Arc::new(Self {
            queries: RwLock::new(HashMap::new()),
            by_session: RwLock::new(HashMap::new()),
            event_tx,
        });
        (mgr, event_rx)
    }

    /// Get a new broadcast receiver for live events.
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<LiveEvent> {
        self.event_tx.subscribe()
    }

    /// Register a new LIVE SELECT query.
    ///
    /// Parses the query string, creates a [`LiveQuery`], and returns
    /// the assigned [`LiveQueryId`].
    pub fn register(
        &self,
        session_id: SessionId,
        query_str: &str,
    ) -> Result<LiveQueryId, String> {
        let parsed = parse_live_select(query_str)?;

        let live_id = LiveQueryId::new_v4();
        let query = LiveQuery {
            id: live_id,
            collection: parsed.collection,
            fields: parsed.fields,
            filter: parsed.filter,
            session_id,
            raw_query: query_str.to_string(),
        };

        {
            let mut queries = self.queries.write().expect("live query lock poisoned");
            queries.insert(live_id, query);
        }
        {
            let mut by_session = self.by_session.write().expect("live query session lock poisoned");
            by_session.entry(session_id).or_default().insert(live_id);
        }

        debug!(
            live_id = %live_id,
            session_id = %session_id,
            query = query_str,
            "live query registered"
        );

        Ok(live_id)
    }

    /// Unregister (kill) a live query by ID.
    ///
    /// Returns `true` if the query existed and was removed.
    pub fn kill(&self, live_id: &LiveQueryId, session_id: &SessionId) -> bool {
        let removed = {
            let mut queries = self.queries.write().expect("live query lock poisoned");
            let entry = queries.get(live_id);

            // Verify ownership.
            if let Some(q) = entry {
                if q.session_id != *session_id {
                    warn!(
                        live_id = %live_id,
                        owner = %q.session_id,
                        requester = %session_id,
                        "kill rejected: session does not own this live query"
                    );
                    return false;
                }
            }

            queries.remove(live_id).is_some()
        };

        if removed {
            let mut by_session = self.by_session.write().expect("live query session lock poisoned");
            if let Some(set) = by_session.get_mut(session_id) {
                set.remove(live_id);
                if set.is_empty() {
                    by_session.remove(session_id);
                }
            }

            debug!(
                live_id = %live_id,
                session_id = %session_id,
                "live query killed"
            );
        }

        removed
    }

    /// Remove all live queries for a session (on disconnect).
    ///
    /// Returns the number of queries removed.
    pub fn kill_session(&self, session_id: &SessionId) -> usize {
        let live_ids: Vec<LiveQueryId> = {
            let mut by_session = self.by_session.write().expect("live query session lock poisoned");
            match by_session.remove(session_id) {
                Some(ids) => ids.into_iter().collect(),
                None => return 0,
            }
        };

        let count = live_ids.len();
        {
            let mut queries = self.queries.write().expect("live query lock poisoned");
            for id in &live_ids {
                queries.remove(id);
            }
        }

        if count > 0 {
            debug!(
                session_id = %session_id,
                removed = count,
                "live queries cleaned up on disconnect"
            );
        }

        count
    }

    /// Process a change event and produce live events for matching queries.
    ///
    /// `entity_data` is the post-mutation state of affected entities, keyed
    /// by entity ID. For DELETE operations, this should contain the pre-deletion
    /// snapshot.
    ///
    /// Returns the list of generated [`LiveEvent`]s, also broadcast on the
    /// internal channel.
    pub fn process_change(
        &self,
        event: &ChangeEvent,
        entity_data: &HashMap<String, Value>,
        action: LiveAction,
    ) -> Vec<(SessionId, LiveEvent)> {
        let queries = self.queries.read().expect("live query lock poisoned");
        let mut results = Vec::new();

        for (_, query) in queries.iter() {
            // Collection filter: skip if the change doesn't target this collection.
            if let Some(ref event_type) = event.entity_type {
                if *event_type != query.collection {
                    continue;
                }
            } else {
                // No entity type on the event -- cannot match collection-specific queries.
                continue;
            }

            // For each affected entity, evaluate the filter.
            for entity_id in &event.entity_ids {
                let entity = match entity_data.get(entity_id) {
                    Some(e) => e,
                    None => continue,
                };

                if !query.filter.matches(entity) {
                    continue;
                }

                // Apply field projection.
                let projected = project_fields(entity, &query.fields);

                let live_event = LiveEvent {
                    live_id: query.id,
                    action,
                    result: projected,
                    tx_id: event.tx_id,
                };

                // Broadcast on the channel.
                let _ = self.event_tx.send(live_event.clone());

                results.push((query.session_id, live_event));
            }
        }

        results
    }

    /// Get the total number of active live queries.
    pub fn query_count(&self) -> usize {
        self.queries.read().expect("live query lock poisoned").len()
    }

    /// List all live query IDs for a given session.
    pub fn session_queries(&self, session_id: &SessionId) -> Vec<LiveQueryId> {
        self.by_session
            .read()
            .expect("live query session lock poisoned")
            .get(session_id)
            .map(|ids| ids.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// Project entity fields according to the LIVE SELECT field list.
fn project_fields(entity: &Value, fields: &LiveSelectFields) -> Value {
    match fields {
        LiveSelectFields::All => entity.clone(),
        LiveSelectFields::Named(names) => {
            let obj = match entity.as_object() {
                Some(o) => o,
                None => return entity.clone(),
            };
            let mut projected = serde_json::Map::new();
            // Always include the ID field.
            for id_key in &["_id", "id", "entity_id"] {
                if let Some(v) = obj.get(*id_key) {
                    projected.insert(id_key.to_string(), v.clone());
                }
            }
            for name in names {
                if let Some(v) = obj.get(name) {
                    projected.insert(name.clone(), v.clone());
                }
            }
            Value::Object(projected)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // FilterPredicate evaluation
    // -----------------------------------------------------------------------

    #[test]
    fn filter_all_matches_everything() {
        let f = FilterPredicate::All;
        assert!(f.matches(&json!({})));
        assert!(f.matches(&json!({"x": 1})));
    }

    #[test]
    fn filter_eq() {
        let f = FilterPredicate::Eq {
            field: "age".into(),
            value: json!(25),
        };
        assert!(f.matches(&json!({"age": 25})));
        assert!(!f.matches(&json!({"age": 30})));
        assert!(!f.matches(&json!({"name": "Alice"})));
    }

    #[test]
    fn filter_ne() {
        let f = FilterPredicate::Ne {
            field: "status".into(),
            value: json!("inactive"),
        };
        assert!(f.matches(&json!({"status": "active"})));
        assert!(!f.matches(&json!({"status": "inactive"})));
        // Missing field -> ne is true.
        assert!(f.matches(&json!({})));
    }

    #[test]
    fn filter_gt() {
        let f = FilterPredicate::Gt {
            field: "age".into(),
            value: json!(18),
        };
        assert!(f.matches(&json!({"age": 19})));
        assert!(f.matches(&json!({"age": 100})));
        assert!(!f.matches(&json!({"age": 18})));
        assert!(!f.matches(&json!({"age": 17})));
    }

    #[test]
    fn filter_gte() {
        let f = FilterPredicate::Gte {
            field: "score".into(),
            value: json!(50),
        };
        assert!(f.matches(&json!({"score": 50})));
        assert!(f.matches(&json!({"score": 51})));
        assert!(!f.matches(&json!({"score": 49})));
    }

    #[test]
    fn filter_lt_lte() {
        let lt = FilterPredicate::Lt {
            field: "price".into(),
            value: json!(100),
        };
        assert!(lt.matches(&json!({"price": 99})));
        assert!(!lt.matches(&json!({"price": 100})));

        let lte = FilterPredicate::Lte {
            field: "price".into(),
            value: json!(100),
        };
        assert!(lte.matches(&json!({"price": 100})));
        assert!(!lte.matches(&json!({"price": 101})));
    }

    #[test]
    fn filter_contains_string() {
        let f = FilterPredicate::Contains {
            field: "name".into(),
            value: json!("Ali"),
        };
        assert!(f.matches(&json!({"name": "Alice"})));
        assert!(!f.matches(&json!({"name": "Bob"})));
    }

    #[test]
    fn filter_contains_array() {
        let f = FilterPredicate::Contains {
            field: "tags".into(),
            value: json!("rust"),
        };
        assert!(f.matches(&json!({"tags": ["rust", "wasm"]})));
        assert!(!f.matches(&json!({"tags": ["python"]})));
    }

    #[test]
    fn filter_in() {
        let f = FilterPredicate::In {
            field: "role".into(),
            values: vec![json!("admin"), json!("moderator")],
        };
        assert!(f.matches(&json!({"role": "admin"})));
        assert!(f.matches(&json!({"role": "moderator"})));
        assert!(!f.matches(&json!({"role": "user"})));
    }

    #[test]
    fn filter_and() {
        let f = FilterPredicate::And {
            predicates: vec![
                FilterPredicate::Gt {
                    field: "age".into(),
                    value: json!(18),
                },
                FilterPredicate::Eq {
                    field: "active".into(),
                    value: json!(true),
                },
            ],
        };
        assert!(f.matches(&json!({"age": 25, "active": true})));
        assert!(!f.matches(&json!({"age": 25, "active": false})));
        assert!(!f.matches(&json!({"age": 17, "active": true})));
    }

    #[test]
    fn filter_or() {
        let f = FilterPredicate::Or {
            predicates: vec![
                FilterPredicate::Eq {
                    field: "role".into(),
                    value: json!("admin"),
                },
                FilterPredicate::Eq {
                    field: "role".into(),
                    value: json!("super"),
                },
            ],
        };
        assert!(f.matches(&json!({"role": "admin"})));
        assert!(f.matches(&json!({"role": "super"})));
        assert!(!f.matches(&json!({"role": "user"})));
    }

    #[test]
    fn filter_not() {
        let f = FilterPredicate::Not {
            predicate: Box::new(FilterPredicate::Eq {
                field: "banned".into(),
                value: json!(true),
            }),
        };
        assert!(f.matches(&json!({"banned": false})));
        assert!(!f.matches(&json!({"banned": true})));
    }

    // -----------------------------------------------------------------------
    // LIVE SELECT parser
    // -----------------------------------------------------------------------

    #[test]
    fn parse_simple_select_all() {
        let result = parse_live_select("LIVE SELECT * FROM users").unwrap();
        assert_eq!(result.collection, "users");
        assert!(matches!(result.fields, LiveSelectFields::All));
        assert!(matches!(result.filter, FilterPredicate::All));
    }

    #[test]
    fn parse_select_with_where() {
        let result =
            parse_live_select("LIVE SELECT * FROM users WHERE age > 18").unwrap();
        assert_eq!(result.collection, "users");
        assert!(matches!(result.filter, FilterPredicate::Gt { .. }));
    }

    #[test]
    fn parse_select_named_fields() {
        let result =
            parse_live_select("LIVE SELECT name, email FROM users WHERE active = true").unwrap();
        assert_eq!(result.collection, "users");
        match &result.fields {
            LiveSelectFields::Named(names) => {
                assert_eq!(names, &["name", "email"]);
            }
            _ => panic!("expected named fields"),
        }
    }

    #[test]
    fn parse_select_compound_where() {
        let result =
            parse_live_select("LIVE SELECT * FROM orders WHERE status = 'pending' AND total > 100")
                .unwrap();
        assert_eq!(result.collection, "orders");
        assert!(matches!(result.filter, FilterPredicate::And { .. }));
    }

    #[test]
    fn parse_select_missing_from() {
        let result = parse_live_select("LIVE SELECT * users");
        assert!(result.is_err());
    }

    #[test]
    fn parse_select_case_insensitive() {
        let result = parse_live_select("live select * from Users where Age > 18").unwrap();
        assert_eq!(result.collection, "Users");
    }

    // -----------------------------------------------------------------------
    // LiveQueryManager
    // -----------------------------------------------------------------------

    #[test]
    fn manager_register_and_kill() {
        let (mgr, _rx) = LiveQueryManager::new(64);
        let sid = SessionId::new_v4();

        let live_id = mgr
            .register(sid, "LIVE SELECT * FROM users WHERE age > 18")
            .unwrap();
        assert_eq!(mgr.query_count(), 1);
        assert_eq!(mgr.session_queries(&sid).len(), 1);

        assert!(mgr.kill(&live_id, &sid));
        assert_eq!(mgr.query_count(), 0);
    }

    #[test]
    fn manager_kill_wrong_session_rejected() {
        let (mgr, _rx) = LiveQueryManager::new(64);
        let sid1 = SessionId::new_v4();
        let sid2 = SessionId::new_v4();

        let live_id = mgr
            .register(sid1, "LIVE SELECT * FROM users")
            .unwrap();

        // Another session cannot kill this query.
        assert!(!mgr.kill(&live_id, &sid2));
        assert_eq!(mgr.query_count(), 1);
    }

    #[test]
    fn manager_kill_session_cleans_all() {
        let (mgr, _rx) = LiveQueryManager::new(64);
        let sid = SessionId::new_v4();

        mgr.register(sid, "LIVE SELECT * FROM users").unwrap();
        mgr.register(sid, "LIVE SELECT * FROM orders").unwrap();
        mgr.register(sid, "LIVE SELECT * FROM products").unwrap();

        assert_eq!(mgr.query_count(), 3);

        let removed = mgr.kill_session(&sid);
        assert_eq!(removed, 3);
        assert_eq!(mgr.query_count(), 0);
    }

    #[test]
    fn manager_process_change_matching() {
        let (mgr, _rx) = LiveQueryManager::new(64);
        let sid = SessionId::new_v4();

        mgr.register(sid, "LIVE SELECT * FROM users WHERE age > 18")
            .unwrap();

        let event = ChangeEvent {
            tx_id: 42,
            entity_ids: vec!["user-1".into()],
            attributes: vec!["age".into()],
            entity_type: Some("users".into()),
            actor_id: None,
        };

        let mut entity_data = HashMap::new();
        entity_data.insert(
            "user-1".to_string(),
            json!({"_id": "user-1", "name": "Alice", "age": 25}),
        );

        let results = mgr.process_change(&event, &entity_data, LiveAction::Update);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, sid);
        assert_eq!(results[0].1.action, LiveAction::Update);
    }

    #[test]
    fn manager_process_change_no_match_filter() {
        let (mgr, _rx) = LiveQueryManager::new(64);
        let sid = SessionId::new_v4();

        mgr.register(sid, "LIVE SELECT * FROM users WHERE age > 18")
            .unwrap();

        let event = ChangeEvent {
            tx_id: 43,
            entity_ids: vec!["user-2".into()],
            attributes: vec!["age".into()],
            entity_type: Some("users".into()),
            actor_id: None,
        };

        let mut entity_data = HashMap::new();
        entity_data.insert(
            "user-2".to_string(),
            json!({"_id": "user-2", "name": "Bob", "age": 16}),
        );

        let results = mgr.process_change(&event, &entity_data, LiveAction::Create);
        assert!(results.is_empty(), "age=16 should not match age > 18");
    }

    #[test]
    fn manager_process_change_wrong_collection() {
        let (mgr, _rx) = LiveQueryManager::new(64);
        let sid = SessionId::new_v4();

        mgr.register(sid, "LIVE SELECT * FROM users").unwrap();

        let event = ChangeEvent {
            tx_id: 44,
            entity_ids: vec!["order-1".into()],
            attributes: vec!["total".into()],
            entity_type: Some("orders".into()),
            actor_id: None,
        };

        let mut entity_data = HashMap::new();
        entity_data.insert("order-1".to_string(), json!({"_id": "order-1", "total": 99}));

        let results = mgr.process_change(&event, &entity_data, LiveAction::Create);
        assert!(results.is_empty(), "orders change should not match users query");
    }

    // -----------------------------------------------------------------------
    // Field projection
    // -----------------------------------------------------------------------

    #[test]
    fn project_all_returns_full_entity() {
        let entity = json!({"_id": "1", "name": "Alice", "age": 25});
        let result = project_fields(&entity, &LiveSelectFields::All);
        assert_eq!(result, entity);
    }

    #[test]
    fn project_named_fields_includes_id() {
        let entity = json!({"_id": "1", "name": "Alice", "age": 25, "email": "a@b.c"});
        let result = project_fields(&entity, &LiveSelectFields::Named(vec!["name".into()]));
        let obj = result.as_object().unwrap();
        assert!(obj.contains_key("_id")); // always included
        assert!(obj.contains_key("name"));
        assert!(!obj.contains_key("age"));
        assert!(!obj.contains_key("email"));
    }
}
