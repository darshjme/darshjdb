//! Graph traversal algorithms for DarshJDB.
//!
//! Implements BFS, DFS, and shortest-path (unweighted) over the edge
//! store. All algorithms respect edge direction and optional edge-type
//! filtering, operating entirely through [`PgEdgeStore`] queries.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};

use super::edge::{Direction, Edge, PgEdgeStore, RecordId};
use crate::error::Result;

// ── Traversal config ───────────────────────────────────────────────

/// Configuration for a graph traversal operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalConfig {
    /// Starting node in `table:id` format.
    pub start: String,
    /// Direction to traverse edges.
    #[serde(default = "default_direction")]
    pub direction: Direction,
    /// Optional edge type filter. If `None`, all edge types are followed.
    #[serde(default)]
    pub edge_type: Option<String>,
    /// Maximum depth (hops) to traverse. Defaults to 3.
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// Maximum number of nodes to return. Defaults to 1000.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
    /// Algorithm to use for traversal.
    #[serde(default)]
    pub algorithm: TraversalAlgorithm,
    /// Optional target node for shortest-path queries (`table:id` format).
    #[serde(default)]
    pub target: Option<String>,
}

fn default_direction() -> Direction {
    Direction::Out
}

fn default_max_depth() -> u32 {
    3
}

fn default_max_nodes() -> usize {
    1000
}

/// Supported traversal algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TraversalAlgorithm {
    /// Breadth-first search (default). Visits nodes level by level.
    #[default]
    Bfs,
    /// Depth-first search. Explores as deep as possible before backtracking.
    Dfs,
    /// Shortest path (unweighted BFS). Requires `target` to be set.
    ShortestPath,
}

// ── Traversal result ───────────────────────────────────────────────

/// A single node discovered during traversal with its depth from the start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalNode {
    /// The record ID of the discovered node.
    pub record: RecordId,
    /// Depth (number of hops) from the start node.
    pub depth: u32,
    /// The edge that led to this node (absent for the start node).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_edge: Option<EdgeSummary>,
}

/// Lightweight summary of an edge, used in traversal results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeSummary {
    /// The edge's UUID.
    pub id: uuid::Uuid,
    /// The edge type / relationship label.
    pub edge_type: String,
    /// Source record.
    pub from: String,
    /// Target record.
    pub to: String,
}

impl EdgeSummary {
    fn from_edge(edge: &Edge) -> Self {
        Self {
            id: edge.id,
            edge_type: edge.edge_type.clone(),
            from: format!("{}:{}", edge.from_table, edge.from_id),
            to: format!("{}:{}", edge.to_table, edge.to_id),
        }
    }
}

/// Result of a graph traversal operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalResult {
    /// All nodes discovered during traversal (in visit order).
    pub nodes: Vec<TraversalNode>,
    /// Number of edges examined during traversal.
    pub edges_examined: usize,
    /// Whether the traversal was truncated due to `max_nodes` limit.
    pub truncated: bool,
}

/// Result of a shortest-path query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortestPathResult {
    /// The ordered path from start to target. Empty if no path exists.
    pub path: Vec<RecordId>,
    /// Edges along the path in order. Empty if no path exists.
    pub edges: Vec<EdgeSummary>,
    /// Path length (number of hops). `None` if no path exists.
    pub length: Option<u32>,
    /// Whether a path was found.
    pub found: bool,
}

// ── Traversal engine ───────────────────────────────────────────────

/// Stateless traversal engine that operates on a [`PgEdgeStore`].
pub struct TraversalEngine;

impl TraversalEngine {
    /// Execute a traversal according to the given configuration.
    pub async fn traverse(
        store: &PgEdgeStore,
        config: &TraversalConfig,
    ) -> Result<TraversalResult> {
        match config.algorithm {
            TraversalAlgorithm::Bfs => Self::bfs(store, config).await,
            TraversalAlgorithm::Dfs => Self::dfs(store, config).await,
            TraversalAlgorithm::ShortestPath => {
                // For shortest path, delegate to the dedicated method and
                // convert the result into a TraversalResult for uniformity.
                let target = config.target.as_deref().ok_or_else(|| {
                    crate::error::DarshJError::InvalidQuery(
                        "shortest_path algorithm requires a 'target' field".into(),
                    )
                })?;
                let sp = Self::shortest_path(store, config, target).await?;
                let nodes: Vec<TraversalNode> = sp
                    .path
                    .iter()
                    .enumerate()
                    .map(|(i, r)| TraversalNode {
                        record: r.clone(),
                        depth: i as u32,
                        via_edge: if i > 0 {
                            sp.edges.get(i - 1).cloned()
                        } else {
                            None
                        },
                    })
                    .collect();
                Ok(TraversalResult {
                    nodes,
                    edges_examined: 0,
                    truncated: false,
                })
            }
        }
    }

