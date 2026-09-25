//! # Loom Daemon
//!
//! Background daemon runtime, debounced filesystem watching, and incremental AST re-indexing engine.
//!
//! ## Invariants
//! - **Sub-5ms Incremental Invalidation**: Only dirty files are re-parsed upon file events.
//! - **Async / Rayon Thread Pool Isolation**: Long-running AST parsing runs on Rayon threads without
//!   blocking the Tokio runtime.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]

pub mod pipeline;
pub mod watcher;

pub use pipeline::IndexingPipeline;
pub use watcher::DaemonWatcher;
