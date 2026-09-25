//! Dependency edge schema connecting semantic symbols in the dependency graph.

use serde::{Deserialize, Serialize};

/// Categorization of the structural or behavioral relationship between symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Function or method invocation (e.g., `process_payment()` calls `validate()`).
    Calls,
    /// Instantiation of a struct, class, or type instance (e.g., `User { ... }`).
    Instantiates,
    /// Type implementing an interface or trait contract (e.g., `impl Display for Foo`).
    Implements,
    /// Parameter, field, or return type reference (e.g., `fn get_user() -> User`).
    ReferencesType,
    /// Subclassing or structural inheritance hierarchy.
    Inherits,
    /// File or module import statement (e.g., `use crate::auth::User;`).
    Imports,
}

impl EdgeKind {
    /// Returns a human-readable identifier for the edge kind.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Calls => "calls",
            Self::Instantiates => "instantiates",
            Self::Implements => "implements",
            Self::ReferencesType => "references_type",
            Self::Inherits => "inherits",
            Self::Imports => "imports",
        }
    }
}

impl std::fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A directed edge in the code graph representing a dependency between two symbols.
///
/// Orientation is always **source = dependant → target = dependency**, i.e. caller to callee.
/// All traversal helpers in `loom-graph` rely on that convention, so reversing it here would
/// silently invert every blast-radius calculation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyEdge {
    /// The specific category of dependency.
    pub kind: EdgeKind,
    /// 1-indexed line number in the source file where this dependency originates.
    pub call_site_line: u32,
    /// Whether this dependency occurs inside a branching context (if, match, loop).
    ///
    /// Recorded so risk scoring can later discount dependencies that only execute on some
    /// code paths, rather than treating every call site as certain.
    pub is_conditional: bool,
}

impl DependencyEdge {
    /// Constructs a new `DependencyEdge`.
    #[must_use]
    pub const fn new(kind: EdgeKind, call_site_line: u32, is_conditional: bool) -> Self {
        Self {
            kind,
            call_site_line,
            is_conditional,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_edge_serde() {
        let edge = DependencyEdge::new(EdgeKind::Calls, 42, true);
        let json = serde_json::to_string(&edge).expect("serialize edge");
        let deserialized: DependencyEdge = serde_json::from_str(&json).expect("deserialize edge");
        assert_eq!(edge, deserialized);
    }
}
