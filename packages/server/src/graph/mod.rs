//! Graph database engine for DarshJDB.
//!
//! Builds on the existing triple store to add SurrealDB-style graph
//! capabilities: record IDs (`table:id`), directed edges (`RELATE`),
//! and multi-hop traversal (BFS, DFS, shortest path).
//!
//! # Architecture
//!
//! ```text
//! ┌──────────────┐     ┌──────────────┐     ┌──────────────────┐
//! │  REST API    │────▶│  GraphEngine │────▶│  PgEdgeStore     │
//! │  /graph/*    │     │  (facade)    │     │  (_edges table)  │
//! └──────────────┘     └──────────────┘     └──────────────────┘
//!                            │
//!                            ▼
//!                      ┌──────────────────┐
//!                      │ TraversalEngine  │
//!                      │ BFS / DFS / SP   │
//!                      └──────────────────┘
//! ```
//!
//! # Record IDs
//!
//! Every node is addressed by a `table:id` string (e.g. `user:darsh`,
//! `company:knowai`). This mirrors SurrealDB's record-link system and
//! integrates naturally with the triple store's entity model.
//!
//! # Edge Storage
//!
//! Edges are persisted in PostgreSQL's `_edges` table with indexes for
//! both forward and reverse traversal. Edge metadata is stored as JSONB.

pub mod edge;
pub mod traverse;

pub use edge::{Direction, Edge, EdgeInput, PgEdgeStore, RecordId};
pub use traverse::{
    ShortestPathResult, TraversalAlgorithm, TraversalConfig, TraversalEngine, TraversalNode,
    TraversalResult,
};

use std::sync::Arc;

use crate::error::Result;

/// High-level graph engine facade that coordinates edge storage and traversal.
///
/// This is the main entry point for graph operations. It owns the
/// [`PgEdgeStore`] and delegates traversal to [`TraversalEngine`].
#[derive(Clone)]
pub struct GraphEngine {
    /// The edge storage backend.
    pub edge_store: Arc<PgEdgeStore>,
}

impl GraphEngine {
    /// Create a new graph engine backed by the given edge store.
    pub fn new(edge_store: Arc<PgEdgeStore>) -> Self {
        Self { edge_store }
    }

    /// Create a directed edge between two records.
    ///
    /// Equivalent to SurrealDB's `RELATE from->edge_type->to`.
    /// If the edge already exists, its `data` field is updated (upsert).
    pub async fn relate(&self, input: &EdgeInput) -> Result<Edge> {
        self.edge_store.relate(input).await
    }

    /// Delete an edge by its UUID.
    pub async fn delete_edge(&self, edge_id: uuid::Uuid) -> Result<bool> {
        self.edge_store.delete_edge(edge_id).await
    }

    /// Get all neighbors of a record (edges in both directions).
    pub async fn neighbors(&self, record: &RecordId, edge_type: Option<&str>) -> Result<Vec<Edge>> {
        self.edge_store.get_neighbors(record, edge_type).await
    }

    /// Get outgoing edges from a record.
    pub async fn outgoing(&self, record: &RecordId, edge_type: Option<&str>) -> Result<Vec<Edge>> {
        self.edge_store.get_outgoing(record, edge_type).await
    }

    /// Get incoming edges to a record.
    pub async fn incoming(&self, record: &RecordId, edge_type: Option<&str>) -> Result<Vec<Edge>> {
        self.edge_store.get_incoming(record, edge_type).await
    }

    /// Execute a graph traversal (BFS, DFS, or shortest path).
    pub async fn traverse(&self, config: &TraversalConfig) -> Result<TraversalResult> {
        TraversalEngine::traverse(&self.edge_store, config).await
    }

    /// Find the shortest path between two nodes.
    pub async fn shortest_path(
        &self,
        start: &str,
        target: &str,
        edge_type: Option<&str>,
        max_depth: Option<u32>,
    ) -> Result<ShortestPathResult> {
        let config = TraversalConfig {
            start: start.to_string(),
            direction: Direction::Out,
            edge_type: edge_type.map(String::from),
            max_depth: max_depth.unwrap_or(10),
            max_nodes: 10_000,
            algorithm: TraversalAlgorithm::ShortestPath,
            target: Some(target.to_string()),
        };
        TraversalEngine::shortest_path(&self.edge_store, &config, target).await
    }

    /// Multi-hop path traversal.
    ///
    /// Follows a chain of `(edge_type, direction)` hops from the start record.
    pub async fn traverse_path(
        &self,
        start: &RecordId,
        hops: &[(String, Direction)],
    ) -> Result<Vec<RecordId>> {
        self.edge_store.traverse_path(start, hops).await
    }
}
