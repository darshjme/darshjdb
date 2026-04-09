//! SurrealDB-style row-level permission engine for DarshJDB.
//!
//! Provides declarative per-table, per-operation permission expressions that
//! are evaluated against the `$auth` context at query time. Permission
//! expressions support field references, comparisons, logical operators,
//! and `$auth.*` variable substitution.
//!
//! # Permission DSL
//!
//! ```text
//! DEFINE TABLE posts PERMISSIONS
//!   FOR select WHERE published = true OR user = $auth.id
//!   FOR create WHERE $auth.role = "admin"
//!   FOR update WHERE user = $auth.id
//!   FOR delete WHERE $auth.role = "admin"
//! ```
//!
//! # Architecture
//!
//! ```text
//! Query ──▶ RowLevelSecurity::evaluate()
//!                  │
//!                  ├── Lookup table permissions from _permissions store
//!                  ├── Substitute $auth.* variables
//!                  ├── For reads: inject WHERE clause into query
//!                  └── For writes: evaluate expression as gate (allow/deny)
//! ```
//!
//! # Security
//!
//! - All `$auth.*` references are resolved to bind parameters, never
//!   interpolated as raw strings.
//! - Expressions are parsed at DEFINE time and stored as AST nodes,
//!   preventing injection at evaluation time.
//! - Unknown `$auth.*` fields resolve to NULL, which fails closed
//!   (comparisons with NULL yield false).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{AuthContext, AuthError};

// ---------------------------------------------------------------------------
// Permission expression AST
// ---------------------------------------------------------------------------

/// A parsed permission expression node.
///
/// Expressions are stored as an AST after parsing the `WHERE` clause
/// from a `DEFINE TABLE ... PERMISSIONS` statement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PermExpr {
    /// A literal boolean, string, number, or null value.
    Literal(LiteralValue),

    /// A reference to a field on the current row (e.g., `published`, `user`).
    FieldRef {
        /// The field name on the row being evaluated.
        field: String,
    },

    /// A reference to an `$auth.*` context variable.
    ///
    /// Supported paths:
    /// - `$auth.id` — user ID
    /// - `$auth.role` — checks membership in the roles array
    /// - `$auth.roles` — the full roles array
    /// - `$auth.session_id` — current session ID
    /// - `$auth.ip` — originating IP address
    AuthVar {
        /// The path after `$auth.` (e.g., `"id"`, `"role"`, `"roles"`).
        path: String,
    },

    /// Binary comparison: `lhs op rhs`.
    Compare {
        /// Left-hand side expression.
        lhs: Box<PermExpr>,
        /// Comparison operator.
        op: CompareOp,
        /// Right-hand side expression.
        rhs: Box<PermExpr>,
    },

    /// Logical AND of two expressions.
    And {
        left: Box<PermExpr>,
        right: Box<PermExpr>,
    },

    /// Logical OR of two expressions.
    Or {
        left: Box<PermExpr>,
        right: Box<PermExpr>,
    },

    /// Logical NOT of an expression.
    Not { inner: Box<PermExpr> },

    /// Membership check: `field CONTAINS value` or `value IN field`.
    Contains {
        /// The array-like expression.
        haystack: Box<PermExpr>,
        /// The value to search for.
        needle: Box<PermExpr>,
    },

    /// Always evaluates to true (FULL access).
    Full,

    /// Always evaluates to false (NONE, deny all).
    None,
}

/// Literal values in permission expressions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum LiteralValue {
    Bool(bool),
    String(String),
    Int(i64),
    Float(f64),
    Null,
}

/// Comparison operators supported in permission expressions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    /// `=` or `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

// ---------------------------------------------------------------------------
// Table permission definition
// ---------------------------------------------------------------------------

/// CRUD operation types for row-level permissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowOp {
    Select,
    Create,
    Update,
    Delete,
}

impl std::fmt::Display for RowOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Select => write!(f, "select"),
            Self::Create => write!(f, "create"),
            Self::Update => write!(f, "update"),
            Self::Delete => write!(f, "delete"),
        }
    }
}

/// Permission definition for a single table, mapping each operation
/// to its guard expression.
///
/// Mirrors SurrealDB's `DEFINE TABLE ... PERMISSIONS` syntax.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TablePermissions {
    /// The table name this permission set applies to.
    pub table: String,
    /// Per-operation permission expressions. An absent key means NONE (deny).
    pub permissions: HashMap<RowOp, PermExpr>,
}

