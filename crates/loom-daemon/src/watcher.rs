//! OS-level filesystem watcher with 50ms debounced event aggregation.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use notify_debouncer_mini::{DebouncedEvent, new_debouncer};
use tracing::{error, info, warn};

use crate::pipeline::IndexingPipeline;

/// File watcher monitoring repository changes and dispatching to the indexing pipeline.
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
    /// # Errors
    /// Returns error if the OS file watcher fails to initialize.
    pub fn watch_loop(&self) -> anyhow::Result<()> {
        let (tx, rx): (
            _,
            Receiver<Result<Vec<DebouncedEvent>, notify_debouncer_mini::notify::Error>>,
        ) = channel();

        // 50ms aggregation window per specifications in IDEA.md
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
                        if path.exists() {
                            if path.is_file() {
                                self.pipeline.index_file(&path);
                            }
                        } else {
                            self.pipeline.remove_file(&path);
                        }
                    }
                }
                Err(err) => {
                    warn!("Filesystem watcher event error: {:?}", err);
                }
            }
        }

        error!("Daemon watcher event channel closed unexpectedly");
        Ok(())
    }
}
