//! In-memory directed code dependency graph backed by `petgraph` with bidirectional lookup tables.

use std::collections::{HashMap, HashSet, VecDeque};
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
    ///
    /// Linear scan over node weights: intended for MCP lookups and resolution fallbacks
    /// where a repo-wide name is ambiguous. Hot paths must resolve by [`SymbolId`].
    #[must_use]
    pub fn get_symbols_by_name<'a>(&'a self, name: &str) -> Vec<&'a SymbolNode> {
        self.graph
            .node_weights()
            .filter(|node| node.name == name)
            .collect()
    }

    /// Retrieves all symbols defined within a specific file path.
    ///
    /// Driven by the `file_to_symbols` reverse index, so it stays $O(k)$ in the file's
    /// symbol count instead of scanning the whole graph. Unknown files return an empty
    /// vector (graceful degradation for deleted or never-indexed paths).
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
    ///
    /// The returned [`NodeIndex`] is stable for the lifetime of the node: a later
    /// [`CodeGraph::remove_symbol`] of an *unrelated* node may relocate this node's
    /// index, which is exactly why `symbol_to_node` must be re-synchronized on removal.
    /// Callers that need a permanent handle should retain the `SymbolId` instead.
    pub fn upsert_symbol(&mut self, node: SymbolNode) -> NodeIndex {
        let symbol_id = node.id;
        let file_path = node.file_path.clone();

        if let Some(&existing_idx) = self.symbol_to_node.get(&symbol_id) {
            // Fast path: the deterministic `SymbolId` already exists, so refresh the
            // metadata (line ranges, signature, epoch, ...) without touching the topology.
            // Edges are intentionally preserved: re-parsing a file must not sever callers
            // that live in *other* files, they are re-linked by the indexing pipeline.
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
    /// Edges are oriented caller → callee, so the source is the *impacted* symbol when
    /// walking inbound neighbours. Both endpoints are resolved through
    /// [`CodeGraph::symbol_to_node`], which guarantees that traversal algorithms never
    /// observe a stale [`NodeIndex`].
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
    ///
    /// # Why the reconciliation below is mandatory
    /// `DiGraph::remove_node` is a *swap-removal*: the highest-indexed node is moved into
    /// the freed slot. Any external `SymbolId -> NodeIndex` table would silently start
    /// pointing at the wrong node, corrupting every subsequent traversal (a classic
    /// "works until you delete a file" class of bug). We therefore re-read the node that
    /// landed on `node_idx` and repoint its mapping.
    pub fn remove_symbol(&mut self, id: SymbolId) -> Option<SymbolNode> {
        let node_idx = self.symbol_to_node.remove(&id)?;

        // Capture the pre-removal tail index; after the swap-removal it is either the
        // removed node itself (nothing moved) or the node that got relocated.
        let old_last_idx = NodeIndex::new(self.graph.node_count().saturating_sub(1));
        let removed = self.graph.remove_node(node_idx);

        // Petgraph `remove_node` moves the last node to the removed index position
        if node_idx != old_last_idx && node_idx.index() < self.graph.node_count() {
            let swapped_symbol = &self.graph[node_idx];
            self.symbol_to_node.insert(swapped_symbol.id, node_idx);
        }

        // Keep the file index consistent: drop the dead id and forget files that no
        // longer contribute any symbol, so `file_count` stays truthful.
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
    ///
    /// This is the backbone of the incremental update invariant: only symbols in the dirty
    /// file are dropped (plus their incident edges, which petgraph discards automatically),
    /// while the rest of the workspace topology is left untouched. Runs in $O(k)$ where
    /// $k$ is the number of symbols in the file, which is what keeps single-file
    /// re-indexing well under the 5 ms budget.
    pub fn invalidate_file(&mut self, file_path: &Path) -> Vec<SymbolNode> {
        let Some(symbol_ids) = self.file_to_symbols.remove(file_path) else {
            return Vec::new();
        };

        let mut removed = Vec::with_capacity(symbol_ids.len());
        for id in symbol_ids {
            if let Some(node_idx) = self.symbol_to_node.remove(&id) {
                // Same swap-removal reconciliation as `remove_symbol`; it must be repeated
                // per iteration because every removal can relocate a *different* node.
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
    ///
    /// This is the single-hop seed used by blast-radius analysis; it is intentionally
    /// restricted to [`EdgeKind::Calls`] so that type references or imports do not
    /// inflate impact estimates.
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
    ///
    /// Direction mapping note: for [`Direction::Incoming`] the *other* endpoint of an edge is
    /// its `source()` (the caller), for [`Direction::Outgoing`] it is its `target()`
    /// (the callee). Callers of this function are the sole place translating petgraph's
    /// edge orientation into "who calls whom" semantics.
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

    /// Returns transitive inbound callers up to `max_depth` BFS layers away.
    ///
    /// The result is a list of `(SymbolNode, depth)` tuples, ordered by distance.
    /// Safely handles cyclic call graphs without infinite loops.
    ///
    /// Depth 1 means "direct caller"; the depth value is the exponential-decay exponent
    /// consumed by the blast-radius risk scoring in `loom-analysis`, so it must stay an
    /// exact shortest-path distance rather than an arbitrary visit order.
    #[must_use]
    pub fn find_transitive_callers(
        &self,
        root: &SymbolId,
        max_depth: usize,
    ) -> Vec<(SymbolNode, usize)> {
        self.traverse_transitive(root, Direction::Incoming, Some(EdgeKind::Calls), max_depth)
    }

    /// Returns transitive outbound callees up to `max_depth` BFS layers away.
    ///
    /// The result is a list of `(SymbolNode, depth)` tuples, ordered by distance.
    /// Safely handles cyclic call graphs without infinite loops.
    #[must_use]
    pub fn find_transitive_callees(
        &self,
        root: &SymbolId,
        max_depth: usize,
    ) -> Vec<(SymbolNode, usize)> {
        self.traverse_transitive(root, Direction::Outgoing, Some(EdgeKind::Calls), max_depth)
    }

    /// Generalized BFS traversal collecting reachable nodes and their topological depth.
    ///
    /// Single-pass, depth-bounded, cycle-safe BFS in $O(V + E)$ over the filtered sub-graph.
    ///
    /// Semantics:
    /// - The root itself is **not** included in the results; every returned entry is a
    ///   strict neighbor at depth `1..=max_depth`, ordered by BFS discovery (i.e. by
    ///   topological distance, which is what blast-radius depth decay relies on).
    /// - `max_depth == 0` yields an empty set: no expansion is requested at all.
    /// - `visited` is seeded with the root so that self-recursive symbols (`a -> a`) and
    ///   mutual recursion (`a -> b -> a`) terminate in one pass without duplicate entries.
    #[must_use]
    pub fn traverse_transitive(
        &self,
        root: &SymbolId,
        direction: Direction,
        kind_filter: Option<EdgeKind>,
        max_depth: usize,
    ) -> Vec<(SymbolNode, usize)> {
        let Some(&root_idx) = self.symbol_to_node.get(root) else {
            return Vec::new();
        };

        if max_depth == 0 {
            return Vec::new();
        }

        // All three containers are allocated up-front and reused; no intermediate
        // collections are built per level, which is what keeps 50k-node traversals
        // inside the < 2 ms budget.
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut results = Vec::new();

        visited.insert(root_idx);
        queue.push_back((root_idx, 0));

        while let Some((curr_idx, curr_depth)) = queue.pop_front() {
            // Depth gate: nodes already at the limit are still recorded (they were pushed
            // by their parent) but never expanded.
            if curr_depth >= max_depth {
                continue;
            }

            for edge_ref in self.graph.edges_directed(curr_idx, direction) {
                if let Some(expected_kind) = kind_filter {
                    if edge_ref.weight().kind != expected_kind {
                        continue;
                    }
                }

                let neighbor_idx = match direction {
                    Direction::Incoming => edge_ref.source(),
                    Direction::Outgoing => edge_ref.target(),
                };

                // `insert` returning true means "first visit": guarantees each node is
                // reported once even in diamond or cyclic topologies.
                if visited.insert(neighbor_idx) {
                    let next_depth = curr_depth + 1;
                    if let Some(node) = self.graph.node_weight(neighbor_idx) {
                        results.push((node.clone(), next_depth));
                    }
                    queue.push_back((neighbor_idx, next_depth));
                }
            }
        }

        results
    }

    /// Computes the shortest dependency path from `from` to `to` along outgoing dependency edges.
    ///
    /// Returns `Some(path)` containing all symbol nodes from `from` to `to` inclusive,
    /// or `None` if no path exists.
    ///
    /// Unweighted BFS, so the first time `to` is dequeued it is reached through a
    /// minimum-edge-count route — no Dijkstra needed. `came_from` doubles as the
    /// predecessor map used to reconstruct the path backwards from the target.
    #[must_use]
    pub fn find_shortest_path(&self, from: &SymbolId, to: &SymbolId) -> Option<Vec<SymbolNode>> {
        let &from_idx = self.symbol_to_node.get(from)?;
        let &to_idx = self.symbol_to_node.get(to)?;

        // Trivial identity path: a symbol is trivially reachable from itself.
        if from_idx == to_idx {
            return self.graph.node_weight(from_idx).cloned().map(|n| vec![n]);
        }

        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();
        let mut came_from = HashMap::new();

        visited.insert(from_idx);
        queue.push_back(from_idx);

        let mut found = false;
        while let Some(curr_idx) = queue.pop_front() {
            // Dequeue-time check keeps the discovered path minimal (BFS level order).
            if curr_idx == to_idx {
                found = true;
                break;
            }

            // Deliberately unfiltered: shortest-path discovery answers "how do I reach
            // it", not "how do I call it", so all edge kinds participate.
            for edge_ref in self.graph.edges_directed(curr_idx, Direction::Outgoing) {
                let neighbor_idx = edge_ref.target();
                if visited.insert(neighbor_idx) {
                    came_from.insert(neighbor_idx, curr_idx);
                    queue.push_back(neighbor_idx);
                }
            }
        }

        if !found {
            return None;
        }

        // Walk predecessors from `to` back to `from`, then reverse into forward order.
        let mut path_indices = Vec::new();
        let mut curr = to_idx;
        path_indices.push(curr);

        while let Some(&prev) = came_from.get(&curr) {
            path_indices.push(prev);
            curr = prev;
            if curr == from_idx {
                break;
            }
        }

        path_indices.reverse();
        let path_nodes = path_indices
            .into_iter()
            .filter_map(|idx| self.graph.node_weight(idx).cloned())
            .collect();

        Some(path_nodes)
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

    #[test]
    fn test_transitive_callers_and_callees() {
        let mut graph = CodeGraph::new();
        // node_alpha -> node_beta -> node_gamma -> node_delta
        let node_alpha = create_dummy_node("alpha", "src/alpha.rs", 1);
        let node_beta = create_dummy_node("beta", "src/beta.rs", 1);
        let node_gamma = create_dummy_node("gamma", "src/gamma.rs", 1);
        let node_delta = create_dummy_node("delta", "src/delta.rs", 1);

        let id_alpha = node_alpha.id;
        let id_beta = node_beta.id;
        let id_gamma = node_gamma.id;
        let id_delta = node_delta.id;

        graph.upsert_symbol(node_alpha);
        graph.upsert_symbol(node_beta);
        graph.upsert_symbol(node_gamma);
        graph.upsert_symbol(node_delta);

        let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
        graph
            .add_edge(id_alpha, id_beta, edge.clone())
            .expect("add alpha->beta");
        graph
            .add_edge(id_beta, id_gamma, edge.clone())
            .expect("add beta->gamma");
        graph
            .add_edge(id_gamma, id_delta, edge.clone())
            .expect("add gamma->delta");

        // Transitive callers of delta (max_depth 5)
        let callers = graph.find_transitive_callers(&id_delta, 5);
        assert_eq!(callers.len(), 3);
        assert_eq!(callers[0].0.id, id_gamma);
        assert_eq!(callers[0].1, 1);
        assert_eq!(callers[1].0.id, id_beta);
        assert_eq!(callers[1].1, 2);
        assert_eq!(callers[2].0.id, id_alpha);
        assert_eq!(callers[2].1, 3);

        // Transitive callers of delta with max_depth 1
        let shallow_callers = graph.find_transitive_callers(&id_delta, 1);
        assert_eq!(shallow_callers.len(), 1);
        assert_eq!(shallow_callers[0].0.id, id_gamma);

        // Transitive outbound of alpha (max_depth 2)
        let outbound = graph.find_transitive_callees(&id_alpha, 2);
        assert_eq!(outbound.len(), 2);
        assert_eq!(outbound[0].0.id, id_beta);
        assert_eq!(outbound[0].1, 1);
        assert_eq!(outbound[1].0.id, id_gamma);
        assert_eq!(outbound[1].1, 2);
    }

    #[test]
    fn test_find_shortest_path() {
        let mut graph = CodeGraph::new();
        // start -> mid_1 -> mid_2 -> destination
        // start -> destination (direct shortcut)
        let node_start = create_dummy_node("start", "src/start.rs", 1);
        let node_mid1 = create_dummy_node("mid1", "src/mid1.rs", 1);
        let node_mid2 = create_dummy_node("mid2", "src/mid2.rs", 1);
        let node_dest = create_dummy_node("dest", "src/dest.rs", 1);
        let node_isolated = create_dummy_node("isolated", "src/isolated.rs", 1);

        let id_start = node_start.id;
        let id_mid1 = node_mid1.id;
        let id_mid2 = node_mid2.id;
        let id_dest = node_dest.id;
        let id_isolated = node_isolated.id;

        graph.upsert_symbol(node_start);
        graph.upsert_symbol(node_mid1);
        graph.upsert_symbol(node_mid2);
        graph.upsert_symbol(node_dest);
        graph.upsert_symbol(node_isolated);

        let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
        graph
            .add_edge(id_start, id_mid1, edge.clone())
            .expect("add start->mid1");
        graph
            .add_edge(id_mid1, id_mid2, edge.clone())
            .expect("add mid1->mid2");
        graph
            .add_edge(id_mid2, id_dest, edge.clone())
            .expect("add mid2->dest");
        graph
            .add_edge(id_start, id_dest, edge)
            .expect("add start->dest shortcut");

        let path = graph
            .find_shortest_path(&id_start, &id_dest)
            .expect("path exists");
        assert_eq!(path.len(), 2);
        assert_eq!(path[0].id, id_start);
        assert_eq!(path[1].id, id_dest);

        // Path to disconnected node
        assert!(graph.find_shortest_path(&id_start, &id_isolated).is_none());
    }
}