impl TablePermissions {
    /// Create a new permission definition for a table with no permissions
    /// (all operations denied by default).
    pub fn new(table: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            permissions: HashMap::new(),
        }
    }

    /// Set the permission expression for an operation.
    pub fn set(&mut self, op: RowOp, expr: PermExpr) -> &mut Self {
        self.permissions.insert(op, expr);
        self
    }

    /// Builder-style: set permission and return self.
    pub fn with(mut self, op: RowOp, expr: PermExpr) -> Self {
        self.permissions.insert(op, expr);
        self
    }

    /// Get the permission expression for an operation.
    ///
    /// Returns `PermExpr::None` if no permission is defined (deny by default).
    pub fn get(&self, op: RowOp) -> &PermExpr {
        self.permissions.get(&op).unwrap_or(&PermExpr::None)
    }
}

// ---------------------------------------------------------------------------
// Auth variable resolution
// ---------------------------------------------------------------------------

/// Resolved `$auth.*` context for expression evaluation.
///
/// Built from an [`AuthContext`] for use during permission evaluation.
#[derive(Debug, Clone)]
pub struct AuthVars {
    /// User ID as a string (UUID format).
    pub id: String,
    /// Session ID as a string.
    pub session_id: String,
    /// User roles.
    pub roles: Vec<String>,
    /// Originating IP.
    pub ip: String,
    /// Custom claims from scope-based tokens (key-value pairs).
    pub custom_claims: HashMap<String, serde_json::Value>,
}

impl From<&AuthContext> for AuthVars {
    fn from(ctx: &AuthContext) -> Self {
        Self {
            id: ctx.user_id.to_string(),
            session_id: ctx.session_id.to_string(),
            roles: ctx.roles.clone(),
            ip: ctx.ip.clone(),
            custom_claims: HashMap::new(),
        }
    }
}

