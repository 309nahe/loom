//! High-performance code analysis, blast radius calculation, and reachability diagnostics.

pub mod blast_radius;
pub mod error;

pub use blast_radius::{
    AssociatedTestDetail, BlastRadiusCalculator, BlastRadiusReport, DirectCallerDetail, RiskLevel,
    TransitiveCallerDetail,
};
pub use error::{AnalysisError, Result};
