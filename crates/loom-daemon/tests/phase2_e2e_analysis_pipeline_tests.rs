use loom_analysis::blast_radius::BlastRadiusCalculator;
use loom_analysis::dead_code::DeadCodeDetector;
use loom_daemon::pipeline::IndexingPipeline;
use loom_graph::CodeGraph;
use std::fs;
use std::sync::{Arc, RwLock};
use tempfile::tempdir;

#[test]
fn test_end_to_end_ast_graph_blast_radius_and_dead_code_pipeline() {
    let temp_dir = tempdir().expect("temp dir");
    let src_dir = temp_dir.path().join("src");
    let tests_dir = temp_dir.path().join("tests");
    fs::create_dir_all(&src_dir).expect("create src dir");
    fs::create_dir_all(&tests_dir).expect("create tests dir");

    let main_rs = src_dir.join("main.rs");
    let api_rs = src_dir.join("api.rs");
    let db_rs = src_dir.join("db.rs");
    let test_rs = tests_dir.join("api_test.rs");

    fs::write(
        &main_rs,
        r#"
fn main() {
    handle_request();
}
"#,
    )
    .expect("write main");

    fs::write(
        &api_rs,
        r#"
pub fn handle_request() {
    helper_calc();
}

fn helper_calc() {
    db_query();
}
"#,
    )
    .expect("write api");

    fs::write(
        &db_rs,
        r#"
pub fn db_query() {
    // executes query
}

fn dead_backup_routine() {
    // old unused routine
}
"#,
    )
    .expect("write db");

    fs::write(
        &test_rs,
        r#"
fn test_handle_request() {
    handle_request();
}
"#,
    )
    .expect("write test");

    let shared_graph = Arc::new(RwLock::new(CodeGraph::new()));
    let pipeline = IndexingPipeline::new(shared_graph);

    // 1. Initial Batch Index
    let files = vec![
        main_rs.clone(),
        api_rs.clone(),
        db_rs.clone(),
        test_rs.clone(),
    ];
    let duration = pipeline.index_batch(&files);
    assert!(duration.as_millis() < 500);

    // 2. Query Blast Radius on `db_query`
    {
        let graph = pipeline.graph().read().expect("lock read");
        let db_symbols = graph.get_symbols_for_file(&db_rs);
        let db_query_sym = db_symbols
            .iter()
            .find(|s| s.name == "db_query")
            .expect("db_query symbol must exist");

        let calc = BlastRadiusCalculator::new(&graph);
        let report = calc
            .calculate(&db_query_sym.id, 5)
            .expect("report must be generated");

        assert_eq!(report.target_symbol.name, "db_query");
        // Direct callers: helper_calc
        assert_eq!(report.direct_callers.len(), 1);
        assert_eq!(report.direct_callers[0].name, "helper_calc");

        // Transitive callers: handle_request
        assert!(
            report
                .transitive_callers
                .iter()
                .any(|c| c.name == "handle_request")
        );

        // Associated tests: test_handle_request
        assert_eq!(report.associated_tests.len(), 1);
        assert_eq!(report.associated_tests[0].name, "test_handle_request");

        // Dead Code Detection
        let detector = DeadCodeDetector::new(&graph);
        let dead_report = detector.find_dead_symbols();
        assert!(
            dead_report
                .dead_symbols
                .iter()
                .any(|d| d.symbol.name == "dead_backup_routine"),
            "dead_backup_routine must be flagged as dead code"
        );
    }

    // 3. Incremental Update: Wire `db_query` to invoke `dead_backup_routine`
    fs::write(
        &db_rs,
        r#"
pub fn db_query() {
    dead_backup_routine();
}

fn dead_backup_routine() {
    // now actively called!
}
"#,
    )
    .expect("write updated db.rs");

    let updated_duration = pipeline.index_file(&db_rs);
    println!("Updated duration: {:?}", updated_duration);
    assert!(updated_duration.is_some());

    // 4. Verify Dead Code is Resolved Dynamically
    {
        let graph = pipeline.graph().read().expect("lock read");
        let detector = DeadCodeDetector::new(&graph);
        let dead_report = detector.find_dead_symbols();

        assert!(
            !dead_report
                .dead_symbols
                .iter()
                .any(|d| d.symbol.name == "dead_backup_routine"),
            "dead_backup_routine is now wired into the live call tree and must NOT be dead code"
        );
    }
}
