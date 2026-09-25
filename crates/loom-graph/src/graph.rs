//! In-memory directed code dependency graph backed by `petgraph` with bidirectional lookup tables.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::SymbolNode;
use petgraph::Direction;
use petgraph::graph::{DiGraph, EdgeIndex, NodeIndex};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};

use crate::error::{GraphError, Result};

/// In-memory directed dependency graph maintaining repository-wide topological structure.
///
/// Edges point from caller / dependant (source) to callee / dependency (target).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGraph {
    /// Underlying directed graph with symbol nodes and dependency edges.
    pub graph: DiGraph<SymbolNode, DependencyEdge>,
    /// Fast $O(1)$ mapping from deterministic `SymbolId` to petgraph `NodeIndex`.
    pub symbol_to_node: HashMap<SymbolId, NodeIndex>,
    /// Index mapping each source file path to the list of symbols defined within it.
    pub file_to_symbols: HashMap<PathBuf, Vec<SymbolId>>,
}

impl Default for CodeGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeGraph {
    /// Creates a new, empty `CodeGraph`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            symbol_to_node: HashMap::new(),
            file_to_symbols: HashMap::new(),
        }
    }

    /// Creates a new `CodeGraph` with pre-allocated node and edge capacities.
    #[must_use]
    pub fn with_capacity(nodes: usize, edges: usize) -> Self {
        Self {
            graph: DiGraph::with_capacity(nodes, edges),
            symbol_to_node: HashMap::with_capacity(nodes),
            file_to_symbols: HashMap::new(),
        }
    }

    /// Total number of symbol nodes currently in the graph.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Total number of dependency edges currently in the graph.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Total number of distinct source files tracked in the graph.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.file_to_symbols.len()
    }

    /// Checks if a symbol exists in the graph.
    #[must_use]
    pub fn contains_symbol(&self, id: &SymbolId) -> bool {
        self.symbol_to_node.contains_key(id)
    }

    /// Retrieves a reference to a `SymbolNode` by its `SymbolId`.
    #[must_use]
    pub fn get_symbol(&self, id: &SymbolId) -> Option<&SymbolNode> {
        let &node_idx = self.symbol_to_node.get(id)?;
        self.graph.node_weight(node_idx)
    }

    /// Retrieves a mutable reference to a `SymbolNode` by its `SymbolId`.
    pub fn get_symbol_mut(&mut self, id: &SymbolId) -> Option<&mut SymbolNode> {
        let &node_idx = self.symbol_to_node.get(id)?;
        self.graph.node_weight_mut(node_idx)
    }

    /// Finds all symbols with a matching name.
    #[must_use]
    pub fn get_symbols_by_name<'a>(&'a self, name: &str) -> Vec<&'a SymbolNode> {
        self.graph
            .node_weights()
            .filter(|node| node.name == name)
            .collect()
    }

    /// Retrieves all symbols defined within a specific file path.
    #[must_use]
    pub fn get_symbols_for_file<'a>(&'a self, path: &Path) -> Vec<&'a SymbolNode> {
        let Some(ids) = self.file_to_symbols.get(path) else {
            return Vec::new();
        };

        ids.iter().filter_map(|id| self.get_symbol(id)).collect()
    }

    /// Inserts or updates a symbol node in the graph.
    ///
    /// If the symbol already exists, its metadata is updated in-place.
    /// If it is new, it is added and registered in both lookup maps.
    pub fn upsert_symbol(&mut self, node: SymbolNode) -> NodeIndex {
        let symbol_id = node.id;
        let file_path = node.file_path.clone();

        if let Some(&existing_idx) = self.symbol_to_node.get(&symbol_id) {
            // Update existing weight
            if let Some(weight) = self.graph.node_weight_mut(existing_idx) {
                *weight = node;
            }
            existing_idx
        } else {
            let node_idx = self.graph.add_node(node);
            self.symbol_to_node.insert(symbol_id, node_idx);
            self.file_to_symbols
                .entry(file_path)
                .or_default()
                .push(symbol_id);
            node_idx
        }
    }

    /// Adds a directed dependency edge from `source_id` to `target_id`.
    ///
    /// # Errors
    /// Returns [`GraphError::EndpointNotFound`] if either `source_id` or `target_id` is missing.
    pub fn add_edge(
        &mut self,
        source_id: SymbolId,
        target_id: SymbolId,
        edge: DependencyEdge,
    ) -> Result<EdgeIndex> {
        let &source_idx = self
            .symbol_to_node
            .get(&source_id)
            .ok_or(GraphError::EndpointNotFound)?;
        let &target_idx = self
            .symbol_to_node
            .get(&target_id)
            .ok_or(GraphError::EndpointNotFound)?;

        Ok(self.graph.add_edge(source_idx, target_idx, edge))
    }

    /// Removes a single symbol and all its incident edges from the graph.
    ///
    /// Handles Petgraph's internal node swap correctly, updating index references.
    pub fn remove_symbol(&mut self, id: SymbolId) -> Option<SymbolNode> {
        let node_idx = self.symbol_to_node.remove(&id)?;

        let old_last_idx = NodeIndex::new(self.graph.node_count().saturating_sub(1));
        let removed = self.graph.remove_node(node_idx);

        // Petgraph `remove_node` moves the last node to the removed index position
        if node_idx != old_last_idx && node_idx.index() < self.graph.node_count() {
            let swapped_symbol = &self.graph[node_idx];
            self.symbol_to_node.insert(swapped_symbol.id, node_idx);
        }

        if let Some(ref node) = removed {
            if let Some(symbols) = self.file_to_symbols.get_mut(&node.file_path) {
                symbols.retain(|&s_id| s_id != id);
                if symbols.is_empty() {
                    self.file_to_symbols.remove(&node.file_path);
                }
            }
        }

        removed
    }

    /// Atomically removes all symbols defined in `file_path` and their incident edges.
    ///
    /// Used for single-file incremental invalidation when a file is modified or deleted.
    pub fn invalidate_file(&mut self, file_path: &Path) -> Vec<SymbolNode> {
        let Some(symbol_ids) = self.file_to_symbols.remove(file_path) else {
            return Vec::new();
        };

        let mut removed = Vec::with_capacity(symbol_ids.len());
        for id in symbol_ids {
            if let Some(node_idx) = self.symbol_to_node.remove(&id) {
                let old_last_idx = NodeIndex::new(self.graph.node_count().saturating_sub(1));
                if let Some(node) = self.graph.remove_node(node_idx) {
                    if node_idx != old_last_idx && node_idx.index() < self.graph.node_count() {
                        let swapped = &self.graph[node_idx];
                        self.symbol_to_node.insert(swapped.id, node_idx);
                    }
                    removed.push(node);
                }
            }
        }
        removed
    }

    /// Returns direct inbound callers / dependants of the specified symbol.
    #[must_use]
    pub fn get_callers<'a>(&'a self, id: &SymbolId) -> Vec<(&'a SymbolNode, &'a DependencyEdge)> {
        self.get_neighbors_by_direction(id, Direction::Incoming, Some(EdgeKind::Calls))
    }

    /// Returns direct outbound callees / dependencies of the specified symbol.
    #[must_use]
    pub fn get_callees<'a>(&'a self, id: &SymbolId) -> Vec<(&'a SymbolNode, &'a DependencyEdge)> {
        self.get_neighbors_by_direction(id, Direction::Outgoing, Some(EdgeKind::Calls))
    }

    /// Returns direct neighbors of the symbol in a given direction, optionally filtered by edge kind.
    #[must_use]
    pub fn get_neighbors_by_direction<'a>(
        &'a self,
        id: &SymbolId,
        direction: Direction,
        kind_filter: Option<EdgeKind>,
    ) -> Vec<(&'a SymbolNode, &'a DependencyEdge)> {
        let Some(&node_idx) = self.symbol_to_node.get(id) else {
            return Vec::new();
        };

        self.graph
            .edges_directed(node_idx, direction)
            .filter_map(|edge_ref| {
                let edge = edge_ref.weight();
                if let Some(expected_kind) = kind_filter {
                    if edge.kind != expected_kind {
                        return None;
                    }
                }

                let target_node_idx = match direction {
                    Direction::Incoming => edge_ref.source(),
                    Direction::Outgoing => edge_ref.target(),
                };

                let node = self.graph.node_weight(target_node_idx)?;
                Some((node, edge))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::symbol::SymbolKind;

    fn create_dummy_node(name: &str, file: &str, line: u32) -> SymbolNode {
        let id = SymbolId::derive(file, &[], name, &format!("fn {name}()"));
        SymbolNode::new(
            id,
            name,
            SymbolKind::Function,
            file,
            (0, 100),
            (line, line + 10),
            None,
            format!("fn {name}()"),
            true,
            1,
        )
    }

    #[test]
    fn test_upsert_and_retrieve_symbol() {
        let mut graph = CodeGraph::new();
        let node = create_dummy_node("authenticate", "src/auth.rs", 10);
        let id = node.id;

        let idx = graph.upsert_symbol(node.clone());
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.file_count(), 1);

        let retrieved = graph.get_symbol(&id).expect("symbol exists");
        assert_eq!(retrieved.name, "authenticate");
        assert_eq!(retrieved.id, id);
        assert_eq!(graph.symbol_to_node[&id], idx);

        let file_symbols = graph.get_symbols_for_file(Path::new("src/auth.rs"));
        assert_eq!(file_symbols.len(), 1);
        assert_eq!(file_symbols[0].id, id);
    }

    #[test]
    fn test_add_edge_and_neighbor_queries() {
        let mut graph = CodeGraph::new();
        let origin = create_dummy_node("login_handler", "src/api.rs", 20);
        let target = create_dummy_node("authenticate", "src/auth.rs", 10);

        let origin_id = origin.id;
        let target_id = target.id;

        graph.upsert_symbol(origin);
        graph.upsert_symbol(target);

        let edge = DependencyEdge::new(EdgeKind::Calls, 25, false);
        graph
            .add_edge(origin_id, target_id, edge)
            .expect("add edge");

        assert_eq!(graph.edge_count(), 1);

        // Inbound caller query
        let callers = graph.get_callers(&target_id);
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].0.id, origin_id);
        assert_eq!(callers[0].1.call_site_line, 25);

        // Outbound callee query
        let outbound = graph.get_callees(&origin_id);
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].0.id, target_id);
    }

    #[test]
    fn test_petgraph_node_swap_removal_consistency() {
        let mut graph = CodeGraph::new();
        let node_a = create_dummy_node("a", "src/a.rs", 1);
        let node_b = create_dummy_node("b", "src/b.rs", 1);
        let node_c = create_dummy_node("c", "src/c.rs", 1);

        let id_a = node_a.id;
        let id_b = node_b.id;
        let id_c = node_c.id;

        graph.upsert_symbol(node_a);
        graph.upsert_symbol(node_b);
        graph.upsert_symbol(node_c);

        assert_eq!(graph.node_count(), 3);

        // Removing node_a (index 0) will cause node_c (index 2) to be swapped into index 0!
        let removed = graph.remove_symbol(id_a);
        assert!(removed.is_some());
        assert_eq!(graph.node_count(), 2);

        // Verify lookup index for node_c is still valid and points to the correct node
        let c_node = graph.get_symbol(&id_c).expect("c must be found");
        assert_eq!(c_node.name, "c");
        assert_eq!(graph.symbol_to_node[&id_c], NodeIndex::new(0));

        let b_node = graph.get_symbol(&id_b).expect("b must be found");
        assert_eq!(b_node.name, "b");
    }

    #[test]
    fn test_invalidate_file() {
        let mut graph = CodeGraph::new();
        let s1 = create_dummy_node("func1", "src/service.rs", 1);
        let s2 = create_dummy_node("func2", "src/service.rs", 20);
        let s3 = create_dummy_node("other", "src/other.rs", 1);

        let id1 = s1.id;
        let id2 = s2.id;
        let id3 = s3.id;

        graph.upsert_symbol(s1);
        graph.upsert_symbol(s2);
        graph.upsert_symbol(s3);

        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.file_count(), 2);

        let removed = graph.invalidate_file(Path::new("src/service.rs"));
        assert_eq!(removed.len(), 2);
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.file_count(), 1);

        assert!(!graph.contains_symbol(&id1));
        assert!(!graph.contains_symbol(&id2));
        assert!(graph.contains_symbol(&id3));
        assert_eq!(
            graph
                .get_symbols_for_file(Path::new("src/service.rs"))
                .len(),
            0
        );
    }
}
