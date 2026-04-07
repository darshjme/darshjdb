//! Edge types and storage for the DarshJDB graph engine.
//!
//! Edges represent directed relationships between record IDs in the
//! SurrealDB-style `table:id` format. Each edge is a first-class citizen
//! stored in the `_edges` PostgreSQL table with optional JSONB metadata.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DarshJError, Result};

// ── Record ID ──────────────────────────────────────────────────────

/// A SurrealDB-style record identifier in `table:id` format.
///
/// Examples: `user:darsh`, `company:knowai`, `post:abc123`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecordId {
    /// The table (entity type) this record belongs to.
    pub table: String,
    /// The unique identifier within the table.
    pub id: String,
}

impl RecordId {
    /// Create a new record ID from table and id components.
    pub fn new(table: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            id: id.into(),
        }
    }

    /// Parse a `table:id` string into a [`RecordId`].
    ///
    /// Returns an error if the string does not contain exactly one colon
    /// or if either component is empty.
    pub fn parse(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(DarshJError::InvalidQuery(format!(
                "invalid record ID '{s}': expected 'table:id' format"
            )));
        }
        Ok(Self {
            table: parts[0].to_string(),
            id: parts[1].to_string(),
        })
    }

    /// Format as the canonical `table:id` string.
    pub fn to_string_repr(&self) -> String {
        format!("{}:{}", self.table, self.id)
    }
}

impl std::fmt::Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.table, self.id)
    }
}

// ── Edge ───────────────────────────────────────────────────────────

/// A directed edge between two record IDs.
///
/// Models the SurrealDB `RELATE` pattern:
/// `RELATE user:darsh->works_at->company:knowai`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    /// Auto-generated primary key.
    pub id: Uuid,
    /// Source table name.
    pub from_table: String,
    /// Source record ID within the table.
    pub from_id: String,
    /// Edge type / relationship label (e.g. `works_at`, `follows`).
    pub edge_type: String,
    /// Target table name.
    pub to_table: String,
    /// Target record ID within the table.
    pub to_id: String,
    /// Optional metadata attached to the edge.
    pub data: Option<serde_json::Value>,
    /// When the edge was created.
    pub created_at: DateTime<Utc>,
}

impl Edge {
    /// The source as a [`RecordId`].
    pub fn from_record(&self) -> RecordId {
        RecordId::new(&self.from_table, &self.from_id)
    }

    /// The target as a [`RecordId`].
    pub fn to_record(&self) -> RecordId {
        RecordId::new(&self.to_table, &self.to_id)
    }
}

// ── Edge input ─────────────────────────────────────────────────────

/// Input for creating a new edge (before server-side id/timestamp assignment).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeInput {
    /// Source record in `table:id` format.
    pub from: String,
    /// Edge type / relationship label.
    pub edge_type: String,
    /// Target record in `table:id` format.
    pub to: String,
    /// Optional JSONB metadata for the edge.
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl EdgeInput {
    /// Validate the input, returning a descriptive error if anything is wrong.
    pub fn validate(&self) -> Result<()> {
        RecordId::parse(&self.from)?;
        RecordId::parse(&self.to)?;
        if self.edge_type.is_empty() {
            return Err(DarshJError::InvalidAttribute(
                "edge_type must not be empty".into(),
            ));
        }
        if self.edge_type.len() > 256 {
            return Err(DarshJError::InvalidAttribute(format!(
                "edge_type exceeds 256 bytes: {} bytes",
                self.edge_type.len()
            )));
        }
        Ok(())
    }
}

// ── Direction ──────────────────────────────────────────────────────

/// Direction for graph traversal queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Follow outgoing edges (`->`).
    Out,
    /// Follow incoming edges (`<-`).
    In,
    /// Follow edges in either direction.
    Both,
}

// ── Edge store ─────────────────────────────────────────────────────

/// Postgres-backed edge storage layer.
///
/// All edges live in the `_edges` table with proper indexes for
/// both forward and reverse traversal.
#[derive(Clone)]
pub struct PgEdgeStore {
    pool: PgPool,
}

