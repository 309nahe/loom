use std::time::Instant;

use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use loom_graph::CodeGraph;

fn helper_create_symbol(
    graph: &mut CodeGraph,
    file: &str,
    name: &str,
    is_exported: bool,
) -> SymbolId {
    let id = SymbolId::derive(file, &[], name, &format!("fn {name}()"));
    let node = SymbolNode::new(
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
    );
    graph.upsert_symbol(node);
    id
}

#[test]
fn test_diamond_dependency_pattern() {
    let mut graph = CodeGraph::new();

    // A -> B -> D
    // A -> C -> D
    let a = helper_create_symbol(&mut graph, "src/lib.rs", "a", true);
    let b = helper_create_symbol(&mut graph, "src/b.rs", "b", false);
    let c = helper_create_symbol(&mut graph, "src/c.rs", "c", false);
    let d = helper_create_symbol(&mut graph, "src/d.rs", "d", false);

    let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);

    graph.add_edge(a, b, edge.clone()).expect("edge a->b");
    graph.add_edge(a, c, edge.clone()).expect("edge a->c");
    graph.add_edge(b, d, edge.clone()).expect("edge b->d");
    graph.add_edge(c, d, edge).expect("edge c->d");

    let callees_a = graph.find_transitive_callees(&a, 3);
    assert_eq!(callees_a.len(), 3);
    let callee_ids: Vec<SymbolId> = callees_a.iter().map(|(node, _)| node.id).collect();
    assert!(callee_ids.contains(&b));
    assert!(callee_ids.contains(&c));
    assert!(callee_ids.contains(&d));

    let callers_d = graph.find_transitive_callers(&d, 3);
    assert_eq!(callers_d.len(), 3);
    let caller_ids: Vec<SymbolId> = callers_d.iter().map(|(node, _)| node.id).collect();
    assert!(caller_ids.contains(&b));
    assert!(caller_ids.contains(&c));
    assert!(caller_ids.contains(&a));

    let path_a_d = graph.find_shortest_path(&a, &d).expect("path must exist");
    assert_eq!(path_a_d.len(), 3); // a -> b (or c) -> d
    assert_eq!(path_a_d[0].id, a);
    assert_eq!(path_a_d[2].id, d);
}

#[test]
fn test_dense_cyclic_loops() {
    let mut graph = CodeGraph::new();

    // A <-> B <-> C <-> A (fully cyclic triangle)
    let a = helper_create_symbol(&mut graph, "src/lib.rs", "cycle_a", false);
    let b = helper_create_symbol(&mut graph, "src/lib.rs", "cycle_b", false);
    let c = helper_create_symbol(&mut graph, "src/lib.rs", "cycle_c", false);

    let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);

    graph.add_edge(a, b, edge.clone()).expect("a->b");
    graph.add_edge(b, a, edge.clone()).expect("b->a");
    graph.add_edge(b, c, edge.clone()).expect("b->c");
    graph.add_edge(c, b, edge.clone()).expect("c->b");
    graph.add_edge(c, a, edge.clone()).expect("c->a");
    graph.add_edge(a, c, edge).expect("a->c");

    let callees = graph.find_transitive_callees(&a, 10);
    assert_eq!(callees.len(), 2); // only b and c, no infinite loop or duplicates

    let callers = graph.find_transitive_callers(&a, 10);
    assert_eq!(callers.len(), 2);
}

#[test]
fn test_deep_linear_call_chain_n100() {
    let mut graph = CodeGraph::new();
    let mut nodes = Vec::with_capacity(100);

    for i in 0..100 {
        let sym = helper_create_symbol(&mut graph, "src/chain.rs", &format!("node_{i}"), false);
        nodes.push(sym);
    }

    for i in 0..99 {
        graph
            .add_edge(
                nodes[i],
                nodes[i + 1],
                DependencyEdge::new(EdgeKind::Calls, 10, false),
            )
            .expect("chain edge");
    }

    // Reachability depth limit 10
    let callees_depth_10 = graph.find_transitive_callees(&nodes[0], 10);
    assert_eq!(callees_depth_10.len(), 10);

    // Full reachability depth 100
    let callees_depth_100 = graph.find_transitive_callees(&nodes[0], 100);
    assert_eq!(callees_depth_100.len(), 99);

    let callers_from_end = graph.find_transitive_callers(&nodes[99], 100);
    assert_eq!(callers_from_end.len(), 99);

    let shortest_path = graph
        .find_shortest_path(&nodes[0], &nodes[99])
        .expect("path should exist");
    assert_eq!(shortest_path.len(), 100);
}

