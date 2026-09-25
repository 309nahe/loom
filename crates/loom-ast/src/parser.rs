//! Multi-language AST parsing and symbol extraction using Tree-sitter.

use std::path::Path;
use std::str::FromStr;

use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language as TsLanguage, Node, Parser, Query, QueryCursor};

use crate::error::{AstError, Result};

/// Supported programming languages for static AST analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// Rust (`.rs`)
    Rust,
    /// TypeScript (`.ts`)
    TypeScript,
    /// TypeScript with JSX (`.tsx`)
    Tsx,
    /// Python (`.py`)
    Python,
}

impl Language {
    /// Detects programming language from a file path extension.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        match ext {
            "rs" => Some(Self::Rust),
            "ts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "py" => Some(Self::Python),
            _ => None,
        }
    }

    /// Returns the tree-sitter language definition.
    #[must_use]
    pub fn tree_sitter_language(self) -> TsLanguage {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }

    /// Returns the SCM query string for extracting symbol definitions.
    ///
    /// Capture contract consumed by `extract_symbols`: every pattern captures the
    /// identifier as `@name` and the whole definition as `@def.<kind>`, where `<kind>`
    /// is a suffix understood by the `capture_name` → `SymbolKind` mapping there. Rust
    /// has no `method_definition` node, so methods arrive as plain `function_item`s; the
    /// innermost-enclosing resolution in the pipeline is what re-attributes them.
    #[must_use]
    pub const fn symbols_query(self) -> &'static str {
        match self {
            Self::Rust => {
                r"
                (function_item name: (identifier) @name) @def.function
                (struct_item name: (type_identifier) @name) @def.struct
                (enum_item name: (type_identifier) @name) @def.enum
                (trait_item name: (type_identifier) @name) @def.trait
                (type_item name: (type_identifier) @name) @def.type_alias
                (mod_item name: (identifier) @name) @def.module
                (const_item name: (identifier) @name) @def.constant
                "
            }
            Self::TypeScript | Self::Tsx => {
                r"
                (function_declaration name: (identifier) @name) @def.function
                (method_definition name: (property_identifier) @name) @def.method
                (class_declaration name: (type_identifier) @name) @def.struct
                (interface_declaration name: (type_identifier) @name) @def.interface
                (type_alias_declaration name: (type_identifier) @name) @def.type_alias
                (enum_declaration name: (identifier) @name) @def.enum
                "
            }
            Self::Python => {
                r"
                (function_definition name: (identifier) @name) @def.function
                (class_definition name: (identifier) @name) @def.struct
                "
            }
        }
    }

    /// Returns the SCM query string for extracting function / method calls.
    ///
    /// Capture contract consumed by `extract_calls`: `@call.name` is the callee as written
    /// (bare identifier, field access, or scoped path) and `@call.site` is the enclosing
    /// call expression whose position becomes the edge's `call_site_line`. Only *call*
    /// syntax is matched; constructing a struct/class literal is not a call edge, so it is
    /// deliberately excluded rather than approximated.
    #[must_use]
    pub const fn calls_query(self) -> &'static str {
        match self {
            Self::Rust => {
                r"
                (call_expression
                  function: [
                    (identifier) @call.name
                    (field_expression field: (field_identifier) @call.name)
                    (scoped_identifier name: (identifier) @call.name)
                  ]) @call.site
                "
            }
            Self::TypeScript | Self::Tsx => {
                r"
                (call_expression
                  function: [
                    (identifier) @call.name
                    (member_expression property: (property_identifier) @call.name)
                  ]) @call.site
                "
            }
            Self::Python => {
                r"
                (call
                  function: [
                    (identifier) @call.name
                    (attribute attribute: (identifier) @call.name)
                  ]) @call.site
                "
            }
        }
    }
}

impl FromStr for Language {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Ok(Self::Rust),
            "typescript" | "ts" => Ok(Self::TypeScript),
            "tsx" => Ok(Self::Tsx),
            "python" | "py" => Ok(Self::Python),
            _ => Err(()),
        }
    }
}

/// An extracted raw function call reference before graph edge linking.
///
/// Calls are intentionally *unresolved* at this stage: the AST layer only knows the callee
/// **name** and its byte position, never which [`SymbolId`] it maps to. Resolution to a
/// deterministic symbol happens in the indexing pipeline, where the whole file set is
/// known. Keeping the two phases separate is what makes cross-file forward references
/// resolvable (see `IndexingPipeline::index_batch`'s two-pass design).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCallReference {
    /// Name of the invoked function or method.
    pub callee_name: String,
    /// 1-indexed line number where the call occurs.
    pub line: u32,
    /// Byte offset of the call site.
    pub byte_range: (usize, usize),
    /// Whether the call site is located within conditional branches.
    ///
    /// Currently always `false`: conditional detection needs parent-node inspection and is
    /// a known follow-up. It is carried in the schema so consumers (risk scoring) can be
    /// written against the final shape without a breaking change.
    pub is_conditional: bool,
}

