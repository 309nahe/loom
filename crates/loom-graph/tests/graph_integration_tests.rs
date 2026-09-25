//! Integration tests for `CodeGraph` consistency under mutation and scale.
//!
//! Focus areas that unit tests cannot cover in isolation:
//! - **Swap-removal safety**: repeatedly invalidating files must never desynchronize the
//!   `symbol_to_node` index table from the underlying petgraph topology.
//! - **Edge hygiene**: removing a file must also drop its incident edges, never leaving
//!   dangling call references.
//! - **Cyclic / multi-edge topologies**: recursion and diamond shapes must traverse once per
//!   node without infinite loops.

use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use loom_graph::CodeGraph;
use std::path::PathBuf;

fn make_symbol(file: &str, name: &str, line: u32) -> SymbolNode {
    let id = SymbolId::derive(file, &[], name, &format!("fn {name}()"));
    SymbolNode::new(
        id,
        name,
        SymbolKind::Function,
        file,
        (0, 50),
        (line, line + 5),
        None,
        format!("fn {name}()"),
        true,
        1,
    )
}

#[test]
fn test_large_graph_scaling_and_consistency() {
    let mut graph = CodeGraph::new();
    let num_files: usize = 20;
    let symbols_per_file: usize = 25; // 500 nodes

    let mut all_ids = Vec::new();

    for f in 0..num_files {
        let file_path = format!("src/module_{f}.rs");
        for s in 0..symbols_per_file {
            let symbol_name = format!("func_{f}_{s}");
            let node = make_symbol(&file_path, &symbol_name, (s * 10) as u32);
            let id = node.id;
            all_ids.push(id);
            graph.upsert_symbol(node);
        }
    }

    assert_eq!(graph.node_count(), num_files * symbols_per_file);
    assert_eq!(graph.file_count(), num_files);

    // Create call edges between sequential symbols
    for i in 0..(all_ids.len() - 1) {
        let edge = DependencyEdge::new(EdgeKind::Calls, 42, false);
        graph
            .add_edge(all_ids[i], all_ids[i + 1], edge)
            .expect("add edge");
    }

    assert_eq!(graph.edge_count(), all_ids.len() - 1);

    // Query caller / callee across chain
    let mid_id = all_ids[10];
    let callers = graph.get_callers(&mid_id);
    let callees = graph.get_callees(&mid_id);

    assert_eq!(callers.len(), 1);
    assert_eq!(callers[0].0.id, all_ids[9]);

    assert_eq!(callees.len(), 1);
    assert_eq!(callees[0].0.id, all_ids[11]);
}

#[test]
fn test_incremental_invalidation_stress() {
    let mut graph = CodeGraph::new();

    // Create 10 files with 5 symbols each
    for f in 0..10 {
        let file_path = format!("src/file_{f}.rs");
        for s in 0..5 {
            let node = make_symbol(&file_path, &format!("sym_{f}_{s}"), s * 10);
            graph.upsert_symbol(node);
        }
    }

    assert_eq!(graph.node_count(), 50);
    assert_eq!(graph.file_count(), 10);

    // Progressively invalidate and replace files
    for f in 0..10 {
        let file_path = PathBuf::from(format!("src/file_{f}.rs"));

        // 1. Invalidate
        let removed = graph.invalidate_file(&file_path);
        assert_eq!(removed.len(), 5);
        assert_eq!(graph.file_count(), 9);

        // Verify symbols no longer exist
        for old_node in &removed {
            assert!(!graph.contains_symbol(&old_node.id));
            assert!(graph.get_symbol(&old_node.id).is_none());
        }

        // 2. Re-insert updated version with 3 new symbols
        for s in 0..3 {
            let new_node = make_symbol(
                &file_path.to_string_lossy(),
                &format!("new_sym_{f}_{s}"),
                s * 20,
            );
            graph.upsert_symbol(new_node);
        }

        assert_eq!(graph.file_count(), 10);
    }

    // Total nodes should now be 10 files * 3 symbols = 30 nodes
    assert_eq!(graph.node_count(), 30);
    assert_eq!(graph.file_count(), 10);

    // Verify all 30 nodes in symbol_to_node match their underlying Petgraph weights
    for &node_idx in graph.symbol_to_node.values() {
        assert!(graph.graph.node_weight(node_idx).is_some());
    }
}

