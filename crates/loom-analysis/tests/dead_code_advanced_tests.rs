use loom_analysis::dead_code::{DeadCodeDetector, DeadSymbolReason};
use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use loom_graph::CodeGraph;

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
fn test_multiple_distinct_entrypoints_reachability() {
    let mut graph = CodeGraph::new();

    // Entrypoint 1: main.rs -> worker
    let main_fn = make_node("src/main.rs", "main", true);
    let worker_fn = make_node("src/worker.rs", "process_queue", false);

    // Entrypoint 2: cli.rs -> parser
    let cli_fn = make_node("src/cli.rs", "run", true);
    let parser_fn = make_node("src/parser.rs", "parse_flags", false);

    // Entrypoint 3: routes.rs -> auth
    let route_fn = make_node("src/routes.rs", "handler", true);
    let auth_fn = make_node("src/auth.rs", "verify_token", false);

    // Dead orphan
    let dead_fn = make_node("src/unused.rs", "unused_calculator", false);

    let id_main = main_fn.id;
    let id_worker = worker_fn.id;
    let id_cli = cli_fn.id;
    let id_parser = parser_fn.id;
    let id_route = route_fn.id;
    let id_auth = auth_fn.id;
    let id_dead = dead_fn.id;

    graph.upsert_symbol(main_fn);
    graph.upsert_symbol(worker_fn);
    graph.upsert_symbol(cli_fn);
    graph.upsert_symbol(parser_fn);
    graph.upsert_symbol(route_fn);
    graph.upsert_symbol(auth_fn);
    graph.upsert_symbol(dead_fn);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
    graph
        .add_edge(id_main, id_worker, edge.clone())
        .expect("main->worker");
    graph
        .add_edge(id_cli, id_parser, edge.clone())
        .expect("cli->parser");
    graph
        .add_edge(id_route, id_auth, edge)
        .expect("route->auth");

    let detector = DeadCodeDetector::new(&graph);
    let report = detector.find_dead_symbols();

    assert_eq!(report.total_dead_count, 1);
    assert_eq!(report.dead_symbols[0].symbol.id, id_dead);
    assert_eq!(
        report.dead_symbols[0].reason,
        DeadSymbolReason::UnreferencedInternalSymbol
    );
}

#[test]
fn test_dead_function_calling_live_function() {
    let mut graph = CodeGraph::new();

    let main_fn = make_node("src/main.rs", "main", true);
    let shared_util = make_node("src/util.rs", "format_string", false);
    let dead_fn = make_node("src/dead.rs", "deprecated_printer", false);

    let id_main = main_fn.id;
    let id_shared = shared_util.id;
    let id_dead = dead_fn.id;

    graph.upsert_symbol(main_fn);
    graph.upsert_symbol(shared_util);
    graph.upsert_symbol(dead_fn);

    let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);
    // main calls shared_util
    graph
        .add_edge(id_main, id_shared, edge.clone())
        .expect("main->shared");
    // dead function ALSO calls shared_util
    graph
        .add_edge(id_dead, id_shared, edge)
        .expect("dead->shared");

    let detector = DeadCodeDetector::new(&graph);
    let report = detector.find_dead_symbols();

    // shared_util is reachable from main, so it must NOT be marked dead
    assert!(!report.dead_symbols.iter().any(|d| d.symbol.id == id_shared));
    // dead_fn must be marked dead
    assert_eq!(report.total_dead_count, 1);
    assert_eq!(report.dead_symbols[0].symbol.id, id_dead);
}

#[test]
fn test_dead_tree_with_nested_branches() {
    let mut graph = CodeGraph::new();

    // Live entrypoint
    let live_entry = make_node("src/main.rs", "main", true);
    graph.upsert_symbol(live_entry);

    // Dead tree:
    // D0 -> D1 -> D3
    // D0 -> D2
    let d0 = make_node("src/old.rs", "dead_root", false);
    let d1 = make_node("src/old.rs", "dead_child_1", false);
    let d2 = make_node("src/old.rs", "dead_child_2", false);
    let d3 = make_node("src/old.rs", "dead_grandchild", false);

    let id_d0 = d0.id;
    let id_d1 = d1.id;
    let id_d2 = d2.id;
    let id_d3 = d3.id;

    graph.upsert_symbol(d0);
    graph.upsert_symbol(d1);
    graph.upsert_symbol(d2);
    graph.upsert_symbol(d3);

    let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);
    graph.add_edge(id_d0, id_d1, edge.clone()).expect("d0->d1");
    graph.add_edge(id_d0, id_d2, edge.clone()).expect("d0->d2");
    graph.add_edge(id_d1, id_d3, edge).expect("d1->d3");

    let detector = DeadCodeDetector::new(&graph);
    let report = detector.find_dead_symbols();

    assert_eq!(report.total_dead_count, 4);
    let dead_ids: Vec<SymbolId> = report.dead_symbols.iter().map(|d| d.symbol.id).collect();
    assert!(dead_ids.contains(&id_d0));
    assert!(dead_ids.contains(&id_d1));
    assert!(dead_ids.contains(&id_d2));
    assert!(dead_ids.contains(&id_d3));
}

#[test]
fn test_zero_dead_code_in_fully_reachable_repository() {
    let mut graph = CodeGraph::new();

    let entry = make_node("src/main.rs", "main", true);
    let id_entry = entry.id;
    graph.upsert_symbol(entry);

    let mut prev_id = id_entry;
    for i in 0..10 {
        let node = make_node("src/pipeline.rs", &format!("step_{i}"), false);
        let id_node = node.id;
        graph.upsert_symbol(node);
        graph
            .add_edge(
                prev_id,
                id_node,
                DependencyEdge::new(EdgeKind::Calls, 10, false),
            )
            .expect("step edge");
        prev_id = id_node;
    }

    let detector = DeadCodeDetector::new(&graph);
    let report = detector.find_dead_symbols();
    assert_eq!(report.total_dead_count, 0);
    assert!(report.dead_symbols.is_empty());
}

#[test]
fn test_dynamic_reanalysis_after_wiring_dead_symbol_to_entrypoint() {
    let mut graph = CodeGraph::new();

    let main_fn = make_node("src/main.rs", "main", true);
    let orphan = make_node("src/feature.rs", "new_feature_engine", false);

    let id_main = main_fn.id;
    let id_orphan = orphan.id;

    graph.upsert_symbol(main_fn);
    graph.upsert_symbol(orphan);

    // Initial check: orphan is dead code
    let det1 = DeadCodeDetector::new(&graph);
    let rep1 = det1.find_dead_symbols();
    assert_eq!(rep1.total_dead_count, 1);
    assert_eq!(rep1.dead_symbols[0].symbol.id, id_orphan);

    // Wire main -> orphan
    graph
        .add_edge(
            id_main,
            id_orphan,
            DependencyEdge::new(EdgeKind::Calls, 1, false),
        )
        .expect("wire edge");

    // Recomputed check: 0 dead code
    let det2 = DeadCodeDetector::new(&graph);
    let rep2 = det2.find_dead_symbols();
    assert_eq!(rep2.total_dead_count, 0);
}
