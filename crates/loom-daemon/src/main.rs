//! Loom daemon executable entry point.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use clap::Parser;
use loom_daemon::{DaemonWatcher, IndexingPipeline};
use loom_graph::CodeGraph;
use tracing::info;
use tracing_subscriber::EnvFilter;

/// Command-line arguments for the Loom daemon.
#[derive(Parser, Debug)]
#[command(author, version, about = "High-performance deterministic code-topography daemon", long_about = None)]
struct Args {
    /// Workspace root directory to monitor and index.
    #[arg(short, long, default_value = ".")]
    workspace: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // `RUST_LOG` overrides the default INFO level, so operators can turn on `debug` to see
    // per-file incremental latency without a rebuild.
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let args = Args::parse();
    // Canonicalize up-front: every path stored in the graph is then absolute and stable,
    // and it also rejects a non-existent workspace before any indexing begins.
    let workspace_root = std::fs::canonicalize(&args.workspace)?;

    info!("Starting Loom Daemon for workspace: {:?}", workspace_root);

    // Single shared graph behind an `RwLock`. Readers (analysis, future MCP handlers) take
    // the read lock; the indexing pipeline takes the write lock only for mutations.
    let graph = Arc::new(RwLock::new(CodeGraph::new()));
    let pipeline = IndexingPipeline::new(graph.clone());

    // Initial batch scan across workspace
    let mut initial_files = Vec::new();
    collect_files_recursive(&workspace_root, &mut initial_files);
    info!(
        "Found {} candidate source files to index",
        initial_files.len()
    );

    // CPU-bound: Rayon parsing of the whole workspace would otherwise occupy an async
    // worker thread and stall every other task, hence `spawn_blocking`.
    let pipeline_clone = pipeline.clone();
    tokio::task::spawn_blocking(move || {
        pipeline_clone.index_batch(&initial_files);
    })
    .await?;

    let node_count = graph.read().map_or(0, |g| g.node_count());
    let edge_count = graph.read().map_or(0, |g| g.edge_count());
    info!(
        "Initial indexing complete: {} symbols, {} dependency edges",
        node_count, edge_count
    );

    // Launch background debounced file watcher.
    // `watch_loop` blocks forever, so awaiting it keeps the process alive as the daemon's
    // main task while the Tokio runtime stays available for future MCP serving.
    let watcher = DaemonWatcher::new(&workspace_root, pipeline);
    tokio::task::spawn_blocking(move || {
        if let Err(err) = watcher.watch_loop() {
            tracing::error!("File watcher failed: {:?}", err);
        }
    })
    .await?;

    Ok(())
}

/// Recursively collects candidate source files under `dir`.
///
/// Deliberately conservative and dependency-free (no `walkdir`/`ignore`): it prunes only
/// directories that are guaranteed to be noise. Language filtering is *not* done here —
/// `IndexingPipeline` owns that decision, keeping one single source of truth for "what is
/// indexable".
fn collect_files_recursive(dir: &std::path::Path, collected: &mut Vec<PathBuf>) {
    // Unreadable directory (permissions, race with deletion) is skipped rather than fatal.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    // `flatten` drops individual entry errors so one bad inode cannot abort the walk.
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Ignore hidden directories like .git and target
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // `.git` and friends via the dot rule; `target` (Rust build output) and
                // `node_modules` are by far the largest trees in a typical workspace and
                // contain no first-party source worth indexing.
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }
            }
            collect_files_recursive(&path, collected);
        } else if path.is_file() {
            collected.push(path);
        }
    }
}
