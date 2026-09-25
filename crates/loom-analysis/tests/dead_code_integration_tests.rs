use loom_analysis::dead_code::{DeadCodeDetector, DeadSymbolReason};
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
        (1, 10),
        None,
        format!("fn {name}()"),
        is_exported,
        1,
    )
}

#[test]
fn test_complex_multi_module_dead_code_analysis() {
    let mut graph = CodeGraph::new();

    // Module 1: Live Public API & Helper
    let api_handler = make_symbol("src/api/auth.rs", "handle_auth", true);
    let auth_crypto = make_symbol("src/crypto/jwt.rs", "verify_jwt_token", false);

    // Module 2: Dead Unreferenced internal function
    let dead_legacy_md5 = make_symbol("src/crypto/legacy.rs", "verify_md5_hash", false);

    // Module 3: Dead circular helper cluster (dead_a -> dead_b -> dead_c -> dead_a)
    let dead_a = make_symbol("src/old/a.rs", "deprecated_parser_a", false);
    let dead_b = make_symbol("src/old/b.rs", "deprecated_parser_b", false);
    let dead_c = make_symbol("src/old/c.rs", "deprecated_parser_c", false);

    // Module 4: Test entrypoint (should be recognized as live entrypoint)
    let test_auth = make_symbol("tests/auth_test.rs", "test_auth_flow", false);

    let id_api = api_handler.id;
    let id_crypto = auth_crypto.id;
    let id_da = dead_a.id;
    let id_db = dead_b.id;
    let id_dc = dead_c.id;
    let id_test = test_auth.id;

    graph.upsert_symbol(api_handler);
    graph.upsert_symbol(auth_crypto);
    graph.upsert_symbol(dead_legacy_md5);
    graph.upsert_symbol(dead_a);
    graph.upsert_symbol(dead_b);
    graph.upsert_symbol(dead_c);
    graph.upsert_symbol(test_auth);

    let edge = DependencyEdge::new(EdgeKind::Calls, 10, false);
    graph
        .add_edge(id_api, id_crypto, edge.clone())
        .expect("api->crypto");
    graph
        .add_edge(id_test, id_api, edge.clone())
        .expect("test->api");

    // Circular dead loop
    graph.add_edge(id_da, id_db, edge.clone()).expect("da->db");
    graph.add_edge(id_db, id_dc, edge.clone()).expect("db->dc");
    graph.add_edge(id_dc, id_da, edge).expect("dc->da");

    let detector = DeadCodeDetector::new(&graph);
    let report = detector.find_dead_symbols();

    assert_eq!(report.total_dead_count, 4);

    let dead_names: Vec<&str> = report
        .dead_symbols
        .iter()
        .map(|d| d.symbol.name.as_str())
        .collect();

    assert!(dead_names.contains(&"verify_md5_hash"));
    assert!(dead_names.contains(&"deprecated_parser_a"));
    assert!(dead_names.contains(&"deprecated_parser_b"));
    assert!(dead_names.contains(&"deprecated_parser_c"));

    let md5_detail = report
        .dead_symbols
        .iter()
        .find(|d| d.symbol.name == "verify_md5_hash")
        .unwrap();
    assert_eq!(
        md5_detail.reason,
        DeadSymbolReason::UnreferencedInternalSymbol
    );

    let da_detail = report
        .dead_symbols
        .iter()
        .find(|d| d.symbol.name == "deprecated_parser_a")
        .unwrap();
    assert_eq!(da_detail.reason, DeadSymbolReason::IsolatedDeadCycle);
}
