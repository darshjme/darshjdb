//! Abstract Syntax Tree for DarshQL — a SurrealQL-inspired query language for DarshJDB.
//!
//! DarshQL supports SQL-like syntax with graph traversal, record links,
//! RELATE statements, LIVE SELECT, type casting, and computed fields.

use serde::{Deserialize, Serialize};

// ── Top-level statement ────────────────────────────────────────────

/// A single DarshQL statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Statement {
    Select(SelectStatement),
    Create(CreateStatement),
    Update(UpdateStatement),
    Delete(DeleteStatement),
    Insert(InsertStatement),
    Relate(RelateStatement),
    LiveSelect(LiveSelectStatement),
    DefineTable(DefineTableStatement),
    DefineField(DefineFieldStatement),
    InfoFor(InfoForStatement),
}

// ── SELECT ─────────────────────────────────────────────────────────

/// `SELECT fields FROM target [WHERE ...] [ORDER BY ...] [LIMIT n] [START n]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectStatement {
    /// Fields to project (may include `*`, graph traversals, type casts, computed).
    pub fields: Vec<Field>,
    /// The source table or record id.
    pub from: Target,
    /// Optional WHERE clause.
    pub condition: Option<Expr>,
    /// ORDER BY clauses.
    pub order: Vec<OrderBy>,
    /// LIMIT clause.
    pub limit: Option<u64>,
    /// START (offset) clause.
    pub start: Option<u64>,
    /// GROUP BY attributes.
    pub group_by: Vec<String>,
    /// FETCH clauses for eager-loading linked records.
    pub fetch: Vec<String>,
}

// ── CREATE ─────────────────────────────────────────────────────────

/// `CREATE target [SET field = value, ...] [CONTENT {...}]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateStatement {
    pub target: Target,
    pub data: SetOrContent,
}

// ── UPDATE ─────────────────────────────────────────────────────────

/// `UPDATE target SET field = value, ... [WHERE ...]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatement {
    pub target: Target,
    pub data: SetOrContent,
    pub condition: Option<Expr>,
}

// ── DELETE ─────────────────────────────────────────────────────────

/// `DELETE target [WHERE ...]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteStatement {
    pub target: Target,
    pub condition: Option<Expr>,
}

// ── INSERT ─────────────────────────────────────────────────────────

/// `INSERT INTO table (fields) VALUES (values), ...`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertStatement {
    pub table: String,
    pub fields: Vec<String>,
    pub values: Vec<Vec<Expr>>,
}

// ── RELATE ─────────────────────────────────────────────────────────

/// `RELATE from->edge->to [SET field = value, ...]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelateStatement {
    pub from: RecordId,
    pub edge: String,
    pub to: RecordId,
    pub data: Option<SetOrContent>,
}

// ── LIVE SELECT ────────────────────────────────────────────────────

/// `LIVE SELECT fields FROM target [WHERE ...]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSelectStatement {
    pub fields: Vec<Field>,
    pub from: Target,
    pub condition: Option<Expr>,
}

// ── DEFINE TABLE ───────────────────────────────────────────────────

/// `DEFINE TABLE name [SCHEMAFULL | SCHEMALESS] [DROP]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefineTableStatement {
    pub name: String,
    pub schema_mode: SchemaMode,
    pub drop: bool,
}

/// Whether a table enforces a strict schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SchemaMode {
    #[default]
    Schemaless,
    Schemafull,
}

// ── DEFINE FIELD ───────────────────────────────────────────────────

/// `DEFINE FIELD name ON TABLE table TYPE type [DEFAULT value] [ASSERT expr]`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DefineFieldStatement {
    pub name: String,
    pub table: String,
    pub field_type: Option<DarshType>,
    pub default: Option<Expr>,
    pub assert: Option<Expr>,
}

// ── INFO FOR ───────────────────────────────────────────────────────

/// `INFO FOR DB | INFO FOR TABLE name`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfoForStatement {
    pub target: InfoTarget,
}

/// What to describe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InfoTarget {
    Db,
    Table(String),
}

