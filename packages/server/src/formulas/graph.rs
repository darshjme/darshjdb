//! Dependency graph for formula fields — topological ordering and cycle detection.
//!
//! When a raw field changes, the graph determines which formula fields must be
//! recalculated and in what order.  Supports cross-table dependencies via
//! qualified field references (`TableName.FieldName`).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::error::{DarshJError, Result};
use crate::formulas::parser::{self, Expr};

/// A node in the dependency graph representing a formula field.
#[derive(Debug, Clone)]
struct FormulaNode {
    /// The raw formula expression string (for debugging / display).
    #[allow(dead_code)]
    formula: String,
    /// Parsed AST (kept for potential re-evaluation).
    pub expr: Expr,
    /// Fields this formula depends on (extracted from the AST).
    pub dependencies: Vec<String>,
}

/// Directed acyclic graph tracking formula field dependencies.
///
/// Nodes are field identifiers (plain `"FieldName"` for same-table, or
/// `"Table.Field"` for cross-table lookups / rollups).  An edge from A → B
/// means "B depends on A" — when A changes, B must be recalculated.
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// field_id → node
    nodes: HashMap<String, FormulaNode>,
    /// field_id → set of field_ids that directly depend on it
    /// (forward edges: "A changed" → recalculate these)
    dependents: HashMap<String, HashSet<String>>,
}

