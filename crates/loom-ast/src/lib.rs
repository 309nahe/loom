//! # Loom AST
//!
//! Multi-language concrete syntax tree parsing and symbol extraction layer using Tree-sitter.
//!
//! ## Invariants
//! - **Multi-Language Determinism**: Parses Rust, TypeScript, TSX, and Python into standardized [`SymbolNode`] representations.
//! - **Zero-Panic Parsing**: Gracefully degrades on syntax errors or invalid trees without daemon crashes.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]

pub mod error;
pub mod parser;

pub use error::{AstError, Result};
pub use parser::{AstEngine, Language, ParsedFile, RawCallReference};
