//! High-performance code analysis, blast radius calculation, and reachability diagnostics.

pub mod blast_radius;
pub mod dead_code;
pub mod error;

pub use blast_radius::{
    AssociatedTestDetail, BlastRadiusCalculator, BlastRadiusReport, DirectCallerDetail, RiskLevel,
    TransitiveCallerDetail,
};
pub use dead_code::{DeadCodeDetector, DeadCodeReport, DeadSymbolDetail, DeadSymbolReason};
pub use error::{AnalysisError, Result};
