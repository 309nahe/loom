//! Error types for Tree-sitter AST extraction.

use std::path::PathBuf;
use thiserror::Error;

/// Errors occurring during AST parsing and symbol extraction.
#[derive(Debug, Error)]
pub enum AstError {
    /// File type or extension is not supported by any configured language parser.
    #[error("unsupported language for file: {0}")]
    UnsupportedLanguage(PathBuf),

    /// Tree-sitter failed to parse the source buffer into a valid syntax tree.
    #[error("failed to parse syntax tree for file: {0}")]
    ParseFailed(PathBuf),

    /// Tree-sitter SCM query compilation failed.
    #[error("query compilation error for {language}: {message}")]
    QueryCompilationError {
        /// Language name
        language: &'static str,
        /// Underlying error description
        message: String,
    },

    /// I/O error reading source file.
    #[error("I/O error reading {path}: {source}")]
    IoError {
        /// Path to the file
        path: PathBuf,
        /// Underlying I/O error
        #[source]
        source: std::io::Error,
    },
}

/// A specialized Result type for AST operations.
pub type Result<T> = std::result::Result<T, AstError>;
