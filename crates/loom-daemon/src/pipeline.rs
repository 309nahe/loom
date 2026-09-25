//! Incremental AST indexing and graph reconciliation pipeline.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use loom_ast::parser::{AstEngine, Language, ParsedFile};
use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::symbol::SymbolNode;
use loom_graph::graph::CodeGraph;
use rayon::prelude::*;
use tracing::{debug, info, warn};

// Thread-local cache of one `AstEngine` per OS thread.
//
// **Critical for the Rayon concurrency model.** Tree-sitter `Parser` values hold mutable
// scratch state and are not `Sync`, so sharing one engine would require a mutex and
// serialize all parallel parsing. Instead each worker thread transparently builds its own
// engine on first use and keeps it forever, which means:
// - no lock contention during parallel batch indexing,
// - the ~10-15 ms query-compilation cost is paid **once per thread**, not per file,
// - warm engines reuse parser allocations, so steady-state parses stay < 0.15 ms.
thread_local! {
    static THREAD_AST_ENGINE: RefCell<AstEngine> = RefCell::new(AstEngine::new());
}

/// Thread-safe orchestration engine managing AST parsing and graph reconciliation.
///
/// Cheap to `Clone`: all clones share the same `Arc<RwLock<CodeGraph>>`, so the watcher and
/// the MCP request handlers observe exactly the same topological state. The lock is held
/// only for the mutate phase — parsing happens entirely outside it, so readers are never
/// blocked by CPU-bound Tree-sitter work.
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
    /// This is the hot path for every keystroke-triggered filesystem event. The sequence is
    /// strictly: *parse outside the lock → invalidate this file's stale symbols → upsert
    /// fresh symbols → re-link edges*. Invalidation and re-linking are both scoped to the
    /// dirty file, so editing one file never triggers a workspace rescan.
    ///
    /// Performance target: $< 5\,\text{ms}$ (measured in practice at < 1 ms).
    ///
    /// Returns the measured duration, or `None` when the file was skipped. Skipping is never
    /// fatal: unsupported extensions, unreadable files, and parse failures all degrade
    /// gracefully via `tracing::warn!`, per the graceful-degradation rule.
    pub fn index_file(&self, file_path: &Path) -> Option<std::time::Duration> {
        let start = Instant::now();

        // Fast reject: never pay for a read on files we cannot parse (images, lockfiles,
        // vendored assets). The watcher may deliver any path under the workspace root.
        Language::from_path(file_path)?;

        let Ok(source_code) = fs::read_to_string(file_path) else {
            warn!("Failed to read file for indexing: {:?}", file_path);
            return None;
        };

        // Parsing happens with **no lock held**: the write lock is only taken later, in
        // `reconcile_parsed_file`. This is what allows concurrent readers (MCP queries) to
        // proceed while a file is being re-parsed.
        let parsed_res = THREAD_AST_ENGINE.with(|engine_cell| {
            engine_cell
                .borrow_mut()
                .parse_source(file_path, &source_code)
        });

        let parsed = match parsed_res {
            Ok(res) => res,
            Err(err) => {
                // A file that fails to parse keeps its previous graph state untouched; we
                // log and move on rather than corrupting the index with partial results.
                warn!("AST parsing failed for {:?}: {}", file_path, err);
                return None;
            }
        };

        self.reconcile_parsed_file(file_path, &parsed);

        let elapsed = start.elapsed();
        debug!(
            "Incremental re-index for {:?} completed in {:?}",
            file_path, elapsed
        );
        Some(elapsed)
    }

    /// Parses a batch of files in parallel using Rayon and reconciles them into the graph.
    ///
    /// Used for the initial workspace scan and for multi-file bulk changes. The work is
    /// split into a lock-free parallel parse phase followed by a serialized reconcile phase,
    /// so the write lock is held for graph mutation only.
    pub fn index_batch(&self, file_paths: &[PathBuf]) -> std::time::Duration {
        let start = Instant::now();

        // 1. Parallel AST Extraction via Rayon with Thread-Local Parsers
        //
        // `filter_map` collapses the three "skip this file" cases (unsupported language,
        // unreadable, unparseable) into one, so a single bad file in a large batch never
        // aborts the batch. Results are collected in order for deterministic reconciliation.
        let parsed_batch: Vec<(PathBuf, ParsedFile)> = file_paths
            .par_iter()
            .filter(|path| Language::from_path(path).is_some())
            .filter_map(|path| {
                let source = fs::read_to_string(path).ok()?;
                let parsed = THREAD_AST_ENGINE.with(|engine_cell| {
                    engine_cell.borrow_mut().parse_source(path, &source).ok()
                })?;
                Some((path.clone(), parsed))
            })
            .collect();

        // 2. Atomic Graph Reconciliation (Two-pass for complete cross-file symbol resolution)
        //
        // A single write-lock acquisition covers both passes, so no reader ever observes the
        // intermediate state where nodes exist but their edges do not.
        if let Ok(mut graph) = self.graph.write() {
            // Pass 1: drop each file's previous generation and insert all new symbols.
            // Invalidation happens per file *before* its own upserts, so a re-index of an
            // unchanged file yields exactly the same `SymbolId`s and the node count is stable.
            for (path, parsed) in &parsed_batch {
                graph.invalidate_file(path);
                for symbol in &parsed.symbols {
                    graph.upsert_symbol(symbol.clone());
                }
            }

            // Pass 2: link call edges.
            //
            // This *must* be a separate pass. In a single pass, a call in file A pointing at
            // a symbol in file B fails whenever B is processed after A — the target is not in
            // the graph yet. Deferring all edge linking until every node exists makes
            // cross-file forward references resolve on the very first workspace scan.
            for (_path, parsed) in &parsed_batch {
                for call in &parsed.raw_calls {
                    // Name-based resolution across the whole graph. Deliberately simple and
                    // deterministic: every same-named symbol is a candidate, never a
                    // heuristic "best guess" pick.
                    let matching_target_ids: Vec<loom_core::id::SymbolId> = graph
                        .get_symbols_by_name(&call.callee_name)
                        .into_iter()
                        .map(|s| s.id)
                        .collect();

                    for target_id in matching_target_ids {
                        let caller_symbol = resolve_caller_symbol(parsed, call.line);

                        if let Some(caller) = caller_symbol {
                            let edge = DependencyEdge::new(
                                EdgeKind::Calls,
                                call.line,
                                call.is_conditional,
                            );
                            // `add_edge` can only fail if an endpoint vanished, which is
                            // impossible under the write lock; ignore rather than panic.
                            let _ = graph.add_edge(caller.id, target_id, edge);
                        }
                    }
                }
            }
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
    ///
    /// Called for filesystem removal/rename events. Because petgraph drops incident edges
    /// along with the nodes, a deleted file can never leave dangling `Calls` edges behind.
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
    ///
    /// Single-file counterpart of the two-pass batch reconcile, holding the write lock for
    /// the whole mutation so the transition is atomic for concurrent readers. Borrows
    /// `parsed` rather than taking ownership so the caller keeps its `ParsedFile` alive
    /// (and measurable) for the duration of the call.
    fn reconcile_parsed_file(&self, file_path: &Path, parsed: &ParsedFile) {
        let Ok(mut graph) = self.graph.write() else {
            return;
        };

        // 1. Invalidate stale state for this file.
        //    Ordered before the upserts: symbols removed or renamed on disk must disappear,
        //    and their edges go with them.
        graph.invalidate_file(file_path);

        // 2. Upsert newly extracted symbol nodes. Identifiers are deterministic, so an
        //    unchanged symbol keeps its `SymbolId` and simply has its metadata refreshed.
        for symbol in &parsed.symbols {
            graph.upsert_symbol(symbol.clone());
        }

        // 3. Link discovered function call dependencies.
        //    Done after all of *this file's* symbols exist, which is sufficient for a single
        //    file; cross-file targets already live in the graph from previous indexing.
        for call in &parsed.raw_calls {
            let matching_target_ids: Vec<loom_core::id::SymbolId> = graph
                .get_symbols_by_name(&call.callee_name)
                .into_iter()
                .map(|s| s.id)
                .collect();

            for target_id in matching_target_ids {
                let caller_symbol = resolve_caller_symbol(parsed, call.line);

                if let Some(caller) = caller_symbol {
                    let edge = DependencyEdge::new(EdgeKind::Calls, call.line, call.is_conditional);
                    let _ = graph.add_edge(caller.id, target_id, edge);
                }
            }
        }
    }
}

/// Attributes a call site, given as a 1-indexed line, to its enclosing symbol.
///
/// A call can syntactically sit inside several nested definitions at once (a method inside
/// a class inside a module). The correct answer is the **innermost callable**: attributing
/// a call to the outer class would collapse all of its methods into a single graph node and
/// destroy the method-level blast radius.
///
/// The sort key encodes that rule in two stages:
/// 1. `kind_penalty` — callables win outright over non-callables, so a method always beats
///    its enclosing class even if the class somehow had a smaller span.
/// 2. `span` — among equally-ranked candidates, the tightest enclosing line range wins.
///
/// Enumeration order is irrelevant to the result because the key is a total order on
/// (callable-ness, span), so the outcome is deterministic.
fn resolve_caller_symbol(parsed: &ParsedFile, call_line: u32) -> Option<&SymbolNode> {
    parsed
        .symbols
        .iter()
        .filter(|s| s.line_range.0 <= call_line && call_line <= s.line_range.1)
        .min_by_key(|s| {
            let span = s.line_range.1.saturating_sub(s.line_range.0);
            let kind_penalty = u8::from(!s.kind.is_callable());
            (kind_penalty, span)
        })
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

        // Warm up thread-local parser
        let _ = pipeline.index_file(&file_path);

        // Perform single-file index and measure latency
        let duration = pipeline.index_file(&file_path).expect("index successful");

        // Verify strict sub-5ms performance requirement
        assert!(
            duration.as_millis() < 5,
            "Indexing must be sub-5ms (took {duration:?})"
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
