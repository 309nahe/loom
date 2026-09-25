//! # Loom Graph
//!
//! In-memory directed dependency graph and topological index for the Loom daemon.
//!
//! ## Invariants
//! - **Petgraph Index Compaction Safety**: Correctly reconciles internal swap-removals in Petgraph `DiGraph`
//!   so that bidirectional lookup tables (`symbol_to_node` and `file_to_symbols`) remain permanently synchronized.
//! - **Atomic File Invalidation**: Modifying a file invalidates only its symbols and incident edges without
//!   requiring full workspace graph re-computation.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]

pub mod error;
pub mod graph;

pub use error::{GraphError, Result};
pub use graph::CodeGraph;