impl AuthVars {
    /// Resolve an `$auth.*` path to a runtime value.
    ///
    /// Returns `None` for unknown paths (which evaluates as NULL,
    /// causing comparisons to fail closed).
    pub fn resolve(&self, path: &str) -> Option<EvalValue> {
        match path {
            "id" => Some(EvalValue::String(self.id.clone())),
            "session_id" => Some(EvalValue::String(self.session_id.clone())),
            "role" | "roles" => Some(EvalValue::Array(
                self.roles
                    .iter()
                    .map(|r| EvalValue::String(r.clone()))
                    .collect(),
            )),
            "ip" => Some(EvalValue::String(self.ip.clone())),
            other => {
                // Check custom claims.
                self.custom_claims.get(other).map(json_to_eval)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Runtime evaluation values
// ---------------------------------------------------------------------------

/// Runtime value used during expression evaluation.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalValue {
    Bool(bool),
    String(String),
    Int(i64),
    Float(f64),
    Array(Vec<EvalValue>),
    Null,
}

impl EvalValue {
    /// Truthiness: false, null, and empty strings/arrays are falsy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            Self::String(s) => !s.is_empty(),
            Self::Int(n) => *n != 0,
            Self::Float(n) => *n != 0.0,
            Self::Array(a) => !a.is_empty(),
            Self::Null => false,
        }
    }

    /// Compare two values with a given operator.
    pub fn compare(&self, op: CompareOp, other: &EvalValue) -> bool {
        // NULL comparisons always fail closed (return false).
        if matches!(self, EvalValue::Null) || matches!(other, EvalValue::Null) {
            return false;
        }

        match op {
            CompareOp::Eq => self == other,
            CompareOp::Ne => self != other,
            CompareOp::Lt => self.partial_ord(other).is_some_and(|o| o.is_lt()),
            CompareOp::Le => self.partial_ord(other).is_some_and(|o| o.is_le()),
            CompareOp::Gt => self.partial_ord(other).is_some_and(|o| o.is_gt()),
            CompareOp::Ge => self.partial_ord(other).is_some_and(|o| o.is_ge()),
        }
    }

    /// Check if this value (array) contains the needle.
    pub fn contains(&self, needle: &EvalValue) -> bool {
        match self {
            EvalValue::Array(arr) => arr.contains(needle),
            EvalValue::String(s) => {
                if let EvalValue::String(n) = needle {
                    s.contains(n.as_str())
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn partial_ord(&self, other: &EvalValue) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (EvalValue::Int(a), EvalValue::Int(b)) => a.partial_cmp(b),
            (EvalValue::Float(a), EvalValue::Float(b)) => a.partial_cmp(b),
            (EvalValue::Int(a), EvalValue::Float(b)) => (*a as f64).partial_cmp(b),
            (EvalValue::Float(a), EvalValue::Int(b)) => a.partial_cmp(&(*b as f64)),
            (EvalValue::String(a), EvalValue::String(b)) => a.partial_cmp(b),
            _ => None,
        }
    }
}

/// Convert a serde_json::Value to EvalValue.
fn json_to_eval(v: &serde_json::Value) -> EvalValue {
    match v {
        serde_json::Value::Bool(b) => EvalValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                EvalValue::Int(i)
            } else if let Some(f) = n.as_f64() {
                EvalValue::Float(f)
            } else {
                EvalValue::Null
            }
        }
        serde_json::Value::String(s) => EvalValue::String(s.clone()),
        serde_json::Value::Array(arr) => EvalValue::Array(arr.iter().map(json_to_eval).collect()),
        serde_json::Value::Null => EvalValue::Null,
        serde_json::Value::Object(_) => EvalValue::Null, // objects not supported in expressions
    }
}

/// Convert a LiteralValue to EvalValue.
fn literal_to_eval(lit: &LiteralValue) -> EvalValue {
    match lit {
        LiteralValue::Bool(b) => EvalValue::Bool(*b),
        LiteralValue::String(s) => EvalValue::String(s.clone()),
        LiteralValue::Int(n) => EvalValue::Int(*n),
        LiteralValue::Float(f) => EvalValue::Float(*f),
        LiteralValue::Null => EvalValue::Null,
    }
}

// ---------------------------------------------------------------------------
// Expression evaluation
// ---------------------------------------------------------------------------

/// Evaluate a permission expression against auth context and optional row data.
///
/// For **read** queries, `row_data` is `None` and the expression is converted
/// to a SQL WHERE clause instead.
///
/// For **write** queries (create/update/delete), `row_data` contains the
/// row being operated on, and the expression is evaluated directly to
/// produce an allow/deny decision.
///
/// # Arguments
///
/// - `expr`: The permission expression to evaluate.
/// - `auth`: Resolved auth variables from the request context.
/// - `row_data`: Optional JSON object representing the row. For reads,
///   this is typically `None`.
pub fn evaluate_expr(
    expr: &PermExpr,
    auth: &AuthVars,
    row_data: Option<&serde_json::Value>,
) -> EvalValue {
    match expr {
        PermExpr::Full => EvalValue::Bool(true),
        PermExpr::None => EvalValue::Bool(false),

        PermExpr::Literal(lit) => literal_to_eval(lit),

        PermExpr::FieldRef { field } => row_data
            .and_then(|row| row.get(field))
            .map(json_to_eval)
            .unwrap_or(EvalValue::Null),

        PermExpr::AuthVar { path } => auth.resolve(path).unwrap_or(EvalValue::Null),

        PermExpr::Compare { lhs, op, rhs } => {
            let left = evaluate_expr(lhs, auth, row_data);
            let right = evaluate_expr(rhs, auth, row_data);

            // Special handling for role checks: if one side is an auth.role
            // reference (which is an array) and the other is a string, check
            // membership instead of equality.
            if *op == CompareOp::Eq {
                match (&left, &right) {
                    (EvalValue::Array(arr), scalar) | (scalar, EvalValue::Array(arr)) => {
                        return EvalValue::Bool(arr.contains(scalar));
                    }
                    _ => {}
                }
            }
            if *op == CompareOp::Ne {
                match (&left, &right) {
                    (EvalValue::Array(arr), scalar) | (scalar, EvalValue::Array(arr)) => {
                        return EvalValue::Bool(!arr.contains(scalar));
                    }
                    _ => {}
                }
            }

            EvalValue::Bool(left.compare(*op, &right))
        }

        PermExpr::And { left, right } => {
            let l = evaluate_expr(left, auth, row_data);
            if !l.is_truthy() {
                return EvalValue::Bool(false);
            }
            let r = evaluate_expr(right, auth, row_data);
            EvalValue::Bool(r.is_truthy())
        }

        PermExpr::Or { left, right } => {
            let l = evaluate_expr(left, auth, row_data);
            if l.is_truthy() {
                return EvalValue::Bool(true);
            }
            let r = evaluate_expr(right, auth, row_data);
            EvalValue::Bool(r.is_truthy())
        }

        PermExpr::Not { inner } => {
            let v = evaluate_expr(inner, auth, row_data);
            EvalValue::Bool(!v.is_truthy())
        }

        PermExpr::Contains { haystack, needle } => {
            let h = evaluate_expr(haystack, auth, row_data);
            let n = evaluate_expr(needle, auth, row_data);
            EvalValue::Bool(h.contains(&n))
        }
    }
}

// ---------------------------------------------------------------------------
// SQL WHERE clause generation
// ---------------------------------------------------------------------------

/// Convert a permission expression to a SQL WHERE clause fragment.
///
/// `$auth.*` references are replaced with bind parameter placeholders
/// (`$N`) and the corresponding values are collected into `bind_values`.
///
/// `param_offset` is the next available positional parameter number
/// (1-indexed).
///
/// # Returns
///
/// `(sql_fragment, bind_values)` — the caller should append the
/// fragment to the query's WHERE clause and add the bind values
/// to the query's parameter list.
pub fn expr_to_sql(
    expr: &PermExpr,
    auth: &AuthVars,
    param_offset: &mut usize,
) -> (String, Vec<String>) {
    match expr {
        PermExpr::Full => ("TRUE".to_string(), vec![]),
        PermExpr::None => ("FALSE".to_string(), vec![]),

        PermExpr::Literal(lit) => match lit {
            LiteralValue::Bool(b) => (b.to_string().to_uppercase(), vec![]),
            LiteralValue::String(s) => {
                let idx = *param_offset;
                *param_offset += 1;
                (format!("${idx}"), vec![s.clone()])
            }
            LiteralValue::Int(n) => (n.to_string(), vec![]),
            LiteralValue::Float(f) => (f.to_string(), vec![]),
            LiteralValue::Null => ("NULL".to_string(), vec![]),
        },

        PermExpr::FieldRef { field } => {
            // Validate field name to prevent SQL injection: only
            // alphanumeric and underscore allowed.
            let safe_field: String = field
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (format!("\"{safe_field}\""), vec![])
        }

        PermExpr::AuthVar { path } => {
            let idx = *param_offset;
            *param_offset += 1;
            let val = auth.resolve(path);
            let val_str = match val {
                Some(EvalValue::String(s)) => s,
                Some(EvalValue::Int(n)) => n.to_string(),
                Some(EvalValue::Float(f)) => f.to_string(),
                Some(EvalValue::Bool(b)) => b.to_string(),
                _ => return ("NULL".to_string(), vec![]),
            };
            (format!("${idx}"), vec![val_str])
        }

        PermExpr::Compare { lhs, op, rhs } => {
            let (left_sql, left_params) = expr_to_sql(lhs, auth, param_offset);
            let op_str = match op {
                CompareOp::Eq => "=",
                CompareOp::Ne => "!=",
                CompareOp::Lt => "<",
                CompareOp::Le => "<=",
                CompareOp::Gt => ">",
                CompareOp::Ge => ">=",
            };
            let (right_sql, right_params) = expr_to_sql(rhs, auth, param_offset);
            let mut params = left_params;
            params.extend(right_params);
            (format!("{left_sql} {op_str} {right_sql}"), params)
        }

        PermExpr::And { left, right } => {
            let (l_sql, l_params) = expr_to_sql(left, auth, param_offset);
            let (r_sql, r_params) = expr_to_sql(right, auth, param_offset);
            let mut params = l_params;
            params.extend(r_params);
            (format!("({l_sql} AND {r_sql})"), params)
        }

        PermExpr::Or { left, right } => {
            let (l_sql, l_params) = expr_to_sql(left, auth, param_offset);
            let (r_sql, r_params) = expr_to_sql(right, auth, param_offset);
            let mut params = l_params;
            params.extend(r_params);
            (format!("({l_sql} OR {r_sql})"), params)
        }

        PermExpr::Not { inner } => {
            let (inner_sql, params) = expr_to_sql(inner, auth, param_offset);
            (format!("NOT ({inner_sql})"), params)
        }

        PermExpr::Contains { haystack, needle } => {
            let (h_sql, h_params) = expr_to_sql(haystack, auth, param_offset);
            let (n_sql, n_params) = expr_to_sql(needle, auth, param_offset);
            let mut params = h_params;
            params.extend(n_params);
            (format!("{n_sql} = ANY({h_sql})"), params)
        }
    }
}

// ---------------------------------------------------------------------------
// Row-level security engine
// ---------------------------------------------------------------------------

/// The row-level security engine that holds table permission definitions
/// and evaluates them against request contexts.
///
/// Permissions are stored in-memory and can be loaded from the `_permissions`
/// system table or defined programmatically via the `DEFINE TABLE` DDL.
pub struct RowLevelSecurity {
    /// Map from table name to its permission definitions.
    tables: HashMap<String, TablePermissions>,
}

impl RowLevelSecurity {
    /// Create an empty RLS engine.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
        }
    }

    /// Register table permissions.
    pub fn define_table(&mut self, perms: TablePermissions) {
        self.tables.insert(perms.table.clone(), perms);
    }

    /// Remove a table's permission definitions.
    pub fn remove_table(&mut self, table: &str) {
        self.tables.remove(table);
    }

    /// Get the permission definition for a table.
    pub fn get_table(&self, table: &str) -> Option<&TablePermissions> {
        self.tables.get(table)
    }

    /// Check whether an operation is allowed on a specific row.
    ///
    /// For write operations (create/update/delete), evaluates the
    /// permission expression against the row data and returns a
    /// boolean allow/deny decision.
    ///
    /// # Arguments
    ///
    /// - `table`: The table name.
    /// - `op`: The operation being performed.
    /// - `auth`: The authenticated user context.
    /// - `row_data`: The row being operated on (JSON object).
    pub fn check_row(
        &self,
        table: &str,
        op: RowOp,
        ctx: &AuthContext,
        row_data: &serde_json::Value,
    ) -> Result<bool, AuthError> {
        let perms = self.tables.get(table);
        let expr = match perms {
            Some(p) => p.get(op),
            // No permissions defined for this table: deny by default.
            None => return Ok(false),
        };

        let auth_vars = AuthVars::from(ctx);
        let result = evaluate_expr(expr, &auth_vars, Some(row_data));
        Ok(result.is_truthy())
    }

    /// Generate a SQL WHERE clause for read queries on a table.
    ///
    /// The returned clause restricts rows to only those the user has
    /// permission to see. Returns `None` if the expression is `FULL`
    /// (no restriction needed).
    ///
    /// # Arguments
    ///
    /// - `table`: The table name.
    /// - `ctx`: The authenticated user context.
    /// - `param_offset`: The next available bind parameter number.
    ///
    /// # Returns
    ///
    /// `Some((sql_fragment, bind_values))` if filtering is needed,
    /// `None` if the user has full access.
    pub fn build_select_filter(
        &self,
        table: &str,
        ctx: &AuthContext,
        param_offset: usize,
    ) -> Result<Option<(String, Vec<String>)>, AuthError> {
        let perms = self.tables.get(table);
        let expr = match perms {
            Some(p) => p.get(RowOp::Select),
            // No permissions defined: deny all rows.
            None => return Ok(Some(("FALSE".to_string(), vec![]))),
        };

        match expr {
            PermExpr::Full => Ok(None), // No filter needed.
            PermExpr::None => Ok(Some(("FALSE".to_string(), vec![]))),
            _ => {
                let auth_vars = AuthVars::from(ctx);
                let mut offset = param_offset;
                let (sql, params) = expr_to_sql(expr, &auth_vars, &mut offset);
                Ok(Some((sql, params)))
            }
        }
    }

    /// Evaluate whether a batch of rows passes the permission check.
    ///
    /// Returns a vector of booleans, one per row, indicating whether
    /// each row is allowed.
    pub fn filter_rows(
        &self,
        table: &str,
        op: RowOp,
        ctx: &AuthContext,
        rows: &[serde_json::Value],
    ) -> Vec<bool> {
        let perms = self.tables.get(table);
        let expr = match perms {
            Some(p) => p.get(op),
            None => return vec![false; rows.len()],
        };

        let auth_vars = AuthVars::from(ctx);
        rows.iter()
            .map(|row| evaluate_expr(expr, &auth_vars, Some(row)).is_truthy())
            .collect()
    }
}