impl DependencyGraph {
    /// Create an empty graph.
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            dependents: HashMap::new(),
        }
    }

    /// Register a formula field.  Parses dependencies from the AST and wires
    /// up the internal edge map.
    ///
    /// `field_id` — the identifier of the formula field (e.g. `"Total"`).
    /// `formula` — the raw formula string (e.g. `"{Price} * {Qty}"`).
    pub fn add_formula_field(&mut self, field_id: impl Into<String>, formula: &str) -> Result<()> {
        let field_id = field_id.into();
        let expr = parser::parse(formula)?;
        let deps = parser::extract_field_refs(&expr);

        // Wire forward edges: for each dependency, record that `field_id` depends on it.
        for dep in &deps {
            self.dependents
                .entry(dep.clone())
                .or_default()
                .insert(field_id.clone());
        }

        self.nodes.insert(
            field_id,
            FormulaNode {
                formula: formula.to_string(),
                expr,
                dependencies: deps,
            },
        );

        Ok(())
    }

    /// Register a formula field from a pre-parsed expression.
    pub fn add_formula_field_expr(
        &mut self,
        field_id: impl Into<String>,
        expr: Expr,
    ) {
        let field_id = field_id.into();
        let deps = parser::extract_field_refs(&expr);

        for dep in &deps {
            self.dependents
                .entry(dep.clone())
                .or_default()
                .insert(field_id.clone());
        }

        self.nodes.insert(
            field_id,
            FormulaNode {
                formula: String::new(),
                expr,
                dependencies: deps,
            },
        );
    }

    /// Remove a formula field and clean up edges.
    pub fn remove_formula_field(&mut self, field_id: &str) {
        if let Some(node) = self.nodes.remove(field_id) {
            for dep in &node.dependencies {
                if let Some(set) = self.dependents.get_mut(dep) {
                    set.remove(field_id);
                    if set.is_empty() {
                        self.dependents.remove(dep);
                    }
                }
            }
        }
    }

    /// Given a set of changed (raw) fields, return the list of formula fields
    /// that need recalculation, in topological order (dependencies before
    /// dependents).
    ///
    /// This performs a BFS to find all transitively affected formula fields,
    /// then does a Kahn's-algorithm topological sort restricted to that subset.
    pub fn calculation_order(&self, changed_fields: &[String]) -> Vec<String> {
        // 1. BFS to collect all affected formula fields
        let mut affected: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        for field in changed_fields {
            if let Some(deps) = self.dependents.get(field) {
                for dep in deps {
                    if affected.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        while let Some(field) = queue.pop_front() {
            if let Some(deps) = self.dependents.get(&field) {
                for dep in deps {
                    if affected.insert(dep.clone()) {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        if affected.is_empty() {
            return Vec::new();
        }

        // 2. Topological sort (Kahn's algorithm) over the affected subset
        // Build in-degree counts restricted to the affected set.
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        for field in &affected {
            in_degree.entry(field.clone()).or_insert(0);
            if let Some(node) = self.nodes.get(field) {
                for dep in &node.dependencies {
                    if affected.contains(dep) {
                        *in_degree.entry(field.clone()).or_insert(0) += 1;
                    }
                }
            }
        }

        let mut ready: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(k, _)| k.clone())
            .collect();
        // Sort for deterministic output
        let mut ready_vec: Vec<String> = ready.drain(..).collect();
        ready_vec.sort();
        ready = ready_vec.into_iter().collect();

        let mut order = Vec::new();
        while let Some(field) = ready.pop_front() {
            order.push(field.clone());
            if let Some(deps) = self.dependents.get(&field) {
                for dep in deps {
                    if affected.contains(dep) {
                        if let Some(deg) = in_degree.get_mut(dep) {
                            *deg = deg.saturating_sub(1);
                            if *deg == 0 {
                                ready.push_back(dep.clone());
                            }
                        }
                    }
                }
            }
        }

        // If order doesn't contain all affected, there's a cycle —
        // but we still return what we can.
        order
    }

    /// Detect circular references in the graph.
    ///
    /// Returns `Some(cycle)` with the field names forming the cycle,
    /// or `None` if the graph is acyclic.
    pub fn detect_cycles(&self) -> Option<Vec<String>> {
        // DFS-based cycle detection with path tracking
        let mut visited: HashSet<String> = HashSet::new();
        let mut on_stack: HashSet<String> = HashSet::new();
        let mut path: Vec<String> = Vec::new();

        for field_id in self.nodes.keys() {
            if !visited.contains(field_id) {
                if let Some(cycle) =
                    self.dfs_cycle(field_id, &mut visited, &mut on_stack, &mut path)
                {
                    return Some(cycle);
                }
            }
        }
        None
    }

    fn dfs_cycle(
        &self,
        field: &str,
        visited: &mut HashSet<String>,
        on_stack: &mut HashSet<String>,
        path: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        visited.insert(field.to_string());
        on_stack.insert(field.to_string());
        path.push(field.to_string());

        // Follow edges: this field depends on others, and others depend on this field.
        // For cycle detection, we follow the "depends on" edges from formula nodes.
        if let Some(node) = self.nodes.get(field) {
            for dep in &node.dependencies {
                // Only follow dependencies that are themselves formula fields
                if self.nodes.contains_key(dep) {
                    if !visited.contains(dep) {
                        if let Some(cycle) = self.dfs_cycle(dep, visited, on_stack, path) {
                            return Some(cycle);
                        }
                    } else if on_stack.contains(dep) {
                        // Found a cycle — extract it from the path
                        let cycle_start = path.iter().position(|p| p == dep).unwrap();
                        let mut cycle: Vec<String> = path[cycle_start..].to_vec();
                        cycle.push(dep.clone()); // close the loop
                        return Some(cycle);
                    }
                }
            }
        }

        on_stack.remove(field);
        path.pop();
        None
    }

    /// Get the parsed expression for a formula field.
    pub fn get_expr(&self, field_id: &str) -> Option<&Expr> {
        self.nodes.get(field_id).map(|n| &n.expr)
    }

    /// Get all registered formula field ids.
    pub fn formula_fields(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    /// Get direct dependencies for a field.
    pub fn dependencies_of(&self, field_id: &str) -> Option<&[String]> {
        self.nodes.get(field_id).map(|n| n.dependencies.as_slice())
    }

    /// Get direct dependents of a field (fields that use this field).
    pub fn dependents_of(&self, field_id: &str) -> Vec<String> {
        self.dependents
            .get(field_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Number of formula fields registered.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether the graph is empty.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_dependency() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("Total", "{Price} * {Qty}").unwrap();

        let order = g.calculation_order(&["Price".into()]);
        assert_eq!(order, vec!["Total"]);
    }

    #[test]
    fn test_no_affected() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("Total", "{Price} * {Qty}").unwrap();

        let order = g.calculation_order(&["Unrelated".into()]);
        assert!(order.is_empty());
    }

    #[test]
    fn test_chain_dependency() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("Subtotal", "{Price} * {Qty}").unwrap();
        g.add_formula_field("Tax", "{Subtotal} * 0.1").unwrap();
        g.add_formula_field("Total", "{Subtotal} + {Tax}").unwrap();

        let order = g.calculation_order(&["Price".into()]);
        // Subtotal must come before Tax and Total; Tax before Total
        let sub_pos = order.iter().position(|x| x == "Subtotal").unwrap();
        let tax_pos = order.iter().position(|x| x == "Tax").unwrap();
        let total_pos = order.iter().position(|x| x == "Total").unwrap();
        assert!(sub_pos < tax_pos);
        assert!(sub_pos < total_pos);
        assert!(tax_pos < total_pos);
    }

    #[test]
    fn test_detect_no_cycle() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("A", "{X} + 1").unwrap();
        g.add_formula_field("B", "{A} + 2").unwrap();
        assert!(g.detect_cycles().is_none());
    }

    #[test]
    fn test_detect_direct_cycle() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("A", "{B} + 1").unwrap();
        g.add_formula_field("B", "{A} + 1").unwrap();
        let cycle = g.detect_cycles();
        assert!(cycle.is_some());
        let c = cycle.unwrap();
        assert!(c.contains(&"A".to_string()));
        assert!(c.contains(&"B".to_string()));
    }

    #[test]
    fn test_detect_indirect_cycle() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("A", "{B} + 1").unwrap();
        g.add_formula_field("B", "{C} + 1").unwrap();
        g.add_formula_field("C", "{A} + 1").unwrap();
        assert!(g.detect_cycles().is_some());
    }

    #[test]
    fn test_remove_formula_field() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("Total", "{Price} * {Qty}").unwrap();
        assert_eq!(g.len(), 1);
        g.remove_formula_field("Total");
        assert_eq!(g.len(), 0);
        assert!(g.calculation_order(&["Price".into()]).is_empty());
    }

    #[test]
    fn test_multiple_changed_fields() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("Full", r#"CONCAT({First}, " ", {Last})"#)
            .unwrap();
        g.add_formula_field("Score", "{A} + {B}").unwrap();

        let order = g.calculation_order(&["First".into(), "A".into()]);
        assert!(order.contains(&"Full".to_string()));
        assert!(order.contains(&"Score".to_string()));
    }

    #[test]
    fn test_diamond_dependency() {
        // A depends on X, B depends on X, C depends on A and B
        let mut g = DependencyGraph::new();
        g.add_formula_field("A", "{X} + 1").unwrap();
        g.add_formula_field("B", "{X} * 2").unwrap();
        g.add_formula_field("C", "{A} + {B}").unwrap();

        let order = g.calculation_order(&["X".into()]);
        assert_eq!(order.len(), 3);
        let c_pos = order.iter().position(|x| x == "C").unwrap();
        let a_pos = order.iter().position(|x| x == "A").unwrap();
        let b_pos = order.iter().position(|x| x == "B").unwrap();
        assert!(a_pos < c_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_cross_table_ref() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("OrderTotal", "{Items.Price} * {Qty}")
            .unwrap();
        let deps = g.dependencies_of("OrderTotal").unwrap();
        assert!(deps.contains(&"Items.Price".to_string()));
        assert!(deps.contains(&"Qty".to_string()));
    }

    #[test]
    fn test_dependents_of() {
        let mut g = DependencyGraph::new();
        g.add_formula_field("Total", "{Price} * {Qty}").unwrap();
        g.add_formula_field("Display", r#"CONCAT("$", {Total})"#)
            .unwrap();

        let deps = g.dependents_of("Price");
        assert!(deps.contains(&"Total".to_string()));

        let deps2 = g.dependents_of("Total");
        assert!(deps2.contains(&"Display".to_string()));
    }
}