/// Result of parsing a single source code file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFile {
    /// Discovered symbol definitions.
    pub symbols: Vec<SymbolNode>,
    /// Discovered raw function call references.
    pub raw_calls: Vec<RawCallReference>,
}

/// Pre-compiled, reusable parsing state for one language.
///
/// **This struct is the single biggest performance decision in `loom-ast`.** Compiling a
/// Tree-sitter [`Query`] builds a DFA over the grammar and costs 10–15 ms. Doing that per
/// file destroyed the 5 ms incremental budget, so parsers *and* queries are compiled once
/// per language and reused for every subsequent parse, dropping per-file cost to
/// < 0.15 ms. A `LanguageConfig` is therefore single-threaded state (the inner `Parser`
/// keeps scratch memory), which is why engines are thread-local in the daemon.
struct LanguageConfig {
    parser: Parser,
    symbols_query: Query,
    calls_query: Query,
}

impl LanguageConfig {
    /// Compiles the parser and both query DFAs for `lang`.
    ///
    /// The `expect` calls are safe by construction: the query sources are `const` string
    /// literals in this same module, so a failure here is a programming error detected at
    /// startup rather than a runtime condition on user input.
    fn new(lang: Language) -> Self {
        let ts_lang = lang.tree_sitter_language();
        let mut parser = Parser::new();
        let _ = parser.set_language(&ts_lang);

        let symbols_query = Query::new(&ts_lang, lang.symbols_query())
            .expect("static symbols query compilation must succeed");
        let calls_query = Query::new(&ts_lang, lang.calls_query())
            .expect("static calls query compilation must succeed");

        Self {
            parser,
            symbols_query,
            calls_query,
        }
    }
}

/// AST Extraction engine managing pre-compiled Tree-sitter parsers and queries.
///
/// Owns one [`LanguageConfig`] per supported language. The engine is **not** `Sync`-safe by
/// design: Tree-sitter parsers hold mutable scratch state, so the daemon keeps a
/// thread-local engine per Rayon worker (see `THREAD_AST_ENGINE`) instead of locking.
pub struct AstEngine {
    rust: LanguageConfig,
    typescript: LanguageConfig,
    tsx: LanguageConfig,
    python: LanguageConfig,
}

impl Default for AstEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AstEngine {
    /// Initializes a new `AstEngine` with pre-compiled language parsers and queries.
    ///
    /// Eager compilation pays the ~10–15 ms query-build cost exactly once per thread.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rust: LanguageConfig::new(Language::Rust),
            typescript: LanguageConfig::new(Language::TypeScript),
            tsx: LanguageConfig::new(Language::Tsx),
            python: LanguageConfig::new(Language::Python),
        }
    }

    /// Parses source code for a given file path and returns extracted symbols and calls.
    ///
    /// Two independent query passes run over the same tree: definitions first, then call
    /// sites. Order matters for the caller because definitions are what the pipeline
    /// upserts before it attempts any name→symbol edge resolution.
    ///
    /// # Errors
    /// Returns [`AstError::UnsupportedLanguage`] for extensions outside the supported set
    /// (callers are expected to log-and-skip rather than abort the daemon) and
    /// [`AstError::ParseFailed`] if Tree-sitter yields no tree. Malformed *syntax* is not an
    /// error: Tree-sitter is error-tolerant and still yields a partial tree.
    pub fn parse_source(&mut self, file_path: &Path, source_code: &str) -> Result<ParsedFile> {
        let language = Language::from_path(file_path)
            .ok_or_else(|| AstError::UnsupportedLanguage(file_path.to_path_buf()))?;

        // Borrow only the config for the detected language; other parsers stay untouched
        // and keep their warmed-up state.
        let config = match language {
            Language::Rust => &mut self.rust,
            Language::TypeScript => &mut self.typescript,
            Language::Tsx => &mut self.tsx,
            Language::Python => &mut self.python,
        };

        // `None` as old tree: parsing a whole file from scratch is cheaper than trying to
        // reuse a previous tree, since single-file reparses are the common incremental case.
        let tree = config
            .parser
            .parse(source_code, None)
            .ok_or_else(|| AstError::ParseFailed(file_path.to_path_buf()))?;

        let root_node = tree.root_node();

        let symbols = extract_symbols(
            &config.symbols_query,
            root_node,
            source_code,
            file_path,
            language,
        );
        let raw_calls = extract_calls(&config.calls_query, root_node, source_code);

        Ok(ParsedFile { symbols, raw_calls })
    }
}