impl Default for RowLevelSecurity {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Expression builder helpers (SurrealDB-style DSL)
// ---------------------------------------------------------------------------

/// Builder helpers for constructing permission expressions in a readable
/// way that mirrors the SurrealDB DDL syntax.
pub mod expr {
    use super::*;

    /// Reference a field on the current row.
    pub fn field(name: &str) -> PermExpr {
        PermExpr::FieldRef {
            field: name.to_string(),
        }
    }

    /// Reference `$auth.id`.
    pub fn auth_id() -> PermExpr {
        PermExpr::AuthVar {
            path: "id".to_string(),
        }
    }

    /// Reference `$auth.role` (checks role membership).
    pub fn auth_role() -> PermExpr {
        PermExpr::AuthVar {
            path: "role".to_string(),
        }
    }

    /// Reference any `$auth.*` path.
    pub fn auth(path: &str) -> PermExpr {
        PermExpr::AuthVar {
            path: path.to_string(),
        }
    }

    /// A literal boolean value.
    pub fn bool_val(b: bool) -> PermExpr {
        PermExpr::Literal(LiteralValue::Bool(b))
    }

    /// A literal string value.
    pub fn str_val(s: &str) -> PermExpr {
        PermExpr::Literal(LiteralValue::String(s.to_string()))
    }

