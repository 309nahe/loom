//! Symbol node and category representations for the code topography graph.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::id::SymbolId;

/// Classification of a semantic unit within the codebase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    /// Standalone function definition.
    Function,
    /// Associated method within a struct, class, or impl block.
    Method,
    /// Data structure / class declaration.
    Struct,
    /// Enumeration type.
    Enum,
    /// Rust Trait or abstract contract.
    Trait,
    /// TypeScript / Go / Java interface contract.
    Interface,
    /// Type alias / typedef.
    TypeAlias,
    /// Constant or static value.
    Constant,
    /// Module or namespace boundary.
    Module,
}

impl SymbolKind {
    /// Returns true if this symbol represents a callable item (function or method).
    ///
    /// Load-bearing classification, not a convenience: the indexing pipeline uses it to
    /// attribute a call site to the *innermost callable* rather than to its enclosing class
    /// or module, and the analysis layer uses it to separate callers from containers.
    #[must_use]
    pub const fn is_callable(&self) -> bool {
        matches!(self, Self::Function | Self::Method)
    }

    /// Returns true if this symbol represents a type definition.
    #[must_use]
    pub const fn is_type(&self) -> bool {
        matches!(
            self,
            Self::Struct | Self::Enum | Self::Trait | Self::Interface | Self::TypeAlias
        )
    }

    /// Returns a human-readable string identifier for the symbol kind.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Interface => "interface",
            Self::TypeAlias => "type_alias",
            Self::Constant => "constant",
            Self::Module => "module",
        }
    }
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A node in the code graph representing an extracted semantic symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolNode {
    /// Deterministic 128-bit truncated BLAKE3 identifier.
    pub id: SymbolId,
    /// Identifier name (e.g., `authenticate_user`).
    pub name: String,
    /// Categorization of this symbol.
    pub kind: SymbolKind,
    /// Normalized repository-relative file path where the symbol is defined.
    pub file_path: PathBuf,
    /// Byte offsets `(start_byte, end_byte)` in the source file.
    pub byte_range: (usize, usize),
    /// 1-indexed line boundaries `(start_line, end_line)` in the source file.
    pub line_range: (u32, u32),
    /// Extracted documentation comment, if present.
    pub docstring: Option<String>,
    /// Exact declaration signature (e.g. `pub fn authenticate(req: Request) -> Result<User>`).
    pub signature: String,
    /// Whether this symbol is exported / publicly visible outside its immediate scope.
    ///
    /// Consumed by the analysis layer as a root classification: exported symbols and tests
    /// are treated as entrypoints, which is what keeps dead-code detection from flagging
    /// public API as unused.
    pub is_exported: bool,
    /// Incremental generational version counter for cache invalidation.
    ///
    /// The `SymbolId` is a content hash, so it is *unchanged* when a file is re-indexed even
    /// though the node's line ranges and signature may have moved. `epoch` is what lets a
    /// persistent cache or a long-lived reader tell "same symbol, new generation" apart from
    /// "untouched", without hashing the whole node again.
    pub epoch: u64,
}

impl SymbolNode {
    /// Creates a new `SymbolNode` instance.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: SymbolId,
        name: impl Into<String>,
        kind: SymbolKind,
        file_path: impl AsRef<Path>,
        byte_range: (usize, usize),
        line_range: (u32, u32),
        docstring: Option<String>,
        signature: impl Into<String>,
        is_exported: bool,
        epoch: u64,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            file_path: file_path.as_ref().to_path_buf(),
            byte_range,
            line_range,
            docstring,
            signature: signature.into(),
            is_exported,
            epoch,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_kind_properties() {
        assert!(SymbolKind::Function.is_callable());
        assert!(SymbolKind::Method.is_callable());
        assert!(!SymbolKind::Struct.is_callable());

        assert!(SymbolKind::Struct.is_type());
        assert!(SymbolKind::Trait.is_type());
        assert!(!SymbolKind::Function.is_type());
    }

    #[test]
    fn test_symbol_node_serde() {
        let id = SymbolId::derive("src/lib.rs", &[], "init", "pub fn init()");
        let node = SymbolNode::new(
            id,
            "init",
            SymbolKind::Function,
            "src/lib.rs",
            (0, 50),
            (1, 5),
            Some("Initializes the subsystem.".to_string()),
            "pub fn init()",
            true,
            1,
        );

        let json = serde_json::to_string(&node).expect("serialize node");
        let deserialized: SymbolNode = serde_json::from_str(&json).expect("deserialize node");
        assert_eq!(node, deserialized);
    }
}
