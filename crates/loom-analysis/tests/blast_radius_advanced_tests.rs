//! Advanced blast-radius cases: boundary conditions and dynamic recomputation.
//!
//! Covers isolated symbols, standalone public APIs, mixed visibility chains, risk escalation
//! as callers are added at runtime, and test-suite association through shared intermediate
//! helpers (the "unit + integration + E2E all call the same factory" scenario).

use loom_analysis::blast_radius::{BlastRadiusCalculator, RiskLevel};
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
fn test_isolated_symbol_with_zero_callers() {
    let mut graph = CodeGraph::new();
    let node = make_node("src/utils.rs", "standalone_helper", false);
    let id = node.id;
    graph.upsert_symbol(node);

    let calc = BlastRadiusCalculator::new(&graph);
    let report = calc.calculate(&id, 5).expect("report exists");

    assert_eq!(report.risk_level, RiskLevel::Low);
    assert_eq!(report.total_affected_symbols, 0);
    assert!(report.direct_callers.is_empty());
    assert!(report.transitive_callers.is_empty());
    assert!(report.associated_tests.is_empty());
    assert!(report.risk_rationale.contains("LOW"));
}

#[test]
fn test_isolated_public_api_with_zero_callers() {
    let mut graph = CodeGraph::new();
    let node = make_node("src/lib.rs", "public_api_endpoint", true);
    let id = node.id;
    graph.upsert_symbol(node);

    let calc = BlastRadiusCalculator::new(&graph);
    let report = calc.calculate(&id, 5).expect("report exists");

    // Public API has base export score 15.0 (>= 8.0) -> High risk
    assert_eq!(report.risk_level, RiskLevel::High);
    assert_eq!(report.total_affected_symbols, 0);
    assert!(report.risk_rationale.contains("HIGH"));
}

#[test]
fn test_mixed_visibility_chain_blast_radius() {
    let mut graph = CodeGraph::new();

    // Chain:
    // Leaf (Private) <- Internal_Helper (Private) <- Public_Service (Public) <- Public_Controller (Public)
    let leaf = make_node("src/db/raw.rs", "execute_sql_raw", false);
    let helper = make_node("src/db/repo.rs", "fetch_user_record", false);
    let service = make_node("src/services/user.rs", "get_user_profile", true);
    let controller = make_node("src/api/v1.rs", "handle_get_user", true);

    let id_leaf = leaf.id;
    let id_helper = helper.id;
    let id_service = service.id;
    let id_controller = controller.id;

    graph.upsert_symbol(leaf);
    graph.upsert_symbol(helper);
    graph.upsert_symbol(service);
    graph.upsert_symbol(controller);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
    graph
        .add_edge(id_helper, id_leaf, edge.clone())
        .expect("helper->leaf");
    graph
        .add_edge(id_service, id_helper, edge.clone())
        .expect("service->helper");
    graph
        .add_edge(id_controller, id_service, edge)
        .expect("controller->service");

    let calc = BlastRadiusCalculator::new(&graph);
    let report = calc.calculate(&id_leaf, 5).expect("report exists");

    // Total affected upstream: helper (depth 1), service (depth 2), controller (depth 3) = 3 total
    assert_eq!(report.total_affected_symbols, 3);
    assert_eq!(report.direct_callers.len(), 1);
    assert_eq!(report.transitive_callers.len(), 2);

    // Theoretical score:
    // Helper: depth 1, private -> 0.75 * 1.0 * 2.0 = 1.5
    // Service: depth 2, public -> (0.75)^2 * 3.0 * 2.0 = 0.5625 * 6.0 = 3.375
    // Controller: depth 3, public -> (0.75)^3 * 3.0 * 2.0 = 0.421875 * 6.0 = 2.53125
    // Total raw score = 1.5 + 3.375 + 2.53125 = 7.40625
    let expected_score = 7.40625_f64;
    assert!(
        (report.risk_score_raw - expected_score).abs() < 1e-5,
        "Expected score {}, got {}",
        expected_score,
        report.risk_score_raw
    );

    // Public API boundary impacted with 3 callers (>= 2) => RiskLevel::High
    assert_eq!(report.risk_level, RiskLevel::High);
}

#[test]
fn test_dynamic_mutation_blast_radius_recomputation() {
    let mut graph = CodeGraph::new();

    let target = make_node("src/core.rs", "compute_hash", false);
    let id_target = target.id;
    graph.upsert_symbol(target);

    // Initial state: 0 callers
    let calc1 = BlastRadiusCalculator::new(&graph);
    let rep1 = calc1.calculate(&id_target, 5).expect("rep1");
    assert_eq!(rep1.risk_level, RiskLevel::Low);

    // Dynamically add 5 public API callers
    for i in 0..5 {
        let caller = make_node("src/api.rs", &format!("endpoint_{i}"), true);
        let id_caller = caller.id;
        graph.upsert_symbol(caller);
        graph
            .add_edge(
                id_caller,
                id_target,
                DependencyEdge::new(EdgeKind::Calls, 10, false),
            )
            .expect("caller->target");
    }

    // Recomputed state: 5 public direct callers
    let calc2 = BlastRadiusCalculator::new(&graph);
    let rep2 = calc2.calculate(&id_target, 5).expect("rep2");

    // 5 public callers: score = 5 * (0.75 * 3.0 * 2.0) = 22.5 (>= 20.0, total_affected=5) -> Critical
    assert_eq!(rep2.risk_level, RiskLevel::Critical);
    assert_eq!(rep2.total_affected_symbols, 5);
    assert_eq!(rep2.direct_callers.len(), 5);
}

#[test]
fn test_multi_test_suite_association_with_shared_helpers() {
    let mut graph = CodeGraph::new();

    let target = make_node("src/auth.rs", "validate_signature", true);
    let id_target = target.id;
    graph.upsert_symbol(target);

    // Intermediate helper in src/
    let helper = make_node("src/factories.rs", "create_signature_factory", false);
    let id_helper = helper.id;
    graph.upsert_symbol(helper);

    // Test 1 (Unit test direct)
    let t1 = make_node("tests/unit_auth_test.rs", "test_sig_direct", false);
    let id_t1 = t1.id;
    graph.upsert_symbol(t1);

    // Test 2 (Integration test via helper)
    let t2 = make_node("tests/integration_test.rs", "test_auth_flow", false);
    let id_t2 = t2.id;
    graph.upsert_symbol(t2);

    // Test 3 (E2E test via helper)
    let t3 = make_node("tests/e2e_test.rs", "test_full_system", false);
    let id_t3 = t3.id;
    graph.upsert_symbol(t3);

    let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);
    graph
        .add_edge(id_t1, id_target, edge.clone())
        .expect("t1->target");
    graph
        .add_edge(id_helper, id_target, edge.clone())
        .expect("helper->target");
    graph
        .add_edge(id_t2, id_helper, edge.clone())
        .expect("t2->helper");
    graph.add_edge(id_t3, id_helper, edge).expect("t3->helper");

    let calc = BlastRadiusCalculator::new(&graph);
    let report = calc.calculate(&id_target, 5).expect("report");

    assert_eq!(report.associated_tests.len(), 3);
    let test_ids: Vec<SymbolId> = report.associated_tests.iter().map(|t| t.id).collect();
    assert!(test_ids.contains(&id_t1));
    assert!(test_ids.contains(&id_t2));
    assert!(test_ids.contains(&id_t3));
}
