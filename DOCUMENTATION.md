# Loom Engineering Documentation & Changelog

## Overview
This document tracks engineering decisions, architectural trade-offs, challenges faced during development, and a chronological changelog for the **Loom** daemon project in accordance with [AGENTS.md](AGENTS.md).

---

## 1. Project Inception & Initial Setup

### 1.1 Scope & Context
- Initialized the repository for **Loom** (Local Code-Topography Daemon & MCP Server).
- Established the core technical blueprint in [IDEA.md](IDEA.md) and autonomous contributor rules in [AGENTS.md](AGENTS.md).
- Decomposed Phase 1 (Core Engine) into atomic, trackable GitHub issues.

### 1.2 GitHub Issues Created for Phase 1
1. **[Issue #1](https://github.com/309nahe/loom/issues/1) — `feat(core): implement core symbol schemas and deterministic BLAKE3 hashing`**
   - Implement `SymbolId` (128-bit truncated BLAKE3 hash: `[u8; 16]`), `SymbolNode`, `SymbolKind`, `DependencyEdge`, `EdgeKind`.
   - Ensure serde derive compatibility and strict deterministic hashing.
2. **[Issue #2](https://github.com/309nahe/loom/issues/2) — `feat(graph): implement CodeGraph with petgraph and bidirectional lookup maps`**
   - Implement `CodeGraph` containing `DiGraph<SymbolNode, DependencyEdge>`, `symbol_to_node`, and `file_to_symbols`.
   - Provide atomic node/edge mutations and file-level invalidation.
3. **[Issue #3](https://github.com/309nahe/loom/issues/3) — `feat(ast): build multi-language Tree-sitter AST extraction layer`**
   - Tree-sitter query extraction for Rust, TypeScript, and Python.
   - Zero-copy AST traversal targeting sub-millisecond single-file parse times.
4. **[Issue #4](https://github.com/309nahe/loom/issues/4) — `feat(engine): connect debounced file watcher to incremental AST re-indexing pipeline`**
   - Integrate `notify-debouncer-mini` (50ms window) and `rayon` thread pool.
   - Enforce $< 5\,\text{ms}$ incremental invalidation and update cycles.

---

## 2. Issue Deep-Dives & Implementation Logs

### 2.1 Issue #1: Core Symbol Schemas & Deterministic BLAKE3 Hashing

#### Why We Did This First (Rationale & Pre-requisite Analysis)
- **Zero-Dependency Domain Foundation**: In static code topography, the `SymbolNode` and `DependencyEdge` structs are the fundamental leaf models. Every downstream layer—`CodeGraph` (petgraph storage), Tree-sitter AST extractors, persistent redb cache tables, and JSON-RPC Model Context Protocol (MCP) responses—must agree on exact schema definitions.
- **Enforcing Determinism Invariant from Day One**: Non-deterministic symbol IDs (e.g. UUIDs, memory pointers, auto-incrementing integers) are catastrophic for incremental dirty-file re-indexing and persistence. If a symbol's ID changes across daemon restarts or file re-parses, incremental graph diffing and caching break completely. Implementing deterministic BLAKE3 hashing first guarantees mathematical reproducibility:
  $$\text{SymbolID} = \text{BLAKE3}(\text{file\_path} \parallel \text{namespace\_hierarchy} \parallel \text{symbol\_name} \parallel \text{signature})$$
- **Preventing Downstream Cascading Refactors**: Establishing robust types with serialization (`serde`), error handling (`thiserror`), and strict zero-allocation boundaries early isolates data modeling from graph algorithm complexities.

#### What We Did (Technical Implementation)
1. **Workspace & Crate Setup**:
   - Initialized a modern Cargo workspace targeting **Rust 2024 Edition / 1.85+**.
   - Created the core domain crate `crates/loom-core` with zero internal crate dependencies.
2. **Deterministic `SymbolId` (`crates/loom-core/src/id.rs`)**:
   - Represented as a compact 16-byte fixed array (`[u8; 16]`) containing the truncated 128-bit BLAKE3 hash.
   - Implemented `SymbolId::derive(...)` with explicit null-byte (`\0`) and namespace-delimiter (`::`) framing between variable-length inputs (`file_path`, `namespace_hierarchy`, `symbol_name`, `signature`) to mathematically prevent collision/concatenation attacks across variable-length components.
   - Added hex serialization (`to_hex()`, `from_hex()`, `FromStr`, `Display`) and human-readable Serde dispatch (hex string in JSON/MCP; raw bytes in binary stores).
3. **Symbol & Node Models (`crates/loom-core/src/symbol.rs`)**:
   - Defined `SymbolKind` covering all language constructs: `Function`, `Method`, `Struct`, `Enum`, `Trait`, `Interface`, `TypeAlias`, `Constant`, and `Module`.
   - Defined `SymbolNode` with exact byte ranges, line ranges, signatures, docstrings, visibility flags, and generational `epoch` counters for cache tracking.
4. **Dependency Edge Schemas (`crates/loom-core/src/edge.rs`)**:
   - Defined `EdgeKind` (`Calls`, `Instantiates`, `Implements`, `ReferencesType`, `Inherits`, `Imports`).
   - Defined `DependencyEdge` with call site line numbers and conditional execution flags (`is_conditional`).
5. **Typed Error Hierarchy (`crates/loom-core/src/error.rs`)**:
   - Implemented `LoomError` utilizing `thiserror` for zero-panic, typed error propagation across parsing and decoding.

#### Results & Verification
- **Formatting**: `cargo fmt --check` passed cleanly across all crate targets.
- **Strict Linting**: `cargo clippy --all-targets --all-features -- -D warnings` completed with zero warnings under `#![warn(clippy::pedantic)]`.
- **Test Suite**: 9 unit tests passed in 0.00s covering:
  - BLAKE3 hash stability across identical inputs.
  - Signature and namespace collision resistance.
  - Hex parsing and malformed string rejection.
  - Serde JSON bidirectional round-trip serialization.

---

## 3. Technical Decisions & Swaps in Ideas

- **Framed Delimiters in BLAKE3 Hashing**: Instead of naive byte concatenation `a + b + c`, inserted null delimiters `\0` and `::` between segments to eliminate collision ambiguity where boundary shifts could otherwise generate identical digests.
- **Dual Serde Representation for `SymbolId`**: Formatted as 32-character hex strings in human-readable serializers (`serde_json`) for clean MCP inspection, while retaining efficient 16-byte binary serialization for disk/memory caches.
- **Tool Protocol Enforcement**: Adhered to `AGENTS.md` prioritizing GitHub MCP tools (`github-mcp-server`) for issue inspection, status updates, and authenticated remote synchronization.

---

## 4. Challenges Faced & Resolutions

| Challenge | Impact | Resolution |
| :--- | :--- | :--- |
| **Git Push HTTPS Authentication** | Standard `git push` over HTTPS prompted interactively for credentials. | Utilized GitHub MCP `push_files` to push commits directly and deterministically through the authenticated API. |
| **Sandbox Subprocess Reset** | Transient sandbox socket reset during process startup. | Managed commands through resilient execution loops and leveraged native file/MCP tools for state management. |

---

## 5. Changelog

### [2026-09-25] - Issue #1 Implementation: Core Schemas & Deterministic Hashing
- **Added**: `crates/loom-core` crate in Cargo workspace.
- **Implemented**: `SymbolId` 128-bit truncated BLAKE3 deterministic hashing with boundary-safe delimiter framing.
- **Implemented**: `SymbolNode` and `SymbolKind` semantic representations.
- **Implemented**: `DependencyEdge` and `EdgeKind` dependency graph edge schemas.
- **Implemented**: `LoomError` typed error enum using `thiserror`.
- **Added**: Comprehensive unit test suite for determinism, collision resistance, and serde serialization.
- **Verified**: Passed `cargo fmt`, `cargo clippy --all-targets --all-features -- -D warnings`, and `cargo test`.
- **Updated**: [DOCUMENTATION.md](DOCUMENTATION.md) and closed [Issue #1](https://github.com/309nahe/loom/issues/1).

### [2026-09-25] - Repository Initialization & Phase 1 Planning
- **Added**: [IDEA.md](IDEA.md) architecture blueprint and roadmap.
- **Added**: [AGENTS.md](AGENTS.md) guidelines, performance constraints, and MCP protocol priority rules.
- **Created**: GitHub repository `309nahe/loom` on GitHub.
- **Created**: GitHub issues [#1](https://github.com/309nahe/loom/issues/1), [#2](https://github.com/309nahe/loom/issues/2), [#3](https://github.com/309nahe/loom/issues/3), and [#4](https://github.com/309nahe/loom/issues/4) covering Phase 1 milestones.
- **Added**: [DOCUMENTATION.md](DOCUMENTATION.md) tracking development decisions and changelog.