#[test]
fn test_cyclic_dependencies_and_multiple_edge_kinds() {
    let mut graph = CodeGraph::new();
    let node_a = make_symbol("src/a.rs", "node_a", 10);
    let node_b = make_symbol("src/b.rs", "node_b", 20);

    let id_a = node_a.id;
    let id_b = node_b.id;

    graph.upsert_symbol(node_a);
    graph.upsert_symbol(node_b);

    // A calls B
    graph
        .add_edge(id_a, id_b, DependencyEdge::new(EdgeKind::Calls, 15, false))
        .expect("add A->B edge");

    // B calls A (cycle)
    graph
        .add_edge(id_b, id_a, DependencyEdge::new(EdgeKind::Calls, 25, false))
        .expect("add B->A edge");

    // A also instantiates B
    graph
        .add_edge(
            id_a,
            id_b,
            DependencyEdge::new(EdgeKind::Instantiates, 16, true),
        )
        .expect("add A instantiates B edge");

    assert_eq!(graph.edge_count(), 3);

    let callers_of_a = graph.get_callers(&id_a);
    assert_eq!(callers_of_a.len(), 1);
    assert_eq!(callers_of_a[0].0.id, id_b);

    let callers_of_b = graph.get_callers(&id_b);
    assert_eq!(callers_of_b.len(), 1); // Only calls, not instantiates
    assert_eq!(callers_of_b[0].0.id, id_a);
}

#[test]
fn test_complex_diamond_and_multi_layer_reachability() {
    let mut graph = CodeGraph::new();

    // Diamond:
    //      Entry
    //      /   \
    //   Left   Right
    //      \   /
    //     Target
    //        |
    //     Database
    let entry = make_symbol("src/entry.rs", "entrypoint", 1);
    let left = make_symbol("src/left.rs", "left_handler", 1);
    let right = make_symbol("src/right.rs", "right_handler", 1);
    let target = make_symbol("src/target.rs", "core_logic", 1);
    let db = make_symbol("src/db.rs", "db_query", 1);

    let id_entry = entry.id;
    let id_left = left.id;
    let id_right = right.id;
    let id_target = target.id;
    let id_db = db.id;

    graph.upsert_symbol(entry);
    graph.upsert_symbol(left);
    graph.upsert_symbol(right);
    graph.upsert_symbol(target);
    graph.upsert_symbol(db);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
    graph
        .add_edge(id_entry, id_left, edge.clone())
        .expect("entry->left");
    graph
        .add_edge(id_entry, id_right, edge.clone())
        .expect("entry->right");
    graph
        .add_edge(id_left, id_target, edge.clone())
        .expect("left->target");
    graph
        .add_edge(id_right, id_target, edge.clone())
        .expect("right->target");
    graph.add_edge(id_target, id_db, edge).expect("target->db");

    // Transitive callers of Target (should be left, right, and entry)
    let callers = graph.find_transitive_callers(&id_target, 5);
    assert_eq!(callers.len(), 3);
    let caller_names: Vec<&str> = callers.iter().map(|(s, _)| s.name.as_str()).collect();
    assert!(caller_names.contains(&"left_handler"));
    assert!(caller_names.contains(&"right_handler"));
    assert!(caller_names.contains(&"entrypoint"));

    // Transitive callees of Entry
    let callees = graph.find_transitive_callees(&id_entry, 5);
    assert_eq!(callees.len(), 4);

    // Shortest path from entry to DB (length = 4 nodes: entry -> left/right -> target -> db)
    let path = graph
        .find_shortest_path(&id_entry, &id_db)
        .expect("path exists");
    assert_eq!(path.len(), 4);
    assert_eq!(path[0].id, id_entry);
    assert_eq!(path[3].id, id_db);
}
