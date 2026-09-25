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
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    let args = Args::parse();
    let workspace_root = std::fs::canonicalize(&args.workspace)?;

    info!("Starting Loom Daemon for workspace: {:?}", workspace_root);

    let graph = Arc::new(RwLock::new(CodeGraph::new()));
    let pipeline = IndexingPipeline::new(graph.clone());

    // Initial batch scan across workspace
    let mut initial_files = Vec::new();
    collect_files_recursive(&workspace_root, &mut initial_files);
    info!(
        "Found {} candidate source files to index",
        initial_files.len()
    );

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

    // Launch background debounced file watcher
    let watcher = DaemonWatcher::new(&workspace_root, pipeline);
    tokio::task::spawn_blocking(move || {
        if let Err(err) = watcher.watch_loop() {
            tracing::error!("File watcher failed: {:?}", err);
        }
    })
    .await?;

    Ok(())
}

fn collect_files_recursive(dir: &std::path::Path, collected: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Ignore hidden directories like .git and target
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
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
