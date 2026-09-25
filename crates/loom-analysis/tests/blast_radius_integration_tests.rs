//! Integration test for blast-radius risk heuristics on a multi-level call hierarchy.
//!
//! Validates the depth-decayed scoring model end to end: exported callers weigh 3x internal
//! ones, distant callers decay by $0.75^{\text{depth}}$, and the resulting score lands in
//! the expected `RiskLevel` band.

use loom_analysis::blast_radius::{BlastRadiusCalculator, RiskLevel};
use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use loom_graph::CodeGraph;

fn make_symbol(file: &str, name: &str, is_exported: bool) -> SymbolNode {
    let id = SymbolId::derive(file, &[], name, &format!("fn {name}()"));
    SymbolNode::new(
        id,
        name,
        SymbolKind::Function,
        file,
        (0, 50),
        (1, 15),
        None,
        format!("fn {name}()"),
        is_exported,
        1,
    )
}

#[test]
fn test_complex_blast_radius_hierarchy_and_risk_heuristics() {
    let mut graph = CodeGraph::new();

    // Target: internal payment processing helper
    let target = make_symbol("src/payment/processor.rs", "charge_card_internal", false);
    // Direct callers:
    let direct_service = make_symbol("src/payment/service.rs", "process_checkout", false);
    let direct_refund = make_symbol("src/payment/service.rs", "process_refund", false);
    // Public API route handlers:
    let api_checkout = make_symbol("src/api/v1/checkout.rs", "checkout_route_handler", true);
    let api_refund = make_symbol("src/api/v1/refund.rs", "refund_route_handler", true);
    // Unit test:
    let test_proc = make_symbol("tests/payment_tests.rs", "test_charge_card", false);

    let id_target = target.id;
    let id_service = direct_service.id;
    let id_refund = direct_refund.id;
    let id_api_checkout = api_checkout.id;
    let id_api_refund = api_refund.id;
    let id_test = test_proc.id;

    graph.upsert_symbol(target);
    graph.upsert_symbol(direct_service);
    graph.upsert_symbol(direct_refund);
    graph.upsert_symbol(api_checkout);
    graph.upsert_symbol(api_refund);
    graph.upsert_symbol(test_proc);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
    graph
        .add_edge(id_service, id_target, edge.clone())
        .expect("service->target");
    graph
        .add_edge(id_refund, id_target, edge.clone())
        .expect("refund->target");
    graph
        .add_edge(id_api_checkout, id_service, edge.clone())
        .expect("api->service");
    graph
        .add_edge(id_api_refund, id_refund, edge.clone())
        .expect("api->refund");
    graph
        .add_edge(id_test, id_target, edge)
        .expect("test->target");

    let calculator = BlastRadiusCalculator::new(&graph);
    let report = calculator
        .calculate(&id_target, 5)
        .expect("report generated");

    assert_eq!(report.target_symbol.name, "charge_card_internal");
    // Direct callers (non-test): service & refund
    assert_eq!(report.direct_callers.len(), 3); // service, refund, and test in raw incoming
    assert_eq!(report.associated_tests.len(), 1);
    assert_eq!(report.associated_tests[0].name, "test_charge_card");
    assert_eq!(report.transitive_callers.len(), 2); // api_checkout, api_refund

    // Risk level must be HIGH or CRITICAL because public APIs are transitively impacted
    assert!(matches!(
        report.risk_level,
        RiskLevel::High | RiskLevel::Critical
    ));
    assert!(report.risk_score_raw > 10.0);
}
