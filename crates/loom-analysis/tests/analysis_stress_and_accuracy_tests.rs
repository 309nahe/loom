use loom_analysis::blast_radius::{BlastRadiusCalculator, RiskLevel};
use loom_analysis::dead_code::{DeadCodeDetector, DeadSymbolReason};
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
fn test_blast_radius_exact_risk_scoring_formula() {
    let mut graph = CodeGraph::new();

    // Target node T (Private function)
    let t = helper_create_symbol(&mut graph, "src/lib.rs", "target", false);

    // Direct caller C1 (Private, weight=1.0, depth=1)
    let c1 = helper_create_symbol(&mut graph, "src/internal.rs", "caller1", false);
    // Direct caller C2 (Public, weight=3.0, depth=1)
    let c2 = helper_create_symbol(&mut graph, "src/api.rs", "caller2_public", true);
    // 2-level caller C3 (Public, weight=3.0, depth=2) -> calls C1
    let c3 = helper_create_symbol(&mut graph, "src/api.rs", "caller3_public", true);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);

    graph.add_edge(c1, t, edge.clone()).expect("c1->t");
    graph.add_edge(c2, t, edge.clone()).expect("c2->t");
    graph.add_edge(c3, c1, edge).expect("c3->c1");

    let calculator = BlastRadiusCalculator::new(&graph);
    let report = calculator.calculate(&t, 5).expect("report should generate");

    assert_eq!(report.target_symbol.id, t);
    assert_eq!(report.total_affected_symbols, 3);
    assert_eq!(report.direct_callers.len(), 2);
    assert_eq!(report.transitive_callers.len(), 1);

    // Theoretical score calculation:
    // C1: depth 1, private => 0.75 * 1.0 * 2.0 = 1.5
    // C2: depth 1, public  => 0.75 * 3.0 * 2.0 = 4.5
    // C3: depth 2, public  => (0.75)^2 * 3.0 * 2.0 = 0.5625 * 6.0 = 3.375
    // Expected sum: 1.5 + 4.5 + 3.375 = 9.375
    let expected_score = 9.375_f64;
    assert!(
        (report.risk_score_raw - expected_score).abs() < 1e-6,
        "Expected risk score {}, got {}",
        expected_score,
        report.risk_score_raw
    );

    // Under 9.375 score with public API boundary impacted and 3 callers, risk level is High
    assert_eq!(report.risk_level, RiskLevel::High);
}

#[test]
fn test_blast_radius_high_risk_and_critical_boundaries() {
    let mut graph = CodeGraph::new();

    let t = helper_create_symbol(&mut graph, "src/db.rs", "query_exec", false);

    // Create 3 public API callers at depth 1
    for i in 0..3 {
        let api_fn = helper_create_symbol(&mut graph, "src/api.rs", &format!("endpoint_{i}"), true);
        graph
            .add_edge(api_fn, t, DependencyEdge::new(EdgeKind::Calls, 10, false))
            .expect("api->t");
    }

    let calculator = BlastRadiusCalculator::new(&graph);
    let report = calculator.calculate(&t, 5).expect("report should generate");
    // Score = 3 * (3.0 * 0.75 * 2.0) = 13.5 (>= 8.0, total_affected=3 => High)
    assert_eq!(report.risk_level, RiskLevel::High);
    assert_eq!(report.direct_callers.len(), 3);
}

#[test]
fn test_blast_radius_associated_tests_mapping() {
    let mut graph = CodeGraph::new();

    let target = helper_create_symbol(&mut graph, "src/math.rs", "add", true);

    let test_direct =
        helper_create_symbol(&mut graph, "tests/math_test.rs", "test_add_direct", false);

    let helper_fn = helper_create_symbol(&mut graph, "src/helpers.rs", "math_helper", false);

    let test_indirect =
        helper_create_symbol(&mut graph, "tests/math_test.rs", "test_add_indirect", false);

    let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);

    // test_direct -> target
    graph
        .add_edge(test_direct, target, edge.clone())
        .expect("test_direct->target");
    // test_indirect -> helper_fn -> target
    graph
        .add_edge(helper_fn, target, edge.clone())
        .expect("helper_fn->target");
    graph
        .add_edge(test_indirect, helper_fn, edge)
        .expect("test_indirect->helper");

    let calculator = BlastRadiusCalculator::new(&graph);
    let report = calculator
        .calculate(&target, 5)
        .expect("report should generate");
    assert_eq!(report.associated_tests.len(), 2);
    let test_ids: Vec<SymbolId> = report.associated_tests.iter().map(|s| s.id).collect();
    assert!(test_ids.contains(&test_direct));
    assert!(test_ids.contains(&test_indirect));
}

#[test]
fn test_dead_code_under_isolated_dense_cycles() {
    let mut graph = CodeGraph::new();

    // Entrypoint
    let main_fn = helper_create_symbol(&mut graph, "src/main.rs", "main", true);

    let used_fn = helper_create_symbol(&mut graph, "src/lib.rs", "used_work", false);

    graph
        .add_edge(
            main_fn,
            used_fn,
            DependencyEdge::new(EdgeKind::Calls, 10, false),
        )
        .expect("main->used");

    // Dense isolated cycle: C1 <-> C2 <-> C3 <-> C1 (private symbols, never called by main)
    let c1 = helper_create_symbol(&mut graph, "src/dead.rs", "cycle_1", false);
    let c2 = helper_create_symbol(&mut graph, "src/dead.rs", "cycle_2", false);
    let c3 = helper_create_symbol(&mut graph, "src/dead.rs", "cycle_3", false);

    let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);

    graph.add_edge(c1, c2, edge.clone()).expect("c1->c2");
    graph.add_edge(c2, c1, edge.clone()).expect("c2->c1");
    graph.add_edge(c2, c3, edge.clone()).expect("c2->c3");
    graph.add_edge(c3, c2, edge.clone()).expect("c3->c2");
    graph.add_edge(c3, c1, edge.clone()).expect("c3->c1");
    graph.add_edge(c1, c3, edge).expect("c1->c3");

    let detector = DeadCodeDetector::new(&graph);
    let report = detector.find_dead_symbols();

    // main_fn and used_fn should NOT be flagged as dead
    assert!(!report.dead_symbols.iter().any(|d| d.symbol.id == main_fn));
    assert!(!report.dead_symbols.iter().any(|d| d.symbol.id == used_fn));

    // c1, c2, c3 MUST all be detected as IsolatedDeadCycle
    assert_eq!(report.dead_symbols.len(), 3);
    for item in &report.dead_symbols {
        assert_eq!(item.reason, DeadSymbolReason::IsolatedDeadCycle);
    }
}
