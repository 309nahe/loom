//! # Loom Core
//!
//! Core domain types, schemas, and deterministic hashing primitives for the Loom code topography daemon.
//!
//! ## Invariants
//! - **Deterministic BLAKE3 Identification**: All symbol identifiers ([`SymbolId`]) are computed
//!   deterministically via a 128-bit truncated BLAKE3 hash:
//!   $$\text{SymbolID} = \text{BLAKE3}(\text{file\_path} \parallel \text{namespace\_hierarchy} \parallel \text{symbol\_name} \parallel \text{signature})$$
//! - **No UUIDs / Ephemeral IDs**: Prevents non-deterministic graph states and cache misses.
//! - **Zero-Panic Production Paths**: All errors are bubbled up using typed [`LoomError`] enums.

#![warn(missing_docs)]
#![warn(clippy::pedantic)]

pub mod edge;
pub mod error;
pub mod id;
pub mod symbol;

pub use edge::{DependencyEdge, EdgeKind};
pub use error::{LoomError, Result};
pub use id::SymbolId;
pub use symbol::{SymbolKind, SymbolNode};
