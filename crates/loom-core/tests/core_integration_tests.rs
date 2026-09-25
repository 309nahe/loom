use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::error::LoomError;
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use std::collections::HashSet;

#[test]
fn test_blake3_determinism_and_collision_resistance() {
    let mut hashes = HashSet::new();

    // 1. Permutations of file_path and symbol_name with delimiter safety
    let id1 = SymbolId::derive("src/foo.rs", &["bar"], "baz", "fn baz()");
    let id2 = SymbolId::derive("src/foobar.rs", &[], "baz", "fn baz()");
    let id3 = SymbolId::derive("src/foo.rs", &[], "bar_baz", "fn baz()");
    let id4 = SymbolId::derive("src/foo.rs", &["bar"], "baz", "fn baz() -> i32");

    assert!(hashes.insert(id1));
    assert!(hashes.insert(id2));
    assert!(hashes.insert(id3));
    assert!(hashes.insert(id4));
    assert_eq!(hashes.len(), 4);

    // Identical parameters must produce identical SymbolId
    let id1_duplicate = SymbolId::derive("src/foo.rs", &["bar"], "baz", "fn baz()");
    assert_eq!(id1, id1_duplicate);
    assert_eq!(id1.to_hex(), id1_duplicate.to_hex());
}

#[test]
fn test_symbol_id_byte_conversion_and_hex() {
    let raw_bytes: [u8; 16] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54, 0x32,
        0x10,
    ];

    let sym_id = SymbolId::from_bytes(raw_bytes);
    assert_eq!(sym_id.into_bytes(), raw_bytes);
    assert_eq!(sym_id.as_bytes(), &raw_bytes);

    let hex_str = sym_id.to_hex();
    assert_eq!(hex_str, "0123456789abcdeffedcba9876543210");

    let from_hex = SymbolId::from_hex(&hex_str).expect("decode valid hex");
    assert_eq!(from_hex, sym_id);

    // Invalid hex length
    assert!(matches!(
        SymbolId::from_hex("012345"),
        Err(LoomError::InvalidSymbolIdHex(_))
    ));

    // Non-hex characters
    assert!(matches!(
        SymbolId::from_hex("0123456789abcdefghijklmnopqrstuv"),
        Err(LoomError::InvalidSymbolIdHex(_))
    ));
}

#[test]
fn test_all_symbol_kinds_and_edges_serde() {
    let kinds = [
        SymbolKind::Function,
        SymbolKind::Method,
        SymbolKind::Struct,
        SymbolKind::Enum,
        SymbolKind::Trait,
        SymbolKind::Interface,
        SymbolKind::TypeAlias,
        SymbolKind::Constant,
        SymbolKind::Module,
    ];

    for kind in kinds {
        let json = serde_json::to_string(&kind).expect("serialize SymbolKind");
        let decoded: SymbolKind = serde_json::from_str(&json).expect("deserialize SymbolKind");
        assert_eq!(kind, decoded);
    }

    let edges = [
        EdgeKind::Calls,
        EdgeKind::Instantiates,
        EdgeKind::Implements,
        EdgeKind::ReferencesType,
        EdgeKind::Inherits,
        EdgeKind::Imports,
    ];

    for edge in edges {
        let dep_edge = DependencyEdge::new(edge, 100, true);
        let json = serde_json::to_string(&dep_edge).expect("serialize DependencyEdge");
        let decoded: DependencyEdge =
            serde_json::from_str(&json).expect("deserialize DependencyEdge");
        assert_eq!(dep_edge, decoded);
        assert_eq!(decoded.kind, edge);
        assert_eq!(decoded.call_site_line, 100);
        assert!(decoded.is_conditional);
    }
}

#[test]
fn test_symbol_node_complete_roundtrip() {
    let id = SymbolId::derive(
        "src/core/cache.rs",
        &["core", "cache"],
        "LruCache",
        "pub struct LruCache<K, V>",
    );

    let node = SymbolNode::new(
        id,
        "LruCache",
        SymbolKind::Struct,
        "src/core/cache.rs",
        (120, 450),
        (10, 35),
        Some("High-performance LRU cache implementation.".to_string()),
        "pub struct LruCache<K, V>",
        true,
        42,
    );

    let json = serde_json::to_string_pretty(&node).expect("serialize SymbolNode");
    let deserialized: SymbolNode = serde_json::from_str(&json).expect("deserialize SymbolNode");

    assert_eq!(node, deserialized);
    assert_eq!(deserialized.epoch, 42);
    assert_eq!(
        deserialized.docstring.as_deref(),
        Some("High-performance LRU cache implementation.")
    );
}
