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

### 2.2 Issue #2: CodeGraph with Petgraph & Bidirectional Lookup Maps

#### Why We Did This Next (Rationale & Pre-requisite Analysis)
- **Central Topological State Engine**: With domain types established in `loom-core`, the in-memory directed dependency graph (`CodeGraph`) is the central state store required by both AST extraction and file indexing pipelines.
- **Solving the Petgraph Index Compaction Invariant Early**: In Petgraph's `DiGraph`, removing a node executes a swap-removal (moving the highest-indexed node into the removed slot). Without specialized synchronization logic, external lookup maps (`SymbolId -> NodeIndex`) quickly point to wrong or corrupted graph nodes. Solving this in isolation prevents difficult-to-debug data corruption in higher-level pipelines.

#### What We Did (Technical Implementation)
1. **Crate Setup (`crates/loom-graph`)**:
   - Created `loom-graph` depending on `loom-core` and `petgraph`.
2. **`CodeGraph` Structure (`crates/loom-graph/src/graph.rs`)**:
   - Maintained `graph: DiGraph<SymbolNode, DependencyEdge>`, `symbol_to_node: HashMap<SymbolId, NodeIndex>`, and `file_to_symbols: HashMap<PathBuf, Vec<SymbolId>>`.
3. **Atomic Mutations & Node Swap Invariant**:
   - Implemented `upsert_symbol`: updates node in-place if `SymbolId` exists, or inserts new node and synchronizes bidirectional maps.
   - Implemented `remove_symbol`: intercepts Petgraph swap-removals and dynamically re-maps the swapped node's `NodeIndex` in `symbol_to_node`.
   - Implemented `invalidate_file`: atomically strips all nodes and incident edges belonging to a modified or deleted file in $O(k)$ time where $k$ is file symbol count.
4. **Bidirectional Topological Queries**:
   - `get_callers(&id)`: Fast incoming dependency inspection for blast-radius calculation.
   - `get_callees(&id)`: Fast outgoing dependency inspection.
   - `get_symbols_for_file(&path)`: Zero-copy file-level slice queries.

#### Results & Verification
- **Strict Linting & Format**: `cargo clippy --all-targets --all-features -- -D warnings` passed with 0 warnings.
- **Test Suite**: 4 unit tests passed in 0.00s covering:
  - Node insertion, lookup, and property updates.
  - Direct caller and callee edge linking.
  - Petgraph index compaction and swapped-node lookup stability.
  - Atomic single-file invalidation and edge cleanup.

---

### 2.3 Issue #3: Multi-Language Tree-sitter AST Extraction Layer

#### Why We Did This Next (Rationale & Pre-requisite Analysis)
- **Concrete Syntax Extraction without LLM Hallucination**: Loom guarantees deterministic call graphs. Before connecting the background file watcher, the system needed declarative Tree-sitter grammar parsers capable of converting raw source text into verified `SymbolNode` models and raw call dependencies.
- **Multi-Language Support**: The core design must handle polyglot repositories (Rust, TypeScript, TSX, Python) through unified interfaces.

#### What We Did (Technical Implementation)
1. **Crate Setup (`crates/loom-ast`)**:
   - Integrated `tree-sitter` (v0.24), `tree-sitter-rust`, `tree-sitter-typescript`, `tree-sitter-python`, and `streaming-iterator`.
2. **`Language` Abstraction (`crates/loom-ast/src/parser.rs`)**:
   - Automatic extension detection (`.rs`, `.ts`, `.tsx`, `.py`).
   - Declarative SCM queries for extracting functions, methods, structs, classes, interfaces, traits, enums, type aliases, and constants.
   - SCM queries for extracting call expressions and member invocations.
3. **`AstEngine` Implementation**:
   - Manages reusable parser instances.
   - Implemented zero-copy capture extraction and signature line slicing.
   - Extracted line numbers, byte boundaries, and deterministic `SymbolId::derive(...)`.
   - Structured output into `ParsedFile` containing `symbols: Vec<SymbolNode>` and `raw_calls: Vec<RawCallReference>`.
4. **Graceful Error Handling**:
   - Bubble up typed `AstError` without panic on syntax errors.

#### Results & Verification
- **Strict Linting & Format**: Passed `cargo clippy --all-targets --all-features -- -D warnings` with zero warnings.
- **Test Suite**: 3 integration tests passed in 0.05s verifying accurate symbol and call extraction for:
  - Rust structs, enums, functions, and function calls.
  - TypeScript interfaces, classes, methods, and member invocations.
  - Python classes, methods, and function calls.

---

### 2.4 Issue #4: Debounced File Watcher & Incremental AST Re-indexing Pipeline

#### Why We Did This Next (Rationale & Pre-requisite Analysis)
- **Completing the Phase 1 Real-Time Engine**: Connecting the debounced watcher (`notify-debouncer-mini`), parallel Rayon parser, and `CodeGraph` completes the end-to-end real-time daemon pipeline.
- **Enforcing the Sub-5ms Performance Invariant**: Large codebase re-indexing is unacceptable on every keystroke. This pipeline delivers granular dirty-file invalidation so only changed files are re-parsed and reconciled.

#### What We Did (Technical Implementation)
1. **Crate & Binary Setup (`crates/loom-daemon`)**:
   - Created `loom-daemon` providing the orchestration library and CLI executable.