#[test]
fn test_disconnected_islands_forest() {
    let mut graph = CodeGraph::new();

    // Island 1: I1_A -> I1_B
    let i1_a = helper_create_symbol(&mut graph, "src/i1.rs", "i1_a", true);
    let i1_b = helper_create_symbol(&mut graph, "src/i1.rs", "i1_b", false);
    graph
        .add_edge(i1_a, i1_b, DependencyEdge::new(EdgeKind::Calls, 1, false))
        .expect("i1 edge");

    // Island 2: I2_A -> I2_B
    let i2_a = helper_create_symbol(&mut graph, "src/i2.rs", "i2_a", true);
    let i2_b = helper_create_symbol(&mut graph, "src/i2.rs", "i2_b", false);
    graph
        .add_edge(i2_a, i2_b, DependencyEdge::new(EdgeKind::Calls, 1, false))
        .expect("i2 edge");

    // No cross island reachability
    let callees_1 = graph.find_transitive_callees(&i1_a, 5);
    assert_eq!(callees_1.len(), 1);
    assert_eq!(callees_1[0].0.id, i1_b);

    assert!(graph.find_shortest_path(&i1_a, &i2_a).is_none());
    assert!(graph.find_shortest_path(&i1_a, &i2_b).is_none());
}

#[test]
fn test_synthetic_scale_10k_latency_assertion() {
    let mut graph = CodeGraph::new();
    let node_count = 10_000;
    let mut symbol_ids = Vec::with_capacity(node_count);

    for i in 0..node_count {
        let file_path = format!("src/module_{}.rs", i / 100);
        let name = format!("fn_{i}");
        let id = SymbolId::derive(&file_path, &["scale"], &name, "fn()");
        let node = SymbolNode::new(
            id,
            &name,
            SymbolKind::Function,
            &file_path,
            (0, 50),
            (1, 10),
            None,
            "fn()",
            true,
            1,
        );
        graph.upsert_symbol(node);
        symbol_ids.push(id);
    }

    for (i, &from_id) in symbol_ids.iter().enumerate() {
        for b in 1..=4 {
            let target_idx = i * 4 + b;
            if target_idx < node_count {
                let _ = graph.add_edge(
                    from_id,
                    symbol_ids[target_idx],
                    DependencyEdge::new(EdgeKind::Calls, 10, false),
                );
            }
        }
    }

    let root_id = symbol_ids[0];
    let start = Instant::now();
    let results = graph.find_transitive_callees(&root_id, 5);
    let elapsed = start.elapsed();

    assert!(!results.is_empty());
    // Sub-millisecond deterministic traversal, strictly < 2ms
    assert!(
        elapsed.as_millis() < 2,
        "Transitive traversal on 10k nodes took {:?}, must be < 2ms",
        elapsed
    );
}

#[test]
fn test_synthetic_scale_50k_latency_assertion() {
    let mut graph = CodeGraph::new();
    let node_count = 50_000;
    let mut symbol_ids = Vec::with_capacity(node_count);

    for i in 0..node_count {
        let file_path = format!("src/mod_{}.rs", i / 100);
        let name = format!("fn_{i}");
        let id = SymbolId::derive(&file_path, &["scale50k"], &name, "fn()");
        let node = SymbolNode::new(
            id,
            &name,
            SymbolKind::Function,
            &file_path,
            (0, 50),
            (1, 10),
            None,
            "fn()",
            true,
            1,
        );
        graph.upsert_symbol(node);
        symbol_ids.push(id);
    }

    for (i, &from_id) in symbol_ids.iter().enumerate() {
        for b in 1..=3 {
            let target_idx = i * 3 + b;
            if target_idx < node_count {
                let _ = graph.add_edge(
                    from_id,
                    symbol_ids[target_idx],
                    DependencyEdge::new(EdgeKind::Calls, 10, false),
                );
            }
        }
    }

    let root_id = symbol_ids[0];
    let start = Instant::now();
    let results = graph.find_transitive_callees(&root_id, 5);
    let elapsed = start.elapsed();

    assert!(!results.is_empty());
    assert!(
        elapsed.as_millis() < 2,
        "Transitive traversal on 50k nodes took {:?}, must be < 2ms",
        elapsed
    );
}
