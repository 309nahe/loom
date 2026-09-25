//! Error definitions for static analysis and blast radius calculation.

use thiserror::Error;

/// Result type alias for analysis operations.
pub type Result<T> = std::result::Result<T, AnalysisError>;

/// Errors encountered during graph analysis.
#[derive(Debug, Error)]
pub enum AnalysisError {
    /// Specified symbol not found in graph.
    #[error("Symbol not found in graph: {0}")]
    SymbolNotFound(String),
}
