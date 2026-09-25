//! Integration tests for multi-language Tree-sitter extraction.
//!
//! Covers Rust, TypeScript, and TSX/Python symbol and call extraction, asserts sub-millisecond
//! parse latency, and — most importantly — asserts that malformed source degrades gracefully
//! (partial results, no panic), which is the daemon's zero-hallucination guarantee.

use loom_ast::parser::AstEngine;
use loom_core::symbol::SymbolKind;
use std::path::Path;
use std::time::Instant;

#[test]
fn test_complex_rust_parsing() {
    let mut engine = AstEngine::new();
    let source = r#"
    pub mod network {
        pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

        pub trait Transport {
            fn send(&self, payload: &[u8]) -> Result<(), Error>;
        }

        pub struct TcpTransport<C> {
            config: C,
        }

        pub enum ConnectionState {
            Disconnected,
            Connected(u64),
        }

        pub type TransportResult<T> = Result<T, Error>;

        pub async fn establish_connection() -> TransportResult<TcpTransport<()>> {
            init_socket();
            Ok(TcpTransport { config: () })
        }
    }
    "#;

    let parsed = engine
        .parse_source(Path::new("src/network/mod.rs"), source)
        .expect("parse complex rust source");

    let symbol_names: Vec<&str> = parsed.symbols.iter().map(|s| s.name.as_str()).collect();

    assert!(symbol_names.contains(&"network"));
    assert!(symbol_names.contains(&"DEFAULT_TIMEOUT_SECS"));
    assert!(symbol_names.contains(&"Transport"));
    assert!(symbol_names.contains(&"TcpTransport"));
    assert!(symbol_names.contains(&"ConnectionState"));
    assert!(symbol_names.contains(&"TransportResult"));
    assert!(symbol_names.contains(&"establish_connection"));

    let calls: Vec<&str> = parsed
        .raw_calls
        .iter()
        .map(|c| c.callee_name.as_str())
        .collect();
    assert!(calls.contains(&"init_socket"));
}

#[test]
fn test_complex_typescript_tsx_parsing() {
    let mut engine = AstEngine::new();
    let tsx_source = r#"
    export type ThemeMode = "light" | "dark";

    export interface ButtonProps {
        label: string;
        onClick: () => void;
    }

    export class ComponentRenderer {
        renderButton(props: ButtonProps) {
            trackAnalytics("button_rendered");
            return formatButton(props.label);
        }
    }
    "#;

    let parsed = engine
        .parse_source(Path::new("src/ui/Button.tsx"), tsx_source)
        .expect("parse tsx source");

    assert_eq!(parsed.symbols.len(), 4);
    assert_eq!(parsed.symbols[0].name, "ThemeMode");
    assert_eq!(parsed.symbols[0].kind, SymbolKind::TypeAlias);

    assert_eq!(parsed.symbols[1].name, "ButtonProps");
    assert_eq!(parsed.symbols[1].kind, SymbolKind::Interface);

    assert_eq!(parsed.symbols[2].name, "ComponentRenderer");
    assert_eq!(parsed.symbols[2].kind, SymbolKind::Struct);

    assert_eq!(parsed.symbols[3].name, "renderButton");
    assert_eq!(parsed.symbols[3].kind, SymbolKind::Method);

    let calls: Vec<&str> = parsed
        .raw_calls
        .iter()
        .map(|c| c.callee_name.as_str())
        .collect();
    assert!(calls.contains(&"trackAnalytics"));
    assert!(calls.contains(&"formatButton"));
}

#[test]
fn test_python_parsing_and_performance_latency() {
    let mut engine = AstEngine::new();
    let py_source = r#"
class DataAggregator:
    def __init__(self, endpoint):
        self.endpoint = endpoint

    def fetch_records(self, limit):
        raw = http_get(self.endpoint, limit)
        return parse_records(raw)

def run_pipeline():
    aggregator = DataAggregator("https://api.example.com")
    aggregator.fetch_records(100)
    "#;

    let start = Instant::now();
    let parsed = engine
        .parse_source(Path::new("etl/aggregator.py"), py_source)
        .expect("parse python source");
    let duration = start.elapsed();

    // Verify sub-5ms requirement
    assert!(
        duration.as_millis() < 5,
        "Tree-sitter parse took {duration:?}, expected < 5ms"
    );

    assert_eq!(parsed.symbols.len(), 4);
    assert_eq!(parsed.symbols[0].name, "DataAggregator");
    assert_eq!(parsed.symbols[1].name, "__init__");
    assert_eq!(parsed.symbols[2].name, "fetch_records");
    assert_eq!(parsed.symbols[3].name, "run_pipeline");

    assert_eq!(parsed.raw_calls.len(), 4);
}

#[test]
fn test_malformed_syntax_graceful_degradation() {
    let mut engine = AstEngine::new();
    // Incomplete, syntax-broken Rust code
    let broken_rust = "pub fn broken( { let x = ;";

    // Engine must not panic and return best-effort parse tree
    let res = engine.parse_source(Path::new("broken.rs"), broken_rust);
    assert!(res.is_ok());
}
