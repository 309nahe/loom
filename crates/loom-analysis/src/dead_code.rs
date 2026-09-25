//! Dead code and orphan symbol analysis engine.

use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

use loom_core::symbol::SymbolNode;
use loom_graph::CodeGraph;
use petgraph::Direction;
use petgraph::visit::EdgeRef;

/// Diagnostic classification for why a symbol was flagged as dead code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeadSymbolReason {
    /// Internal symbol with zero incoming references/calls across the repository.
    UnreferencedInternalSymbol,
    /// Part of an isolated cyclic dependency cluster unreachable from entrypoints.
    IsolatedDeadCycle,
}

/// Detailed descriptor of a detected dead symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadSymbolDetail {
    /// The dead symbol node.
    pub symbol: SymbolNode,
    /// Reason explaining why it is considered dead.
    pub reason: DeadSymbolReason,
}

/// Diagnostic report containing all detected dead code candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeadCodeReport {
    /// List of dead symbols found.
    pub dead_symbols: Vec<DeadSymbolDetail>,
    /// Total count of dead symbols.
    pub total_dead_count: usize,
}

/// Dead code analysis engine.
#[derive(Debug, Clone)]
pub struct DeadCodeDetector<'a> {
    graph: &'a CodeGraph,
}

impl<'a> DeadCodeDetector<'a> {
    /// Creates a new detector instance over a `CodeGraph` reference.
    #[must_use]
    pub const fn new(graph: &'a CodeGraph) -> Self {
        Self { graph }
    }

    /// Scans the entire graph and returns all detected orphan and dead symbols.
    #[must_use]
    pub fn find_dead_symbols(&self) -> DeadCodeReport {
        let mut dead_symbols = Vec::new();

        // 1. Identify all "Root / Entrypoint / Public" symbols:
        // - is_exported == true
        // - Known entrypoint names ("main", "init", "run", "handler", etc.)
        // - Test symbols (they are test entrypoints)
        let mut entrypoint_indices = Vec::new();

        for &node_idx in self.graph.symbol_to_node.values() {
            if let Some(node) = self.graph.graph.node_weight(node_idx) {
                if is_entrypoint_or_exported(node) {
                    entrypoint_indices.push(node_idx);
                }
            }
        }

        // 2. Perform reachability traversal (BFS) starting from all entrypoints forward (callees)
        let mut reachable_indices = HashSet::new();
        let mut queue = VecDeque::new();

        for entry_idx in entrypoint_indices {
            if reachable_indices.insert(entry_idx) {
                queue.push_back(entry_idx);
            }
        }

        while let Some(curr_idx) = queue.pop_front() {
            for edge_ref in self
                .graph
                .graph
                .edges_directed(curr_idx, Direction::Outgoing)
            {
                let neighbor_idx = edge_ref.target();
                if reachable_indices.insert(neighbor_idx) {
                    queue.push_back(neighbor_idx);
                }
            }
        }

        // 3. Any node NOT in reachable_indices is candidate dead code
        for &node_idx in self.graph.symbol_to_node.values() {
            if !reachable_indices.contains(&node_idx) {
                if let Some(node) = self.graph.graph.node_weight(node_idx) {
                    // Check if it has 0 inbound edges or is in an isolated cycle
                    let in_degree = self
                        .graph
                        .graph
                        .edges_directed(node_idx, Direction::Incoming)
                        .count();
                    let reason = if in_degree == 0 {
                        DeadSymbolReason::UnreferencedInternalSymbol
                    } else {
                        DeadSymbolReason::IsolatedDeadCycle
                    };

                    dead_symbols.push(DeadSymbolDetail {
                        symbol: node.clone(),
                        reason,
                    });
                }
            }
        }

        // Sort by file_path and line number for determinism
        dead_symbols.sort_by(|a, b| {
            a.symbol
                .file_path
                .cmp(&b.symbol.file_path)
                .then(a.symbol.line_range.0.cmp(&b.symbol.line_range.0))
        });

        let total_dead_count = dead_symbols.len();
        DeadCodeReport {
            dead_symbols,
            total_dead_count,
        }
    }
}

/// Checks if a symbol represents an entrypoint or exported surface.
#[must_use]
pub fn is_entrypoint_or_exported(symbol: &SymbolNode) -> bool {
    if symbol.is_exported {
        return true;
    }

    let name = symbol.name.as_str();
    if name == "main" || name == "run" || name == "init" {
        return true;
    }

    crate::blast_radius::is_test_symbol(symbol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom_core::edge::{DependencyEdge, EdgeKind};
    use loom_core::id::SymbolId;
    use loom_core::symbol::SymbolKind;

    fn make_node(file: &str, name: &str, is_exported: bool) -> SymbolNode {
        let id = SymbolId::derive(file, &[], name, &format!("fn {name}()"));
        SymbolNode::new(
            id,
            name,
            SymbolKind::Function,
            file,
            (0, 50),
            (1, 10),
            None,
            format!("fn {name}()"),
            is_exported,
            1,
        )
    }

    #[test]
    fn test_orphan_and_dead_cycle_detection() {
        let mut graph = CodeGraph::new();

        // 1. Live path: main -> live_helper
        let main_fn = make_node("src/main.rs", "main", false);
        let live_helper = make_node("src/util.rs", "live_helper", false);

        // 2. Unreferenced orphan: dead_util (in_degree == 0, !is_exported)
        let dead_util = make_node("src/dead.rs", "dead_util", false);

        // 3. Isolated dead cycle: cycle_a <-> cycle_b (!is_exported, unreached from main)
        let cycle_a = make_node("src/cycle.rs", "cycle_a", false);
        let cycle_b = make_node("src/cycle.rs", "cycle_b", false);

        let id_main = main_fn.id;
        let id_live = live_helper.id;
        let id_ca = cycle_a.id;
        let id_cb = cycle_b.id;

        graph.upsert_symbol(main_fn);
        graph.upsert_symbol(live_helper);
        graph.upsert_symbol(dead_util);
        graph.upsert_symbol(cycle_a);
        graph.upsert_symbol(cycle_b);

        let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);
        graph
            .add_edge(id_main, id_live, edge.clone())
            .expect("add main->live");
        graph
            .add_edge(id_ca, id_cb, edge.clone())
            .expect("add ca->cb");
        graph.add_edge(id_cb, id_ca, edge).expect("add cb->ca");

        let detector = DeadCodeDetector::new(&graph);
        let report = detector.find_dead_symbols();

        assert_eq!(report.total_dead_count, 3);
        let dead_names: Vec<&str> = report
            .dead_symbols
            .iter()
            .map(|d| d.symbol.name.as_str())
            .collect();

        assert!(dead_names.contains(&"dead_util"));
        assert!(dead_names.contains(&"cycle_a"));
        assert!(dead_names.contains(&"cycle_b"));
        assert!(!dead_names.contains(&"main"));
        assert!(!dead_names.contains(&"live_helper"));

        let dead_util_detail = report
            .dead_symbols
            .iter()
            .find(|d| d.symbol.name == "dead_util")
            .unwrap();
        assert_eq!(
            dead_util_detail.reason,
            DeadSymbolReason::UnreferencedInternalSymbol
        );

        let cycle_a_detail = report
            .dead_symbols
            .iter()
            .find(|d| d.symbol.name == "cycle_a")
            .unwrap();
        assert_eq!(cycle_a_detail.reason, DeadSymbolReason::IsolatedDeadCycle);
    }
}
