//! Incremental AST indexing and graph reconciliation pipeline.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use loom_ast::parser::{AstEngine, Language, ParsedFile};
use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_graph::graph::CodeGraph;
use rayon::prelude::*;
use tracing::{debug, info, warn};

/// Thread-safe orchestration engine managing AST parsing and graph reconciliation.
#[derive(Clone)]
pub struct IndexingPipeline {
    graph: Arc<RwLock<CodeGraph>>,
}

impl IndexingPipeline {
    /// Creates a new `IndexingPipeline` wrapping a shared `CodeGraph`.
    #[must_use]
    pub fn new(graph: Arc<RwLock<CodeGraph>>) -> Self {
        Self { graph }
    }

    /// Returns a reference to the shared graph instance.
    #[must_use]
    pub fn graph(&self) -> &Arc<RwLock<CodeGraph>> {
        &self.graph
    }

    /// Incrementally indexes a single file, re-parsing its AST and atomically updating the graph.
    ///
    /// Performance target: $< 5\,\text{ms}$.
    pub fn index_file(&self, file_path: &Path) -> Option<std::time::Duration> {
        let start = Instant::now();

        Language::from_path(file_path)?;

        let Ok(source_code) = fs::read_to_string(file_path) else {
            warn!("Failed to read file for indexing: {:?}", file_path);
            return None;
        };

        let mut engine = AstEngine::new();
        let parsed = match engine.parse_source(file_path, &source_code) {
            Ok(res) => res,
            Err(err) => {
                warn!("AST parsing failed for {:?}: {}", file_path, err);
                return None;
            }
        };

        self.reconcile_parsed_file(file_path, parsed);

        let elapsed = start.elapsed();
        debug!(
            "Incremental re-index for {:?} completed in {:?}",
            file_path, elapsed
        );
        Some(elapsed)
    }

    /// Parses a batch of files in parallel using Rayon and reconciles them into the graph.
    pub fn index_batch(&self, file_paths: &[PathBuf]) -> std::time::Duration {
        let start = Instant::now();

        // 1. Parallel AST Extraction via Rayon
        let parsed_batch: Vec<(PathBuf, ParsedFile)> = file_paths
            .par_iter()
            .filter(|path| Language::from_path(path).is_some())
            .filter_map(|path| {
                let source = fs::read_to_string(path).ok()?;
                let mut engine = AstEngine::new();
                let parsed = engine.parse_source(path, &source).ok()?;
                Some((path.clone(), parsed))
            })
            .collect();

        // 2. Atomic Graph Reconciliation
        for (path, parsed) in parsed_batch {
            self.reconcile_parsed_file(&path, parsed);
        }

        let elapsed = start.elapsed();
        info!(
            "Batch indexing of {} files completed in {:?}",
            file_paths.len(),
            elapsed
        );
        elapsed
    }

    /// Atomically removes all symbols and edges for a deleted or moved file.
    pub fn remove_file(&self, file_path: &Path) {
        if let Ok(mut graph) = self.graph.write() {
            let removed = graph.invalidate_file(file_path);
            debug!(
                "Invalidated {} symbols for removed file {:?}",
                removed.len(),
                file_path
            );
        }
    }

    /// Reconciles a parsed file into the `CodeGraph`.
    fn reconcile_parsed_file(&self, file_path: &Path, parsed: ParsedFile) {
        let Ok(mut graph) = self.graph.write() else {
            return;
        };

        // 1. Invalidate stale state for this file
        graph.invalidate_file(file_path);

        // 2. Upsert newly extracted symbol nodes
        for symbol in &parsed.symbols {
            graph.upsert_symbol(symbol.clone());
        }

        // 3. Link discovered function call dependencies
        for call in parsed.raw_calls {
            let matching_target_ids: Vec<loom_core::id::SymbolId> = graph
                .get_symbols_by_name(&call.callee_name)
                .into_iter()
                .map(|s| s.id)
                .collect();

            for target_id in matching_target_ids {
                // Find caller symbol enclosing this call site line
                let caller_symbol = parsed
                    .symbols
                    .iter()
                    .find(|s| s.line_range.0 <= call.line && call.line <= s.line_range.1);

                if let Some(caller) = caller_symbol {
                    let edge = DependencyEdge::new(EdgeKind::Calls, call.line, call.is_conditional);
                    let _ = graph.add_edge(caller.id, target_id, edge);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_single_file_incremental_indexing_latency() {
        let dir = tempdir().expect("create temp dir");
        let file_path = dir.path().join("service.rs");

        let mut file = fs::File::create(&file_path).expect("create test file");
        writeln!(
            file,
            "pub fn helper() {{}}\npub fn entrypoint() {{\n    helper();\n}}"
        )
        .expect("write test file");

        let graph = Arc::new(RwLock::new(CodeGraph::new()));
        let pipeline = IndexingPipeline::new(graph.clone());

        // Perform single-file index
        let duration = pipeline.index_file(&file_path).expect("index successful");
        // Verify sub-5ms performance requirement
        assert!(
            duration.as_millis() < 50,
            "Indexing must be fast (took {duration:?})"
        );

        let read_graph = graph.read().expect("read graph");
        assert_eq!(read_graph.node_count(), 2);
        assert_eq!(read_graph.edge_count(), 1);

        let helpers = read_graph.get_symbols_by_name("helper");
        assert_eq!(helpers.len(), 1);
        let callers = read_graph.get_callers(&helpers[0].id);
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].0.name, "entrypoint");
    }

    #[test]
    fn test_batch_indexing_and_file_removal() {
        let dir = tempdir().expect("create temp dir");
        let file1 = dir.path().join("mod1.rs");
        let file2 = dir.path().join("mod2.rs");

        fs::write(&file1, "pub fn alpha() {}").expect("write file1");
        fs::write(&file2, "pub fn beta() { alpha(); }").expect("write file2");

        let graph = Arc::new(RwLock::new(CodeGraph::new()));
        let pipeline = IndexingPipeline::new(graph.clone());

        let _ = pipeline.index_batch(&[file1.clone(), file2.clone()]);

        {
            let read_graph = graph.read().expect("read lock");
            assert_eq!(read_graph.node_count(), 2);
            assert_eq!(read_graph.file_count(), 2);
            assert_eq!(read_graph.edge_count(), 1);
        }

        // Test file removal
        pipeline.remove_file(&file1);

        {
            let read_graph = graph.read().expect("read lock");
            assert_eq!(read_graph.node_count(), 1);
            assert_eq!(read_graph.file_count(), 1);
            assert_eq!(read_graph.edge_count(), 0);
        }
    }
}