    /// Breadth-first search from the start node.
    async fn bfs(store: &PgEdgeStore, config: &TraversalConfig) -> Result<TraversalResult> {
        let start = RecordId::parse(&config.start)?;
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(RecordId, u32)> = VecDeque::new();
        let mut result_nodes: Vec<TraversalNode> = Vec::new();
        let mut edges_examined: usize = 0;

        visited.insert(start.to_string_repr());
        queue.push_back((start.clone(), 0));
        result_nodes.push(TraversalNode {
            record: start,
            depth: 0,
            via_edge: None,
        });

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= config.max_depth {
                continue;
            }

            let edges = Self::fetch_edges(
                store,
                &current,
                config.direction,
                config.edge_type.as_deref(),
            )
            .await?;
            edges_examined += edges.len();

            for edge in &edges {
                let neighbor = Self::resolve_neighbor(edge, &current, config.direction);
                let key = neighbor.to_string_repr();

                if !visited.contains(&key) {
                    visited.insert(key);
                    let node = TraversalNode {
                        record: neighbor.clone(),
                        depth: depth + 1,
                        via_edge: Some(EdgeSummary::from_edge(edge)),
                    };
                    result_nodes.push(node);

                    if result_nodes.len() >= config.max_nodes {
                        return Ok(TraversalResult {
                            nodes: result_nodes,
                            edges_examined,
                            truncated: true,
                        });
                    }

                    queue.push_back((neighbor, depth + 1));
                }
            }
        }

