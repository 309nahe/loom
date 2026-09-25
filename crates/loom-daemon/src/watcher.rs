//! OS-level filesystem watcher with 50ms debounced event aggregation.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use notify_debouncer_mini::{DebouncedEvent, new_debouncer};
use tracing::{error, info, warn};

use crate::pipeline::IndexingPipeline;

/// File watcher monitoring repository changes and dispatching to the indexing pipeline.
///
/// Owns the OS-level watch registration and translates raw filesystem events into the three
/// operations the pipeline understands: index a changed file, remove a deleted file, ignore
/// everything else. It holds no graph state of its own — it just forwards to a cloned
/// [`IndexingPipeline`], so many watchers could share one graph safely.
pub struct DaemonWatcher {
    root_path: PathBuf,
    pipeline: IndexingPipeline,
}

impl DaemonWatcher {
    /// Creates a new `DaemonWatcher` monitoring `root_path`.
    #[must_use]
    pub fn new(root_path: impl AsRef<Path>, pipeline: IndexingPipeline) -> Self {
        Self {
            root_path: root_path.as_ref().to_path_buf(),
            pipeline,
        }
    }

    /// Starts the blocking event loop processing debounced filesystem events.
    ///
    /// **This call never returns under normal operation** — it loops on the event channel
    /// until the debouncer is dropped. It is blocking by nature (synchronous `notify`
    /// integration), so the caller must run it on a dedicated blocking thread
    /// (`tokio::task::spawn_blocking`), never directly on an async runtime worker.
    ///
    /// # Errors
    /// Returns error if the OS file watcher fails to initialize.
    pub fn watch_loop(&self) -> anyhow::Result<()> {
        // Channel bridging the debouncer's callback thread to this loop. The callback must
        // stay trivial (just `send`) because it runs on the notify thread and blocking
        // there would stall event delivery.
        let (tx, rx): (
            _,
            Receiver<Result<Vec<DebouncedEvent>, notify_debouncer_mini::notify::Error>>,
        ) = channel();

        // 50ms aggregation window per specifications in IDEA.md.
        //
        // Editors emit several events per save (write, rename, chmod). Without aggregation
        // each save would trigger redundant re-parses; 50 ms is short enough to stay
        // imperceptible while collapsing a save into a single indexing cycle.
        let mut debouncer = new_debouncer(Duration::from_millis(50), move |res| {
            let _ = tx.send(res);
        })?;

        debouncer.watcher().watch(
            &self.root_path,
            notify_debouncer_mini::notify::RecursiveMode::Recursive,
        )?;

        info!("Daemon file watcher initialized on {:?}", self.root_path);

        for events_res in rx {
            match events_res {
                Ok(events) => {
                    for event in events {
                        let path = event.path;
                        // Existence is the discriminator between a change and a deletion:
                        // notify reports both as path-only events.
                        if path.exists() {
                            // Directories are skipped — their contents arrive as their own
                            // events, and indexing a directory is meaningless.
                            if path.is_file() {
                                self.pipeline.index_file(&path);
                            }
                        } else {
                            // Deleted or moved away: drop its symbols and incident edges so
                            // the graph never reports callers into a file that is gone.
                            self.pipeline.remove_file(&path);
                        }
                    }
                }
                Err(err) => {
                    // Individual event errors are non-fatal (e.g. a file vanished between
                    // the event and the read); log and keep watching.
                    warn!("Filesystem watcher event error: {:?}", err);
                }
            }
        }

        error!("Daemon watcher event channel closed unexpectedly");
        Ok(())
    }
}
