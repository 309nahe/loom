//! End-to-end test of the daemon indexing pipeline on a real polyglot workspace.
//!
//! Spins up a temporary Rust + TypeScript + Python workspace, indexes it via the parallel
//! batch pipeline, asserts cross-file call resolution, then edits a file on disk and
//! re-indexes it — verifying both the < 5 ms incremental budget and correct reconciliation
//! of the shared graph state.

use loom_daemon::pipeline::IndexingPipeline;
use loom_graph::CodeGraph;
use std::fs;
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tempfile::tempdir;

#[test]
fn test_end_to_end_polyglot_workspace_indexing() {
    let dir = tempdir().expect("create temp dir");
    let root = dir.path();

    let rust_file = root.join("auth.rs");
    let ts_file = root.join("client.ts");
    let py_file = root.join("worker.py");

    fs::write(
        &rust_file,
        "pub fn verify_token() -> bool { true }\npub fn handle_login() { verify_token(); }",
    )
    .expect("write rust");

    fs::write(
        &ts_file,
        "class ApiClient { login() { callApi(); } }\nfunction callApi() {}",
    )
    .expect("write ts");

    fs::write(
        &py_file,
        "def process_job():\n    fetch_task()\ndef fetch_task():\n    pass",
    )
    .expect("write py");

    let graph = Arc::new(RwLock::new(CodeGraph::new()));
    let pipeline = IndexingPipeline::new(graph.clone());

    let files = vec![rust_file.clone(), ts_file.clone(), py_file.clone()];

    let start = Instant::now();
    let _duration = pipeline.index_batch(&files);
    let total_elapsed = start.elapsed();

    assert!(
        total_elapsed.as_millis() < 500,
        "Batch indexing took {total_elapsed:?}"
    );

    {
        let read_graph = graph.read().expect("read graph");
        assert_eq!(read_graph.file_count(), 3);
        assert_eq!(read_graph.node_count(), 7); // 2 in rust + 3 in ts (ApiClient, login, callApi) + 2 in py
        assert_eq!(read_graph.edge_count(), 3); // 1 in rust + 1 in ts + 1 in py

        // Check Rust dependencies
        let verify_syms = read_graph.get_symbols_by_name("verify_token");
        assert_eq!(verify_syms.len(), 1);
        let callers = read_graph.get_callers(&verify_syms[0].id);
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].0.name, "handle_login");

        // Check TypeScript dependencies
        let call_api_syms = read_graph.get_symbols_by_name("callApi");
        assert_eq!(call_api_syms.len(), 1);
        let ts_callers = read_graph.get_callers(&call_api_syms[0].id);
        assert_eq!(ts_callers.len(), 1);
        assert_eq!(ts_callers[0].0.name, "login");

        // Check Python dependencies
        let fetch_task_syms = read_graph.get_symbols_by_name("fetch_task");
        assert_eq!(fetch_task_syms.len(), 1);
        let py_callers = read_graph.get_callers(&fetch_task_syms[0].id);
        assert_eq!(py_callers.len(), 1);
        assert_eq!(py_callers[0].0.name, "process_job");
    }

    // Now test incremental modification of rust_file
    fs::write(
        &rust_file,
        "pub fn verify_token() -> bool { true }\npub fn refresh_token() { verify_token(); }",
    )
    .expect("modify rust file");

    // Warm up thread-local AST engine on current thread
    let _ = pipeline.index_file(&rust_file);

    // Measure true incremental re-indexing latency
    let reindex_duration = pipeline.index_file(&rust_file).expect("re-index");
    assert!(
        reindex_duration.as_millis() < 5,
        "Single file incremental index must be < 5ms (took {reindex_duration:?})"
    );

    {
        let read_graph = graph.read().expect("read graph");
        assert_eq!(read_graph.file_count(), 3);
        assert_eq!(read_graph.node_count(), 7);

        // handle_login is gone, replaced by refresh_token
        assert!(read_graph.get_symbols_by_name("handle_login").is_empty());
        let refresh_syms = read_graph.get_symbols_by_name("refresh_token");
        assert_eq!(refresh_syms.len(), 1);

        let verify_syms = read_graph.get_symbols_by_name("verify_token");
        let callers = read_graph.get_callers(&verify_syms[0].id);
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].0.name, "refresh_token");
    }
}
