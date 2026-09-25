//! Error definitions for static analysis and blast radius calculation.

use thiserror::Error;

/// Result type alias for analysis operations.
///
/// Analysis is intentionally total: `BlastRadiusCalculator::calculate` and
/// `DeadCodeDetector::find_dead_symbols` return `Option`/empty reports rather than failing
/// on unknown symbols, so this alias exists for forward-compatible variants.
pub type Result<T> = std::result::Result<T, AnalysisError>;

/// Errors encountered during graph analysis.
///
/// The symbol is carried as a `String` (usually the hex `SymbolId`) so this type stays
/// `Clone`-friendly and independent of `loom-core`'s hashing internals.
#[derive(Debug, Error)]
pub enum AnalysisError {
    /// Specified symbol not found in graph.
    #[error("Symbol not found in graph: {0}")]
    SymbolNotFound(String),
}
