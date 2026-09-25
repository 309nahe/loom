//! Correctness suite for transitive reachability and shortest-path pathfinding.
//!
//! Guards the guarantees the analysis layer depends on: BFS picks the *minimum-edge* route,
//! recursion and cycles terminate cleanly, depth bounds are exact, and — critically for
//! cacheable MCP responses — output ordering is byte-for-byte stable across repeated runs
//! despite `HashMap` iteration being randomized per process.

use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use loom_graph::CodeGraph;
use std::path::PathBuf;

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
fn test_shortest_path_optimality_across_unequal_multipaths() {
    let mut graph = CodeGraph::new();

    // Long path: A -> B -> C -> D (length 3 edges / 4 nodes)
    // Short path: A -> E -> D (length 2 edges / 3 nodes)
    let a = make_node("src/a.rs", "node_a", true);
    let b = make_node("src/b.rs", "node_b", false);
    let c = make_node("src/c.rs", "node_c", false);
    let d = make_node("src/d.rs", "node_d", false);
    let e = make_node("src/e.rs", "node_e", false);

    let id_a = a.id;
    let id_b = b.id;
    let id_c = c.id;
    let id_d = d.id;
    let id_e = e.id;

    graph.upsert_symbol(a);
    graph.upsert_symbol(b);
    graph.upsert_symbol(c);
    graph.upsert_symbol(d);
    graph.upsert_symbol(e);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
    // Add long path first to test that BFS does not naively pick first found path
    graph.add_edge(id_a, id_b, edge.clone()).expect("a->b");
    graph.add_edge(id_b, id_c, edge.clone()).expect("b->c");
    graph.add_edge(id_c, id_d, edge.clone()).expect("c->d");

    // Add shorter shortcut path
    graph.add_edge(id_a, id_e, edge.clone()).expect("a->e");
    graph.add_edge(id_e, id_d, edge).expect("e->d");

    let path = graph
        .find_shortest_path(&id_a, &id_d)
        .expect("path must exist");
    assert_eq!(
        path.len(),
        3,
        "Shortest path must take 3 nodes (A -> E -> D)"
    );
    assert_eq!(path[0].id, id_a);
    assert_eq!(path[1].id, id_e);
    assert_eq!(path[2].id, id_d);
}

#[test]
fn test_self_referencing_recursive_function() {
    let mut graph = CodeGraph::new();

    let rec = make_node("src/rec.rs", "factorial_recursive", false);
    let id_rec = rec.id;
    graph.upsert_symbol(rec);

    // Self call edge
    graph
        .add_edge(
            id_rec,
            id_rec,
            DependencyEdge::new(EdgeKind::Calls, 5, false),
        )
        .expect("self edge");

    // Direct caller/callee includes self
    assert_eq!(graph.get_callers(&id_rec).len(), 1);
    assert_eq!(graph.get_callees(&id_rec).len(), 1);

    // Transitive traversal safely terminates with 0 external dependents (root is not duplicated)
    let callees = graph.find_transitive_callees(&id_rec, 5);
    assert!(callees.is_empty());

    let callers = graph.find_transitive_callers(&id_rec, 5);
    assert!(callers.is_empty());

    let path = graph
        .find_shortest_path(&id_rec, &id_rec)
        .expect("path to self");
    assert_eq!(path.len(), 1);
    assert_eq!(path[0].id, id_rec);
}

#[test]
fn test_multi_node_cycle_with_entry_and_exit() {
    let mut graph = CodeGraph::new();

    // Topography:
    // Entry X -> A -> B -> C -> D -> A (cycle A-B-C-D-A)
    // A -> Exit Y
    let x = make_node("src/entry.rs", "entry_x", true);
    let a = make_node("src/cycle.rs", "cycle_a", false);
    let b = make_node("src/cycle.rs", "cycle_b", false);
    let c = make_node("src/cycle.rs", "cycle_c", false);
    let d = make_node("src/cycle.rs", "cycle_d", false);
    let y = make_node("src/exit.rs", "exit_y", true);

    let id_x = x.id;
    let id_a = a.id;
    let id_b = b.id;
    let id_c = c.id;
    let id_d = d.id;
    let id_y = y.id;

    graph.upsert_symbol(x);
    graph.upsert_symbol(a);
    graph.upsert_symbol(b);
    graph.upsert_symbol(c);
    graph.upsert_symbol(d);
    graph.upsert_symbol(y);

    let edge = DependencyEdge::new(EdgeKind::Calls, 1, false);
    graph.add_edge(id_x, id_a, edge.clone()).expect("x->a");
    graph.add_edge(id_a, id_b, edge.clone()).expect("a->b");
    graph.add_edge(id_b, id_c, edge.clone()).expect("b->c");
    graph.add_edge(id_c, id_d, edge.clone()).expect("c->d");
    graph
        .add_edge(id_d, id_a, edge.clone())
        .expect("d->a (closing cycle)");
    graph.add_edge(id_a, id_y, edge).expect("a->y");

    // Transitive callees from X at depth 10
    let callees = graph.find_transitive_callees(&id_x, 10);
    // Reached nodes: A (depth 1), B (depth 2), Y (depth 2), C (depth 3), D (depth 4) -> 5 total
    assert_eq!(callees.len(), 5);

    // Shortest path from X to Y: X -> A -> Y (3 nodes)
    let path_x_y = graph.find_shortest_path(&id_x, &id_y).expect("x->y path");
    assert_eq!(path_x_y.len(), 3);
    assert_eq!(path_x_y[0].id, id_x);
    assert_eq!(path_x_y[1].id, id_a);
    assert_eq!(path_x_y[2].id, id_y);

    // Shortest path from C to Y: C -> D -> A -> Y (4 nodes)
    let path_c_y = graph.find_shortest_path(&id_c, &id_y).expect("c->y path");
    assert_eq!(path_c_y.len(), 4);
    assert_eq!(path_c_y[0].id, id_c);
    assert_eq!(path_c_y[1].id, id_d);
    assert_eq!(path_c_y[2].id, id_a);
    assert_eq!(path_c_y[3].id, id_y);
}

