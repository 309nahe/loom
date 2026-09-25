//! Typed error definitions for Loom core primitives.

use thiserror::Error;

/// Core error types encountered during symbol processing and hashing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LoomError {
    /// Invalid hex string representation for a `SymbolId`.
    #[error("invalid hex format for SymbolId: {0}")]
    InvalidSymbolIdHex(String),

    /// Invalid byte length when constructing a `SymbolId`.
    #[error("invalid byte length for SymbolId: expected 16 bytes, got {0}")]
    InvalidSymbolIdLength(usize),

    /// Serialization or deserialization failure.
    #[error("serialization error: {0}")]
    SerializationError(String),
}

/// A specialized Result type for Loom core operations.
pub type Result<T> = std::result::Result<T, LoomError>;