/// Walks the symbols query matches and materializes one [`SymbolNode`] per definition.
///
/// Cost discipline (per the "no heavy allocations in inner loops" invariant):
/// `utf8_text` returns borrowed `&str` slices straight out of the source buffer, so node
/// text is only copied into an owned `String` at the moment a symbol is formally upserted.
fn extract_symbols(
    symbols_query: &Query,
    root_node: Node<'_>,
    source_code: &str,
    file_path: &Path,
    language: Language,
) -> Vec<SymbolNode> {
    let capture_names = symbols_query.capture_names();
    // `QueryCursor` is cheap and stack-local; it holds the match iteration state.
    let mut cursor = QueryCursor::new();
    // Streaming iterator (not a std `Iterator`): tree-sitter 0.24 yields matches lazily so
    // large files never materialize every match at once.
    let mut matches = cursor.matches(symbols_query, root_node, source_code.as_bytes());

    let mut symbols = Vec::new();
    let file_path_str = file_path.to_string_lossy();

    while let Some(m) = matches.next() {
        // One match carries both the `name` capture and the `def.*` capture; we must pair
        // them before we can build a node, so both are collected in a first pass.
        let mut name = "";
        let mut kind = SymbolKind::Function;
        let mut def_node = None;

        for capture in m.captures {
            let capture_name = capture_names[capture.index as usize];
            if capture_name == "name" {
                if let Ok(text) = capture.node.utf8_text(source_code.as_bytes()) {
                    name = text;
                }
            } else if capture_name.starts_with("def.") {
                def_node = Some(capture.node);
                // Capture names are the language-agnostic contract between the SCM queries
                // above and this mapping: `def.<kind>` becomes a `SymbolKind` variant.
                kind = match capture_name {
                    "def.method" => SymbolKind::Method,
                    "def.struct" => SymbolKind::Struct,
                    "def.enum" => SymbolKind::Enum,
                    "def.trait" => SymbolKind::Trait,
                    "def.interface" => SymbolKind::Interface,
                    "def.type_alias" => SymbolKind::TypeAlias,
                    "def.constant" => SymbolKind::Constant,
                    "def.module" => SymbolKind::Module,
                    _ => SymbolKind::Function,
                };
            }
        }

        // Skip partial matches (e.g. an anonymous impl block with no `name` capture):
        // emitting a node without a name would poison every name-based edge resolution.
        if let Some(node) = def_node {
            if !name.is_empty() {
                // Tree-sitter rows are 0-indexed; Loom's `line_range` is 1-indexed to match
                // editor/compiler conventions. Saturating conversion rather than `unwrap`
                // keeps this panic-free even for pathological inputs.
                let start_row = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);
                let end_row = u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX);
                let byte_range = (node.start_byte(), node.end_byte());
                let line_range = (start_row, end_row);

                // Signature = first physical line of the definition. This is the value fed
                // into the `SymbolId` hash, so it must be derived only from verified AST text
                // (never reconstructed by an LLM or heuristic).
                let signature = match node.utf8_text(source_code.as_bytes()) {
                    Ok(text) => text.lines().next().unwrap_or(name).trim().to_string(),
                    Err(_) => name.to_string(),
                };

                // Visibility is a *language grammar contract*, not a hardcoded `true`:
                // dead-code detection treats exported symbols as entrypoints, so getting
                // this wrong produces false positives across the whole analysis layer.
                let is_exported = match language {
                    Language::Rust => signature.starts_with("pub"),
                    Language::TypeScript | Language::Tsx => signature.starts_with("export"),
                    Language::Python => !name.starts_with('_'),
                };

                // Namespace hierarchy is intentionally empty: Loom resolves symbols by
                // (file, name, signature) rather than module path, which keeps IDs stable
                // when a symbol is moved between modules that export it identically.
                let id = SymbolId::derive(&file_path_str, &[], name, &signature);

                // `epoch = 1`: first generation. The pipeline bumps it on re-index so cache
                // consumers can detect that a node was refreshed even if its ID is unchanged.
                symbols.push(SymbolNode::new(
                    id,
                    name,
                    kind,
                    file_path,
                    byte_range,
                    line_range,
                    None,
                    signature,
                    is_exported,
                    1,
                ));
            }
        }
    }

    symbols
}