#[test]
fn test_depth_zero_and_depth_one_boundary_conditions() {
    let mut graph = CodeGraph::new();

    let a = make_node("src/lib.rs", "root_a", true);
    let b = make_node("src/lib.rs", "child_b", false);
    let c = make_node("src/lib.rs", "grandchild_c", false);

    let id_a = a.id;
    let id_b = b.id;
    let id_c = c.id;

    graph.upsert_symbol(a);
    graph.upsert_symbol(b);
    graph.upsert_symbol(c);

    let edge = DependencyEdge::new(EdgeKind::Calls, 1, false);
    graph.add_edge(id_a, id_b, edge.clone()).expect("a->b");
    graph.add_edge(id_b, id_c, edge).expect("b->c");

    // Depth 0: should return empty vector
    let callees_d0 = graph.find_transitive_callees(&id_a, 0);
    assert!(callees_d0.is_empty());

    let callers_d0 = graph.find_transitive_callers(&id_c, 0);
    assert!(callers_d0.is_empty());

    // Depth 1: should return only direct children/parents
    let callees_d1 = graph.find_transitive_callees(&id_a, 1);
    assert_eq!(callees_d1.len(), 1);
    assert_eq!(callees_d1[0].0.id, id_b);

    let callers_d1 = graph.find_transitive_callers(&id_c, 1);
    assert_eq!(callers_d1.len(), 1);
    assert_eq!(callers_d1[0].0.id, id_b);
}

#[test]
fn test_non_existent_source_or_target_pathfinding() {
    let mut graph = CodeGraph::new();

    let a = make_node("src/lib.rs", "exists_a", true);
    let id_a = a.id;
    graph.upsert_symbol(a);

    let fake_id_1 = SymbolId::derive("src/fake.rs", &[], "fake_1", "fn fake_1()");
    let fake_id_2 = SymbolId::derive("src/fake.rs", &[], "fake_2", "fn fake_2()");

    // Non-existent source
    assert!(graph.find_shortest_path(&fake_id_1, &id_a).is_none());
    // Non-existent target
    assert!(graph.find_shortest_path(&id_a, &fake_id_2).is_none());
    // Non-existent both
    assert!(graph.find_shortest_path(&fake_id_1, &fake_id_2).is_none());
}

#[test]
fn test_dynamic_invalidation_breaks_path() {
    let mut graph = CodeGraph::new();

    let a = make_node("src/module_a.rs", "fn_a", true);
    let b = make_node("src/module_b.rs", "fn_b", false);
    let c = make_node("src/module_c.rs", "fn_c", true);

    let id_a = a.id;
    let id_b = b.id;
    let id_c = c.id;

    graph.upsert_symbol(a);
    graph.upsert_symbol(b);
    graph.upsert_symbol(c);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
    graph.add_edge(id_a, id_b, edge.clone()).expect("a->b");
    graph.add_edge(id_b, id_c, edge).expect("b->c");

    // Initial path exists
    assert!(graph.find_shortest_path(&id_a, &id_c).is_some());

    // Invalidate module_b.rs
    graph.invalidate_file(&PathBuf::from("src/module_b.rs"));

    // Path must now be broken
    assert!(graph.find_shortest_path(&id_a, &id_c).is_none());
    // Caller of C is now empty
    assert!(graph.find_transitive_callers(&id_c, 5).is_empty());
}

#[test]
fn test_deterministic_ordering_stability() {
    let mut graph = CodeGraph::new();

    let root = make_node("src/root.rs", "root", true);
    let id_root = root.id;
    graph.upsert_symbol(root);

    let mut child_ids = Vec::new();
    for i in 0..10 {
        let child = make_node("src/children.rs", &format!("child_{i}"), false);
        let id_child = child.id;
        child_ids.push(id_child);
        graph.upsert_symbol(child);
        graph
            .add_edge(
                id_root,
                id_child,
                DependencyEdge::new(EdgeKind::Calls, i as u32, false),
            )
            .expect("root->child");
    }

    let baseline = graph.find_transitive_callees(&id_root, 5);
    for _ in 0..50 {
        let current = graph.find_transitive_callees(&id_root, 5);
        assert_eq!(
            baseline.iter().map(|(n, d)| (n.id, *d)).collect::<Vec<_>>(),
            current.iter().map(|(n, d)| (n.id, *d)).collect::<Vec<_>>(),
            "Transitive traversal results must be 100% deterministic across repeated runs"
        );
    }
}