2. **`IndexingPipeline` (`crates/loom-daemon/src/pipeline.rs`)**:
   - Thread-safe `Arc<RwLock<CodeGraph>>` coordination.
   - `index_file(&path)`: single-file incremental pipeline that reads, parses AST, invalidates stale file state, upserts new symbols, and re-links call edges.
   - `index_batch(&paths)`: parallel multi-core batch indexing using `rayon::par_iter()`.
   - `remove_file(&path)`: immediate invalidation on file deletion.
3. **`DaemonWatcher` (`crates/loom-daemon/src/watcher.rs`)**:
   - 50ms aggregation debounce window via `notify-debouncer-mini`.
   - Dispatches filesystem events to `IndexingPipeline`.
4. **CLI Entrypoint (`crates/loom-daemon/src/main.rs`)**:
   - Configurable workspace root with `clap`.
   - Tokio async runtime with CPU-bound indexing safely isolated on `spawn_blocking`.

#### Results & Verification
- **Performance Benchmark**: Incremental single-file re-parse and graph update completed in **$< 1\,\text{ms}$**, well beneath the $< 5\,\text{ms}$ threshold.
- **Strict Linting & Format**: `cargo clippy --all-targets --all-features -- -D warnings` passed cleanly.
- **Full Test Suite**: 18/18 tests passed across the entire workspace:
  - `loom-core`: 9 tests passed.
  - `loom-graph`: 4 tests passed.
  - `loom-ast`: 3 tests passed.
  - `loom-daemon`: 2 tests passed.

---

## 3. Technical Decisions & Swaps in Ideas

- **Delimited BLAKE3 Framing**: Applied null delimiters (`\0`) and namespace markers (`::`) to eliminate collision risks across variable-length symbol identifiers.
- **Petgraph Node Compaction Handling**: Intercepted internal swap-removals during node deletion to ensure `symbol_to_node` index tables remain permanently synchronized.
- **Separation of Read Query and Graph Mutation**: In `IndexingPipeline`, query matches are collected into immutable vectors prior to writing dependency edges, preventing Rust borrow checker conflicts.
- **Thread Pool Isolation**: Isolated CPU-bound Tree-sitter AST parsing inside Rayon threads and Tokio `spawn_blocking`, ensuring the async runtime remains unblocked for future JSON-RPC/MCP serving.

---

## 4. Challenges Faced & Resolutions

| Challenge | Impact | Resolution |
| :--- | :--- | :--- |
| **Git Push HTTPS Authentication** | Interactive credential prompt during git push. | Utilized authenticated GitHub MCP `push_files` API for deterministic remote commits. |
| **Petgraph Index Invalidation on Deletion** | Petgraph `remove_node` swaps the last node to the removed index, invalidating external hash maps. | Implemented custom index reconciliation in `remove_symbol` and `invalidate_file`. |
| **Tree-sitter 0.24 Streaming Iterator API** | `QueryMatches` requires streaming iterator semantics rather than standard `Iterator`. | Integrated `streaming-iterator` crate and `while let Some(m) = matches.next()` patterns. |
| **Borrow Conflicts During Graph Edge Linking** | Attempting to query symbols while holding a mutable lock on `CodeGraph`. | Split operation into a candidate ID collection phase followed by an atomic edge insertion loop. |

---

## 5. Changelog

### [2026-09-26] - Phase 1 (Core Engine) Complete: Issues #1, #2, #3, #4
- **Closed #1**: `feat(core): implement core symbol schemas and deterministic BLAKE3 hashing`.
- **Closed #2**: `feat(graph): implement CodeGraph with petgraph and bidirectional lookup maps`.
- **Closed #3**: `feat(ast): build multi-language Tree-sitter AST extraction layer`.
- **Closed #4**: `feat(engine): connect debounced file watcher to incremental AST re-indexing pipeline`.
- **Delivered**:
  - `crates/loom-core`: Deterministic BLAKE3 symbol hashing, symbol nodes, and dependency edge types.
  - `crates/loom-graph`: `CodeGraph` directed graph with bidirectional index tables and atomic file invalidation.
  - `crates/loom-ast`: Tree-sitter multi-language AST extraction (Rust, TypeScript, Python).
  - `crates/loom-daemon`: 50ms debounced file watcher, Rayon parallel batch parser, and sub-millisecond incremental update pipeline.
- **Verified**: 18 unit and integration tests passing with 0 warnings under strict `#![warn(clippy::pedantic)]`.

### [2026-09-25] - Repository Initialization & Phase 1 Planning
- **Added**: [IDEA.md](IDEA.md) architecture blueprint and roadmap.
- **Added**: [AGENTS.md](AGENTS.md) guidelines, performance constraints, and MCP protocol priority rules.
- **Created**: GitHub repository `309nahe/loom` on GitHub.
- **Created**: GitHub issues [#1](https://github.com/309nahe/loom/issues/1), [#2](https://github.com/309nahe/loom/issues/2), [#3](https://github.com/309nahe/loom/issues/3), and [#4](https://github.com/309nahe/loom/issues/4).
- **Added**: [DOCUMENTATION.md](DOCUMENTATION.md) tracking engineering decisions and changelog.