impl PgEdgeStore {
    /// Create a new edge store and ensure the schema exists.
    pub async fn new(pool: PgPool) -> Result<Self> {
        let store = Self { pool };
        store.ensure_schema().await?;
        Ok(store)
    }

    /// Create a store without running migrations (lazy init).
    pub fn new_lazy(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return a reference to the underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Create the `_edges` table and supporting indexes if they do not
    /// already exist. This is idempotent.
    async fn ensure_schema(&self) -> Result<()> {
        sqlx::raw_sql(
            r#"
            CREATE TABLE IF NOT EXISTS _edges (
                id          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
                from_table  TEXT        NOT NULL,
                from_id     TEXT        NOT NULL,
                edge_type   TEXT        NOT NULL,
                to_table    TEXT        NOT NULL,
                to_id       TEXT        NOT NULL,
                data        JSONB,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            );

            -- Forward traversal: from a node, find outgoing edges of a type.
            CREATE INDEX IF NOT EXISTS idx_edges_from
                ON _edges (from_table, from_id, edge_type);

            -- Reverse traversal: from a node, find incoming edges of a type.
            CREATE INDEX IF NOT EXISTS idx_edges_to
                ON _edges (to_table, to_id, edge_type);

            -- Edge type scan for schema discovery.
            CREATE INDEX IF NOT EXISTS idx_edges_type
                ON _edges (edge_type);

            -- Unique constraint to prevent duplicate edges (same from, type, to).
            CREATE UNIQUE INDEX IF NOT EXISTS idx_edges_unique
                ON _edges (from_table, from_id, edge_type, to_table, to_id);
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    /// Create a new edge. Returns the created [`Edge`].
    ///
    /// Implements the SurrealDB `RELATE` semantics:
    /// `RELATE user:darsh->works_at->company:knowai`
    pub async fn relate(&self, input: &EdgeInput) -> Result<Edge> {
        input.validate()?;

        let from = RecordId::parse(&input.from)?;
        let to = RecordId::parse(&input.to)?;
        let id = Uuid::new_v4();

        let row = sqlx::query_as::<_, (Uuid, String, String, String, String, String, Option<serde_json::Value>, DateTime<Utc>)>(
            r#"
            INSERT INTO _edges (id, from_table, from_id, edge_type, to_table, to_id, data)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (from_table, from_id, edge_type, to_table, to_id) DO UPDATE
                SET data = EXCLUDED.data
            RETURNING id, from_table, from_id, edge_type, to_table, to_id, data, created_at
            "#,
        )
        .bind(id)
        .bind(&from.table)
        .bind(&from.id)
        .bind(&input.edge_type)
        .bind(&to.table)
        .bind(&to.id)
        .bind(&input.data)
        .fetch_one(&self.pool)
        .await?;

        Ok(Edge {
            id: row.0,
            from_table: row.1,
            from_id: row.2,
            edge_type: row.3,
            to_table: row.4,
            to_id: row.5,
            data: row.6,
            created_at: row.7,
        })
    }

    /// Delete an edge by its UUID. Returns true if the edge existed.
    pub async fn delete_edge(&self, edge_id: Uuid) -> Result<bool> {
        let result = sqlx::query("DELETE FROM _edges WHERE id = $1")
            .bind(edge_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete all edges matching the from->edge_type->to pattern.
    pub async fn delete_relate(
        &self,
        from: &RecordId,
        edge_type: &str,
        to: &RecordId,
    ) -> Result<bool> {
        let result = sqlx::query(
            r#"
            DELETE FROM _edges
            WHERE from_table = $1 AND from_id = $2
              AND edge_type = $3
              AND to_table = $4 AND to_id = $5
            "#,
        )
        .bind(&from.table)
        .bind(&from.id)
        .bind(edge_type)
        .bind(&to.table)
        .bind(&to.id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Get outgoing edges from a record, optionally filtered by edge type.
    ///
    /// Models: `SELECT ->edge_type->? FROM table:id`
    pub async fn get_outgoing(
        &self,
        from: &RecordId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Edge>> {
        let rows = if let Some(et) = edge_type {
            sqlx::query_as::<_, (Uuid, String, String, String, String, String, Option<serde_json::Value>, DateTime<Utc>)>(
                r#"
                SELECT id, from_table, from_id, edge_type, to_table, to_id, data, created_at
                FROM _edges
                WHERE from_table = $1 AND from_id = $2 AND edge_type = $3
                ORDER BY created_at
                "#,
            )
            .bind(&from.table)
            .bind(&from.id)
            .bind(et)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, String, String, String, String, String, Option<serde_json::Value>, DateTime<Utc>)>(
                r#"
                SELECT id, from_table, from_id, edge_type, to_table, to_id, data, created_at
                FROM _edges
                WHERE from_table = $1 AND from_id = $2
                ORDER BY created_at
                "#,
            )
            .bind(&from.table)
            .bind(&from.id)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.into_iter().map(|r| Edge {
            id: r.0,
            from_table: r.1,
            from_id: r.2,
            edge_type: r.3,
            to_table: r.4,
            to_id: r.5,
            data: r.6,
            created_at: r.7,
        }).collect())
    }

    /// Get incoming edges to a record, optionally filtered by edge type.
    ///
    /// Models: `SELECT <-edge_type<-? FROM table:id`
    pub async fn get_incoming(
        &self,
        to: &RecordId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Edge>> {
        let rows = if let Some(et) = edge_type {
            sqlx::query_as::<_, (Uuid, String, String, String, String, String, Option<serde_json::Value>, DateTime<Utc>)>(
                r#"
                SELECT id, from_table, from_id, edge_type, to_table, to_id, data, created_at
                FROM _edges
                WHERE to_table = $1 AND to_id = $2 AND edge_type = $3
                ORDER BY created_at
                "#,
            )
            .bind(&to.table)
            .bind(&to.id)
            .bind(et)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, String, String, String, String, String, Option<serde_json::Value>, DateTime<Utc>)>(
                r#"
                SELECT id, from_table, from_id, edge_type, to_table, to_id, data, created_at
                FROM _edges
                WHERE to_table = $1 AND to_id = $2
                ORDER BY created_at
                "#,
            )
            .bind(&to.table)
            .bind(&to.id)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.into_iter().map(|r| Edge {
            id: r.0,
            from_table: r.1,
            from_id: r.2,
            edge_type: r.3,
            to_table: r.4,
            to_id: r.5,
            data: r.6,
            created_at: r.7,
        }).collect())
    }

    /// Get all neighbors (both directions) of a record, optionally filtered
    /// by edge type.
    pub async fn get_neighbors(
        &self,
        record: &RecordId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Edge>> {
        let rows = if let Some(et) = edge_type {
            sqlx::query_as::<_, (Uuid, String, String, String, String, String, Option<serde_json::Value>, DateTime<Utc>)>(
                r#"
                SELECT id, from_table, from_id, edge_type, to_table, to_id, data, created_at
                FROM _edges
                WHERE ((from_table = $1 AND from_id = $2) OR (to_table = $1 AND to_id = $2))
                  AND edge_type = $3
                ORDER BY created_at
                "#,
            )
            .bind(&record.table)
            .bind(&record.id)
            .bind(et)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, (Uuid, String, String, String, String, String, Option<serde_json::Value>, DateTime<Utc>)>(
                r#"
                SELECT id, from_table, from_id, edge_type, to_table, to_id, data, created_at
                FROM _edges
                WHERE (from_table = $1 AND from_id = $2) OR (to_table = $1 AND to_id = $2)
                ORDER BY created_at
                "#,
            )
            .bind(&record.table)
            .bind(&record.id)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.into_iter().map(|r| Edge {
            id: r.0,
            from_table: r.1,
            from_id: r.2,
            edge_type: r.3,
            to_table: r.4,
            to_id: r.5,
            data: r.6,
            created_at: r.7,
        }).collect())
    }

    /// Multi-hop traversal: follow a chain of edge types from a starting record.
    ///
    /// For example, `traverse(user:darsh, [("works_at", Out), ("located_in", Out)])`
    /// returns all records reachable via `user:darsh->works_at->?->located_in->?`.
    pub async fn traverse_path(
        &self,
        start: &RecordId,
        hops: &[(String, Direction)],
    ) -> Result<Vec<RecordId>> {
        let mut current_set = vec![start.clone()];

        for (edge_type, direction) in hops {
            let mut next_set = Vec::new();
            for record in &current_set {
                let edges = match direction {
                    Direction::Out => self.get_outgoing(record, Some(edge_type)).await?,
                    Direction::In => self.get_incoming(record, Some(edge_type)).await?,
                    Direction::Both => self.get_neighbors(record, Some(edge_type)).await?,
                };
                for edge in edges {
                    let target = match direction {
                        Direction::Out => edge.to_record(),
                        Direction::In => edge.from_record(),
                        Direction::Both => {
                            if edge.from_table == record.table && edge.from_id == record.id {
                                edge.to_record()
                            } else {
                                edge.from_record()
                            }
                        }
                    };
                    if !next_set.contains(&target) {
                        next_set.push(target);
                    }
                }
            }
            current_set = next_set;
        }

        Ok(current_set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RecordId ───────────────────────────────────────────────────

    #[test]
    fn record_id_parse_valid() {
        let rid = RecordId::parse("user:darsh").unwrap();
        assert_eq!(rid.table, "user");
        assert_eq!(rid.id, "darsh");
    }

    #[test]
    fn record_id_parse_with_colons_in_id() {
        let rid = RecordId::parse("ns:some:complex:id").unwrap();
        assert_eq!(rid.table, "ns");
        assert_eq!(rid.id, "some:complex:id");
    }

    #[test]
    fn record_id_parse_empty_table_fails() {
        assert!(RecordId::parse(":darsh").is_err());
    }

    #[test]
    fn record_id_parse_empty_id_fails() {
        assert!(RecordId::parse("user:").is_err());
    }

    #[test]
    fn record_id_parse_no_colon_fails() {
        assert!(RecordId::parse("user").is_err());
    }

    #[test]
    fn record_id_display() {
        let rid = RecordId::new("company", "knowai");
        assert_eq!(format!("{rid}"), "company:knowai");
        assert_eq!(rid.to_string_repr(), "company:knowai");
    }

    #[test]
    fn record_id_equality() {
        let a = RecordId::new("user", "darsh");
        let b = RecordId::parse("user:darsh").unwrap();
        assert_eq!(a, b);
    }

    // ── EdgeInput validation ───────────────────────────────────────

    #[test]
    fn edge_input_valid() {
        let input = EdgeInput {
            from: "user:darsh".into(),
            edge_type: "works_at".into(),
            to: "company:knowai".into(),
            data: None,
        };
        assert!(input.validate().is_ok());
    }

    #[test]
    fn edge_input_empty_edge_type_fails() {
        let input = EdgeInput {
            from: "user:darsh".into(),
            edge_type: "".into(),
            to: "company:knowai".into(),
            data: None,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn edge_input_invalid_from_fails() {
        let input = EdgeInput {
            from: "invalid".into(),
            edge_type: "works_at".into(),
            to: "company:knowai".into(),
            data: None,
        };
        assert!(input.validate().is_err());
    }

    #[test]
    fn edge_input_invalid_to_fails() {
        let input = EdgeInput {
            from: "user:darsh".into(),
            edge_type: "works_at".into(),
            to: "invalid".into(),
            data: None,
        };
        assert!(input.validate().is_err());
    }

    // ── Edge helpers ───────────────────────────────────────────────

    #[test]
    fn edge_record_accessors() {
        let edge = Edge {
            id: Uuid::new_v4(),
            from_table: "user".into(),
            from_id: "darsh".into(),
            edge_type: "works_at".into(),
            to_table: "company".into(),
            to_id: "knowai".into(),
            data: None,
            created_at: Utc::now(),
        };
        assert_eq!(edge.from_record(), RecordId::new("user", "darsh"));
        assert_eq!(edge.to_record(), RecordId::new("company", "knowai"));
    }

    // ── Direction serialization ────────────────────────────────────

    #[test]
    fn direction_serialization_roundtrip() {
        let d = Direction::Out;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"out\"");
        let back: Direction = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
    }
}