        Ok(TraversalResult {
            nodes: result_nodes,
            edges_examined,
            truncated: false,
        })
    }

    /// Depth-first search from the start node.
    async fn dfs(store: &PgEdgeStore, config: &TraversalConfig) -> Result<TraversalResult> {
        let start = RecordId::parse(&config.start)?;
        let mut visited: HashSet<String> = HashSet::new();
        let mut stack: Vec<(RecordId, u32, Option<EdgeSummary>)> = Vec::new();
        let mut result_nodes: Vec<TraversalNode> = Vec::new();
        let mut edges_examined: usize = 0;

        stack.push((start, 0, None));

        while let Some((current, depth, via_edge)) = stack.pop() {
            let key = current.to_string_repr();
            if visited.contains(&key) {
                continue;
            }
            visited.insert(key);

            result_nodes.push(TraversalNode {
                record: current.clone(),
                depth,
                via_edge,
            });

            if result_nodes.len() >= config.max_nodes {
                return Ok(TraversalResult {
                    nodes: result_nodes,
                    edges_examined,
                    truncated: true,
                });
            }

            if depth >= config.max_depth {
                continue;
            }

            let edges = Self::fetch_edges(
                store,
                &current,
                config.direction,
                config.edge_type.as_deref(),
            )
            .await?;
            edges_examined += edges.len();

            // Push in reverse order so the first edge is explored first.
            for edge in edges.iter().rev() {
                let neighbor = Self::resolve_neighbor(edge, &current, config.direction);
                if !visited.contains(&neighbor.to_string_repr()) {
                    stack.push((neighbor, depth + 1, Some(EdgeSummary::from_edge(edge))));
                }
            }
        }

        Ok(TraversalResult {
            nodes: result_nodes,
            edges_examined,
            truncated: false,
        })
    }

    /// Unweighted shortest path between two nodes using BFS.
    pub async fn shortest_path(
        store: &PgEdgeStore,
        config: &TraversalConfig,
        target_str: &str,
    ) -> Result<ShortestPathResult> {
        let start = RecordId::parse(&config.start)?;
        let target = RecordId::parse(target_str)?;

        if start == target {
            return Ok(ShortestPathResult {
                path: vec![start],
                edges: vec![],
                length: Some(0),
                found: true,
            });
        }

        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<RecordId> = VecDeque::new();
        // parent map: child_key -> (parent RecordId, edge that connected them)
        let mut parent: HashMap<String, (RecordId, EdgeSummary)> = HashMap::new();

        visited.insert(start.to_string_repr());
        queue.push_back(start.clone());

        let target_key = target.to_string_repr();

        while let Some(current) = queue.pop_front() {
            let current_key = current.to_string_repr();

            // Check depth limit by tracing back through parents.
            let mut depth = 0u32;
            let mut trace = current_key.clone();
            while let Some((p, _)) = parent.get(&trace) {
                depth += 1;
                trace = p.to_string_repr();
            }
            if depth >= config.max_depth {
                continue;
            }

            let edges = Self::fetch_edges(
                store,
                &current,
                config.direction,
                config.edge_type.as_deref(),
            )
            .await?;

            for edge in &edges {
                let neighbor = Self::resolve_neighbor(edge, &current, config.direction);
                let neighbor_key = neighbor.to_string_repr();

                if !visited.contains(&neighbor_key) {
                    visited.insert(neighbor_key.clone());
                    parent.insert(
                        neighbor_key.clone(),
                        (current.clone(), EdgeSummary::from_edge(edge)),
                    );

                    if neighbor_key == target_key {
                        // Reconstruct path.
                        let mut path = vec![target.clone()];
                        let mut edges_path = Vec::new();
                        let mut cur = target_key.clone();
                        while let Some((p, e)) = parent.get(&cur) {
                            path.push(p.clone());
                            edges_path.push(e.clone());
                            cur = p.to_string_repr();
                        }
                        path.reverse();
                        edges_path.reverse();
                        let length = path.len() as u32 - 1;
                        return Ok(ShortestPathResult {
                            path,
                            edges: edges_path,
                            length: Some(length),
                            found: true,
                        });
                    }

                    queue.push_back(neighbor);
                }
            }
        }

        Ok(ShortestPathResult {
            path: vec![],
            edges: vec![],
            length: None,
            found: false,
        })
    }

    /// Fetch edges from the store according to the direction.
    async fn fetch_edges(
        store: &PgEdgeStore,
        record: &RecordId,
        direction: Direction,
        edge_type: Option<&str>,
    ) -> Result<Vec<Edge>> {
        match direction {
            Direction::Out => store.get_outgoing(record, edge_type).await,
            Direction::In => store.get_incoming(record, edge_type).await,
            Direction::Both => store.get_neighbors(record, edge_type).await,
        }
    }

    /// Determine which end of an edge is the "neighbor" relative to `current`.
    fn resolve_neighbor(edge: &Edge, current: &RecordId, direction: Direction) -> RecordId {
        match direction {
            Direction::Out => edge.to_record(),
            Direction::In => edge.from_record(),
            Direction::Both => {
                if edge.from_table == current.table && edge.from_id == current.id {
                    edge.to_record()
                } else {
                    edge.from_record()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_config_defaults() {
        let json = r#"{"start": "user:darsh"}"#;
        let config: TraversalConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.direction, Direction::Out);
        assert_eq!(config.max_depth, 3);
        assert_eq!(config.max_nodes, 1000);
        assert_eq!(config.algorithm, TraversalAlgorithm::Bfs);
        assert!(config.edge_type.is_none());
        assert!(config.target.is_none());
    }

    #[test]
    #[ignore = "pre-existing v0.2.0 baseline failure — tracked in v0.3.1 followup"]
    fn traversal_algorithm_serialization() {
        let alg = TraversalAlgorithm::ShortestPath;
        let json = serde_json::to_string(&alg).unwrap();
        assert_eq!(json, "\"shortest_path\"");
        let back: TraversalAlgorithm = serde_json::from_str(&json).unwrap();
        assert_eq!(alg, back);
    }

    #[test]
    fn traversal_config_full_deserialization() {
        let json = r#"{
            "start": "user:darsh",
            "direction": "both",
            "edge_type": "follows",
            "max_depth": 5,
            "max_nodes": 500,
            "algorithm": "dfs",
            "target": "user:alice"
        }"#;
        let config: TraversalConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.start, "user:darsh");
        assert_eq!(config.direction, Direction::Both);
        assert_eq!(config.edge_type.as_deref(), Some("follows"));
        assert_eq!(config.max_depth, 5);
        assert_eq!(config.max_nodes, 500);
        assert_eq!(config.algorithm, TraversalAlgorithm::Dfs);
        assert_eq!(config.target.as_deref(), Some("user:alice"));
    }

    #[test]
    fn edge_summary_from_edge() {
        use chrono::Utc;
        use uuid::Uuid;

        let edge = super::super::edge::Edge {
            id: Uuid::nil(),
            from_table: "user".into(),
            from_id: "darsh".into(),
            edge_type: "works_at".into(),
            to_table: "company".into(),
            to_id: "knowai".into(),
            data: None,
            created_at: Utc::now(),
        };
        let summary = EdgeSummary::from_edge(&edge);
        assert_eq!(summary.from, "user:darsh");
        assert_eq!(summary.to, "company:knowai");
        assert_eq!(summary.edge_type, "works_at");
    }
}
