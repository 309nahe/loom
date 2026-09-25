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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCallReference {
    /// Name of the invoked function or method.
    pub callee_name: String,
    /// 1-indexed line number where the call occurs.
    pub line: u32,
    /// Byte offset of the call site.
    pub byte_range: (usize, usize),
    /// Whether the call site is located within conditional branches.
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

struct LanguageConfig {
    parser: Parser,
    symbols_query: Query,
    calls_query: Query,
}

impl LanguageConfig {
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
    /// # Errors
    /// Returns [`AstError`] if the language is unsupported or parsing fails.
    pub fn parse_source(&mut self, file_path: &Path, source_code: &str) -> Result<ParsedFile> {
        let language = Language::from_path(file_path)
            .ok_or_else(|| AstError::UnsupportedLanguage(file_path.to_path_buf()))?;

        let config = match language {
            Language::Rust => &mut self.rust,
            Language::TypeScript => &mut self.typescript,
            Language::Tsx => &mut self.tsx,
            Language::Python => &mut self.python,
        };

        let tree = config
            .parser
            .parse(source_code, None)
            .ok_or_else(|| AstError::ParseFailed(file_path.to_path_buf()))?;

        let root_node = tree.root_node();

        let symbols = extract_symbols(&config.symbols_query, root_node, source_code, file_path);
        let raw_calls = extract_calls(&config.calls_query, root_node, source_code);

        Ok(ParsedFile { symbols, raw_calls })
    }
}

fn extract_symbols(
    symbols_query: &Query,
    root_node: Node<'_>,
    source_code: &str,
    file_path: &Path,
) -> Vec<SymbolNode> {
    let capture_names = symbols_query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(symbols_query, root_node, source_code.as_bytes());

    let mut symbols = Vec::new();
    let file_path_str = file_path.to_string_lossy();

    while let Some(m) = matches.next() {
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

        if let Some(node) = def_node {
            if !name.is_empty() {
                let start_row = u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX);
                let end_row = u32::try_from(node.end_position().row + 1).unwrap_or(u32::MAX);
                let byte_range = (node.start_byte(), node.end_byte());
                let line_range = (start_row, end_row);

                let signature = match node.utf8_text(source_code.as_bytes()) {
                    Ok(text) => text.lines().next().unwrap_or(name).trim().to_string(),
                    Err(_) => name.to_string(),
                };

                let id = SymbolId::derive(&file_path_str, &[], name, &signature);

                symbols.push(SymbolNode::new(
                    id, name, kind, file_path, byte_range, line_range, None, signature, true, 1,
                ));
            }
        }
    }

    symbols
}

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