    /// A literal integer value.
    pub fn int_val(n: i64) -> PermExpr {
        PermExpr::Literal(LiteralValue::Int(n))
    }

    /// Equality comparison.
    pub fn eq(lhs: PermExpr, rhs: PermExpr) -> PermExpr {
        PermExpr::Compare {
            lhs: Box::new(lhs),
            op: CompareOp::Eq,
            rhs: Box::new(rhs),
        }
    }

    /// Inequality comparison.
    pub fn ne(lhs: PermExpr, rhs: PermExpr) -> PermExpr {
        PermExpr::Compare {
            lhs: Box::new(lhs),
            op: CompareOp::Ne,
            rhs: Box::new(rhs),
        }
    }

    /// Logical AND.
    pub fn and(left: PermExpr, right: PermExpr) -> PermExpr {
        PermExpr::And {
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Logical OR.
    pub fn or(left: PermExpr, right: PermExpr) -> PermExpr {
        PermExpr::Or {
            left: Box::new(left),
            right: Box::new(right),
        }
    }

    /// Logical NOT.
    pub fn not(inner: PermExpr) -> PermExpr {
        PermExpr::Not {
            inner: Box::new(inner),
        }
    }

    /// FULL access (always allowed).
    pub fn full() -> PermExpr {
        PermExpr::Full
    }

    /// NONE access (always denied).
    pub fn none() -> PermExpr {
        PermExpr::None
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::expr::*;
    use super::*;
    use uuid::Uuid;

    fn test_ctx(roles: &[&str]) -> AuthContext {
        AuthContext {
            user_id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            session_id: Uuid::new_v4(),
            roles: roles.iter().map(|r| r.to_string()).collect(),
            ip: "127.0.0.1".into(),
            user_agent: "test".into(),
            device_fingerprint: "test-fp".into(),
        }
    }

    fn test_auth_vars(ctx: &AuthContext) -> AuthVars {
        AuthVars::from(ctx)
    }

    // -- SurrealDB-style permission tests ------------------------------------

    #[test]
    fn surreal_style_select_published_or_owner() {
        // DEFINE TABLE posts PERMISSIONS
        //   FOR select WHERE published = true OR user = $auth.id
        let select_expr = or(
            eq(field("published"), bool_val(true)),
            eq(field("user"), auth_id()),
        );

        let ctx = test_ctx(&["user"]);
        let auth = test_auth_vars(&ctx);

        // Published post: anyone can see.
        let row = serde_json::json!({"published": true, "user": "other-user"});
        let result = evaluate_expr(&select_expr, &auth, Some(&row));
        assert!(result.is_truthy(), "published posts should be visible");

        // Unpublished but owned: visible.
        let row =
            serde_json::json!({"published": false, "user": "11111111-1111-1111-1111-111111111111"});
        let result = evaluate_expr(&select_expr, &auth, Some(&row));
        assert!(result.is_truthy(), "own posts should be visible");

        // Unpublished and not owned: hidden.
        let row = serde_json::json!({"published": false, "user": "other-user"});
        let result = evaluate_expr(&select_expr, &auth, Some(&row));
        assert!(
            !result.is_truthy(),
            "others' unpublished posts should be hidden"
        );
    }

    #[test]
    fn surreal_style_create_admin_only() {
        // FOR create WHERE $auth.role = "admin"
        let create_expr = eq(auth_role(), str_val("admin"));

        let admin_ctx = test_ctx(&["admin"]);
        let admin_auth = test_auth_vars(&admin_ctx);
        let result = evaluate_expr(&create_expr, &admin_auth, None);
        assert!(result.is_truthy(), "admin should be allowed to create");

        let user_ctx = test_ctx(&["user"]);
        let user_auth = test_auth_vars(&user_ctx);
        let result = evaluate_expr(&create_expr, &user_auth, None);
        assert!(
            !result.is_truthy(),
            "non-admin should not be allowed to create"
        );
    }

    #[test]
    fn surreal_style_update_own_only() {
        // FOR update WHERE user = $auth.id
        let update_expr = eq(field("user"), auth_id());

        let ctx = test_ctx(&["user"]);
        let auth = test_auth_vars(&ctx);

        let own_row = serde_json::json!({"user": "11111111-1111-1111-1111-111111111111"});
        assert!(evaluate_expr(&update_expr, &auth, Some(&own_row)).is_truthy());

        let other_row = serde_json::json!({"user": "22222222-2222-2222-2222-222222222222"});
        assert!(!evaluate_expr(&update_expr, &auth, Some(&other_row)).is_truthy());
    }

    #[test]
    fn full_and_none_expressions() {
        let ctx = test_ctx(&[]);
        let auth = test_auth_vars(&ctx);

        assert!(evaluate_expr(&PermExpr::Full, &auth, None).is_truthy());
        assert!(!evaluate_expr(&PermExpr::None, &auth, None).is_truthy());
    }

    #[test]
    fn not_expression() {
        let ctx = test_ctx(&["admin"]);
        let auth = test_auth_vars(&ctx);
        let expr = not(eq(auth_role(), str_val("banned")));
        assert!(evaluate_expr(&expr, &auth, None).is_truthy());
    }

    #[test]
    fn contains_expression() {
        let ctx = test_ctx(&["editor", "reviewer"]);
        let auth_vars = test_auth_vars(&ctx);

        let expr = PermExpr::Contains {
            haystack: Box::new(auth("roles")),
            needle: Box::new(str_val("editor")),
        };
        assert!(evaluate_expr(&expr, &auth_vars, None).is_truthy());

        let expr = PermExpr::Contains {
            haystack: Box::new(auth("roles")),
            needle: Box::new(str_val("admin")),
        };
        assert!(!evaluate_expr(&expr, &auth_vars, None).is_truthy());
    }

    // -- RLS engine tests ----------------------------------------------------

    #[test]
    fn rls_engine_check_row() {
        let mut rls = RowLevelSecurity::new();

        let posts = TablePermissions::new("posts")
            .with(RowOp::Select, full())
            .with(RowOp::Create, eq(auth_role(), str_val("admin")))
            .with(RowOp::Update, eq(field("user"), auth_id()))
            .with(RowOp::Delete, eq(auth_role(), str_val("admin")));
        rls.define_table(posts);

        let admin_ctx = test_ctx(&["admin"]);
        let user_ctx = test_ctx(&["user"]);

        // Admin can create.
        let row = serde_json::json!({});
        assert!(
            rls.check_row("posts", RowOp::Create, &admin_ctx, &row)
                .unwrap()
        );

        // User cannot create.
        assert!(
            !rls.check_row("posts", RowOp::Create, &user_ctx, &row)
                .unwrap()
        );

        // User can update own row.
        let own_row = serde_json::json!({"user": "11111111-1111-1111-1111-111111111111"});
        assert!(
            rls.check_row("posts", RowOp::Update, &user_ctx, &own_row)
                .unwrap()
        );

        // User cannot update others' row.
        let other_row = serde_json::json!({"user": "other"});
        assert!(
            !rls.check_row("posts", RowOp::Update, &user_ctx, &other_row)
                .unwrap()
        );
    }

    #[test]
    fn rls_engine_undefined_table_denies() {
        let rls = RowLevelSecurity::new();
        let ctx = test_ctx(&["admin"]);
        let row = serde_json::json!({});
        assert!(
            !rls.check_row("nonexistent", RowOp::Select, &ctx, &row)
                .unwrap()
        );
    }

    #[test]
    fn rls_engine_undefined_op_denies() {
        let mut rls = RowLevelSecurity::new();
        let perms = TablePermissions::new("posts").with(RowOp::Select, full());
        rls.define_table(perms);

        let ctx = test_ctx(&["admin"]);
        let row = serde_json::json!({});
        // Create is not defined, so it should deny.
        assert!(!rls.check_row("posts", RowOp::Create, &ctx, &row).unwrap());
    }

    #[test]
    fn rls_build_select_filter() {
        let mut rls = RowLevelSecurity::new();
        let posts = TablePermissions::new("posts").with(
            RowOp::Select,
            or(
                eq(field("published"), bool_val(true)),
                eq(field("user_id"), auth_id()),
            ),
        );
        rls.define_table(posts);

        let ctx = test_ctx(&["user"]);
        let result = rls.build_select_filter("posts", &ctx, 1).unwrap();
        assert!(result.is_some());

        let (sql, params) = result.unwrap();
        assert!(sql.contains("OR"), "should contain OR clause: {sql}");
        assert!(!params.is_empty(), "should have bind params");
    }

    #[test]
    fn rls_build_select_filter_full_access() {
        let mut rls = RowLevelSecurity::new();
        let posts = TablePermissions::new("public_data").with(RowOp::Select, full());
        rls.define_table(posts);

        let ctx = test_ctx(&[]);
        let result = rls.build_select_filter("public_data", &ctx, 1).unwrap();
        assert!(result.is_none(), "FULL should produce no filter");
    }

    #[test]
    fn rls_filter_rows_batch() {
        let mut rls = RowLevelSecurity::new();
        let posts = TablePermissions::new("posts")
            .with(RowOp::Select, eq(field("published"), bool_val(true)));
        rls.define_table(posts);

        let ctx = test_ctx(&["user"]);
        let rows = vec![
            serde_json::json!({"published": true, "title": "Public"}),
            serde_json::json!({"published": false, "title": "Draft"}),
            serde_json::json!({"published": true, "title": "Also Public"}),
        ];

        let allowed = rls.filter_rows("posts", RowOp::Select, &ctx, &rows);
        assert_eq!(allowed, vec![true, false, true]);
    }

    // -- SQL generation tests ------------------------------------------------

    #[test]
    fn expr_to_sql_simple_eq() {
        let ctx = test_ctx(&["user"]);
        let auth = test_auth_vars(&ctx);
        let expr = eq(field("user_id"), auth_id());
        let mut offset = 1;
        let (sql, params) = expr_to_sql(&expr, &auth, &mut offset);
        assert_eq!(sql, "\"user_id\" = $1");
        assert_eq!(params, vec!["11111111-1111-1111-1111-111111111111"]);
    }

    #[test]
    fn expr_to_sql_or_expression() {
        let ctx = test_ctx(&["user"]);
        let auth = test_auth_vars(&ctx);
        let expr = or(
            eq(field("published"), bool_val(true)),
            eq(field("user_id"), auth_id()),
        );
        let mut offset = 1;
        let (sql, params) = expr_to_sql(&expr, &auth, &mut offset);
        assert!(sql.contains("OR"));
        assert!(sql.contains("\"published\""));
        assert_eq!(params.len(), 1); // only auth_id is parameterized
    }

    #[test]
    fn expr_to_sql_not_expression() {
        let ctx = test_ctx(&[]);
        let auth = test_auth_vars(&ctx);
        let expr = not(eq(field("deleted"), bool_val(true)));
        let mut offset = 1;
        let (sql, _) = expr_to_sql(&expr, &auth, &mut offset);
        assert!(sql.starts_with("NOT"));
    }

    #[test]
    fn null_comparisons_fail_closed() {
        let ctx = test_ctx(&[]);
        let auth = test_auth_vars(&ctx);

        // Comparing a missing field (NULL) to a value should be false.
        let expr = eq(field("missing_field"), str_val("something"));
        let result = evaluate_expr(&expr, &auth, Some(&serde_json::json!({})));
        assert!(!result.is_truthy(), "NULL comparison must fail closed");
    }

    #[test]
    fn custom_claims_in_auth_vars() {
        let ctx = test_ctx(&[]);
        let mut auth_vars = AuthVars::from(&ctx);
        auth_vars.custom_claims.insert(
            "org_id".to_string(),
            serde_json::Value::String("org-42".to_string()),
        );

        let expr = eq(field("org_id"), auth("org_id"));
        let row = serde_json::json!({"org_id": "org-42"});
        let result = evaluate_expr(&expr, &auth_vars, Some(&row));
        assert!(result.is_truthy());

        let row = serde_json::json!({"org_id": "org-99"});
        let result = evaluate_expr(&expr, &auth_vars, Some(&row));
        assert!(!result.is_truthy());
    }

    #[test]
    fn field_ref_sanitization_in_sql() {
        let ctx = test_ctx(&[]);
        let auth = test_auth_vars(&ctx);
        // Attempt SQL injection via field name.
        let expr = PermExpr::FieldRef {
            field: "name; DROP TABLE--".to_string(),
        };
        let mut offset = 1;
        let (sql, _) = expr_to_sql(&expr, &auth, &mut offset);
        assert!(!sql.contains(';'), "field name must be sanitized: {sql}");
        assert!(
            !sql.contains("DROP"),
            "field name must strip dangerous chars: {sql}"
        );
    }

    #[test]
    fn eval_value_truthiness() {
        assert!(EvalValue::Bool(true).is_truthy());
        assert!(!EvalValue::Bool(false).is_truthy());
        assert!(!EvalValue::Null.is_truthy());
        assert!(EvalValue::String("hello".into()).is_truthy());
        assert!(!EvalValue::String("".into()).is_truthy());
        assert!(EvalValue::Int(1).is_truthy());
        assert!(!EvalValue::Int(0).is_truthy());
        assert!(EvalValue::Array(vec![EvalValue::Int(1)]).is_truthy());
        assert!(!EvalValue::Array(vec![]).is_truthy());
    }

    #[test]
    fn comparison_operators() {
        assert!(EvalValue::Int(5).compare(CompareOp::Lt, &EvalValue::Int(10)));
        assert!(EvalValue::Int(10).compare(CompareOp::Ge, &EvalValue::Int(10)));
        assert!(!EvalValue::Int(10).compare(CompareOp::Lt, &EvalValue::Int(5)));
        assert!(
            EvalValue::String("a".into()).compare(CompareOp::Lt, &EvalValue::String("b".into()))
        );
    }

    #[test]
    fn table_permissions_builder() {
        let perms = TablePermissions::new("users")
            .with(
                RowOp::Select,
                or(
                    eq(field("published"), bool_val(true)),
                    eq(field("user"), auth_id()),
                ),
            )
            .with(RowOp::Create, eq(auth_role(), str_val("admin")))
            .with(RowOp::Update, eq(field("user"), auth_id()))
            .with(RowOp::Delete, eq(auth_role(), str_val("admin")));

        assert!(matches!(perms.get(RowOp::Select), PermExpr::Or { .. }));
        assert!(matches!(perms.get(RowOp::Create), PermExpr::Compare { .. }));
    }
}