// ── Shared types ───────────────────────────────────────────────────

/// Data payload for CREATE / UPDATE: either `SET k = v, ...` or `CONTENT {...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SetOrContent {
    Set(Vec<(String, Expr)>),
    Content(serde_json::Value),
}

/// A record identifier: `table:id` (e.g., `user:darsh`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordId {
    pub table: String,
    pub id: String,
}

impl std::fmt::Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.table, self.id)
    }
}

/// A query target — either a table name or a specific record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Target {
    /// Just a table name: `users`.
    Table(String),
    /// A specific record: `user:darsh`.
    Record(RecordId),
}

impl Target {
    /// Extract the table name regardless of variant.
    pub fn table_name(&self) -> &str {
        match self {
            Target::Table(t) => t,
            Target::Record(r) => &r.table,
        }
    }
}

/// A projected field in a SELECT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Field {
    /// `*` — all fields.
    All,
    /// A plain attribute: `name`.
    Attribute(String),
    /// A type-casted field: `<int>age`.
    Cast {
        cast_type: DarshType,
        expr: Box<Field>,
    },
    /// A graph traversal: `->friends->friends`.
    Graph(GraphTraversal),
    /// A computed expression with alias: `count(->posts) AS post_count`.
    Computed {
        func: String,
        args: Vec<Field>,
        alias: String,
    },
}

/// A multi-hop graph traversal: `->edge1->edge2` or `<-edge` (inbound).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTraversal {
    /// Each step: direction + edge name.
    pub steps: Vec<GraphStep>,
}

/// A single hop in a graph traversal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphStep {
    /// The edge table name (e.g. `friends`, `works_at`).
    pub edge: String,
    /// Direction of traversal.
    pub direction: EdgeDirection,
}

/// Edge direction in graph traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeDirection {
    /// `->` outbound.
    Out,
    /// `<-` inbound.
    In,
}

/// DarshQL type names for casting: `<int>`, `<string>`, `<float>`, `<bool>`, `<datetime>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DarshType {
    Int,
    Float,
    String,
    Bool,
    Datetime,
    Record(String),
    Array,
    Object,
    Any,
}

impl DarshType {
    /// Parse a type name string into a DarshType.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "int" | "integer" => Some(Self::Int),
            "float" | "number" | "decimal" => Some(Self::Float),
            "string" | "text" => Some(Self::String),
            "bool" | "boolean" => Some(Self::Bool),
            "datetime" | "timestamp" => Some(Self::Datetime),
            "array" => Some(Self::Array),
            "object" => Some(Self::Object),
            "any" => Some(Self::Any),
            s if s.starts_with("record") => {
                let inner = s.strip_prefix("record(")?.strip_suffix(')')?;
                Some(Self::Record(inner.to_string()))
            }
            _ => None,
        }
    }
}

/// ORDER BY clause.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBy {
    pub field: String,
    pub direction: SortDir,
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

/// Expression node for WHERE clauses, SET values, and assertions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    /// A literal value.
    Value(serde_json::Value),
    /// A record link: `company:knowai`.
    RecordLink(RecordId),
    /// A column / attribute reference.
    Ident(String),
    /// A binary comparison: `age > 18`.
    BinaryOp {
        left: Box<Expr>,
        op: BinOp,
        right: Box<Expr>,
    },
    /// A logical AND/OR.
    LogicalOp {
        left: Box<Expr>,
        op: LogicOp,
        right: Box<Expr>,
    },
    /// A function call: `count(->posts)`.
    FnCall { name: String, args: Vec<Expr> },
    /// A graph traversal expression (used inside functions).
    GraphExpr(GraphTraversal),
    /// A type cast expression: `<int>field`.
    Cast {
        cast_type: DarshType,
        expr: Box<Expr>,
    },
    /// Parenthesized expression.
    Paren(Box<Expr>),
}

/// Binary comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    Contains,
    Is,
    IsNot,
}

/// Logical operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogicOp {
    And,
    Or,
}
