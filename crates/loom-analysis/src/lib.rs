//! # Loom Analysis
//!
//! Impact analysis layer: blast-radius estimation, deterministic risk scoring, and dead code
//! / orphan symbol detection, computed on top of the read-only [`loom_graph::CodeGraph`] API.
//!
//! ## Why a separate crate
//! Graph storage (`loom-graph`) and syntax extraction (`loom-ast`) know nothing about risk.
//! Keeping heuristics here means the graph layer stays a pure, testable topology engine and
//! the scoring model can evolve without touching indexing.
//!
//! ## Invariants
//! - **Deterministic scoring**: every number is produced by a closed-form formula over
//!   verified graph facts ($0.75^{\text{depth}}$ decay, 3.0x exported-boundary weight).
//!   No LLM inference, no probabilistic classification.
//! - **Zero false positives on live code**: dead code is defined as *unreachable from any
//!   root* (exported symbol, conventional entrypoint, or test), not merely "no callers".
//! - **Read-only consumers**: analyzers borrow a `&CodeGraph` and never mutate topology, so
//!   they can run against a read-locked snapshot while the watcher keeps indexing.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]

pub mod blast_radius;
pub mod dead_code;
pub mod error;

pub use blast_radius::{
    AssociatedTestDetail, BlastRadiusCalculator, BlastRadiusReport, DirectCallerDetail, RiskLevel,
    TransitiveCallerDetail,
};
pub use dead_code::{DeadCodeDetector, DeadCodeReport, DeadSymbolDetail, DeadSymbolReason};
pub use error::{AnalysisError, Result};