/// Walks the calls query matches and materializes unresolved [`RawCallReference`]s.
///
/// Fully qualified callee paths (`module::func`, `obj.method`) are intentionally captured
/// as written in the source; trimming or rewriting them here would make resolution
/// heuristic rather than syntactic.
fn extract_calls(
    calls_query: &Query,
    root_node: Node<'_>,
    source_code: &str,
) -> Vec<RawCallReference> {
    let capture_names = calls_query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(calls_query, root_node, source_code.as_bytes());

    let mut raw_calls = Vec::new();

    while let Some(m) = matches.next() {
        // Same pairing requirement as definitions: `call.name` gives the callee,
        // `call.site` gives the enclosing expression used for position reporting.
        let mut callee_name = String::new();
        let mut call_node = None;

        for capture in m.captures {
            let capture_name = capture_names[capture.index as usize];
            if capture_name == "call.name" {
                if let Ok(text) = capture.node.utf8_text(source_code.as_bytes()) {
                    callee_name = text.to_string();
                }
            } else if capture_name == "call.site" {
                call_node = Some(capture.node);
            }
        }

        if let Some(node) = call_node {
            if !callee_name.is_empty() {
                let line = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);
                let byte_range = (node.start_byte(), node.end_byte());

                raw_calls.push(RawCallReference {
                    callee_name,
                    line,
                    byte_range,
                    is_conditional: false,
                });
            }
        }
    }

    raw_calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_ast_extraction() {
        let mut engine = AstEngine::new();
        let source = r"
        pub struct User {
            pub id: u64,
            pub name: String,
        }

        pub enum Role {
            Admin,
            User,
        }

        pub fn process_order(user_id: u64) -> bool {
            validate_user(user_id);
            true
        }
        ";

        let parsed = engine
            .parse_source(Path::new("src/order.rs"), source)
            .expect("parse rust source");

        assert_eq!(parsed.symbols.len(), 3);
        assert_eq!(parsed.symbols[0].name, "User");
        assert_eq!(parsed.symbols[0].kind, SymbolKind::Struct);

        assert_eq!(parsed.symbols[1].name, "Role");
        assert_eq!(parsed.symbols[1].kind, SymbolKind::Enum);

        assert_eq!(parsed.symbols[2].name, "process_order");
        assert_eq!(parsed.symbols[2].kind, SymbolKind::Function);

        assert_eq!(parsed.raw_calls.len(), 1);
        assert_eq!(parsed.raw_calls[0].callee_name, "validate_user");
        assert_eq!(parsed.raw_calls[0].line, 13);
    }

    #[test]
    fn test_typescript_ast_extraction() {
        let mut engine = AstEngine::new();
        let source = r"
        interface AuthConfig {
            secret: string;
        }

        class AuthService {
            login(username: string): boolean {
                return checkPassword(username);
            }
        }
        ";

        let parsed = engine
            .parse_source(Path::new("src/auth.ts"), source)
            .expect("parse typescript source");

        assert_eq!(parsed.symbols.len(), 3);
        assert_eq!(parsed.symbols[0].name, "AuthConfig");
        assert_eq!(parsed.symbols[0].kind, SymbolKind::Interface);

        assert_eq!(parsed.symbols[1].name, "AuthService");
        assert_eq!(parsed.symbols[1].kind, SymbolKind::Struct);

        assert_eq!(parsed.symbols[2].name, "login");
        assert_eq!(parsed.symbols[2].kind, SymbolKind::Method);

        assert_eq!(parsed.raw_calls.len(), 1);
        assert_eq!(parsed.raw_calls[0].callee_name, "checkPassword");
    }

    #[test]
    fn test_python_ast_extraction() {
        let mut engine = AstEngine::new();
        let source = r"
class PaymentProcessor:
    def process(self, amount):
        verify_funds(amount)
        ";

        let parsed = engine
            .parse_source(Path::new("payment.py"), source)
            .expect("parse python source");

        assert_eq!(parsed.symbols.len(), 2);
        assert_eq!(parsed.symbols[0].name, "PaymentProcessor");
        assert_eq!(parsed.symbols[0].kind, SymbolKind::Struct);

        assert_eq!(parsed.symbols[1].name, "process");
        assert_eq!(parsed.symbols[1].kind, SymbolKind::Function);

        assert_eq!(parsed.raw_calls.len(), 1);
        assert_eq!(parsed.raw_calls[0].callee_name, "verify_funds");
    }
}
