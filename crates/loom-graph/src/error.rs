//! Error types for code graph operations.

use loom_core::id::SymbolId;
use thiserror::Error;

/// Errors arising during graph manipulation or queries.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GraphError {
    /// Referenced symbol was not found in the graph.
    #[error("symbol not found: {0}")]
    SymbolNotFound(SymbolId),

    /// Source or target symbol was missing when attempting to create a dependency edge.
    #[error("cannot create edge: source or target symbol not found in graph")]
    EndpointNotFound,

    /// Graph serialization or deserialization failure.
    #[error("graph serialization error: {0}")]
    SerializationError(String),
}

/// A specialized Result type for graph operations.
pub type Result<T> = std::result::Result<T, GraphError>;
