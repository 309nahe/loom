AGENTS.md — Contributor & Autonomous Agent Directives

Project: Loom (Local Code-Topography Daemon & MCP Server)

Ecosystem: Rust 2024 Edition / 1.85+

Primary Goal: Sub-millisecond deterministic AST indexing, dependency graph traversals, and local MCP tool serving.

1. Tool Priority & GitHub MCP Protocol

When operating in this repository, you must prioritize using the GitHub Model Context Protocol (MCP) server over local fallback git commands or blind codebase edits whenever handling issues, PRs, cross-referencing upstream context, or repository metadata.

1.1 Decision Hierarchy for Tool Use

┌────────────────────────────────────────────────────────────────────────┐
│                   TASK INITIATION OR QUERY RECEIVED                    │
└───────────────────────────────────┬────────────────────────────────────┘
                                    │
                                    ▼
       Is the query related to issues, pull requests, commit history,
         upstream context, branch state, or repository metadata?
                                    │
                   ┌────────────────┴────────────────┐
                   ▼ YES                             ▼ NO
       ┌────────────────────────┐         ┌────────────────────────┐
       │ USE GITHUB MCP FIRST   │         │ USE LOCAL TOOLS        │
       │ (github_* MCP tools)   │         │ (File read/edit/bash)  │
       └────────────────────────┘         └────────────────────────┘


GitHub MCP Server (HIGHEST PRIORITY for GitHub-related actions):

Task Planning / Context Gathering: Before implementing a feature or fixing a bug, use github_get_issue or github_list_issues to parse exact specifications, acceptance criteria, and bug reports.

Pull Requests & Code Reviews: Check existing PRs and reviews using github_list_pull_requests or github_get_pull_request_comments to avoid duplicate work and understand ongoing architectural decisions.

File Tree & Commit History Verification: When confirming remote baseline state or commit provenance, prefer github_get_commit and GitHub search/tree tools over raw local shell calls where applicable.

Issue Linking: Always reference the issue number (Fixes #123) in generated commit messages and PR descriptions.

Loom Local MCP Server (for code self-inspection):

If running Loom on itself, invoke loom_get_blast_radius and loom_get_symbol_context before mutating core graph or AST schemas.

Local Shell / File System Tools (Fallback / Implementation only):

Use local tools exclusively for local file modifications, running cargo commands, and executing test suites.

2. Project Architecture & Invariants

Loom is fundamentally a high-throughput, zero-hallucination systems tool. Treat the following architectural rules as non-negotiable invariants:

2.1 Determinism Over Heuristics

Never use fuzzy matching or LLM inference for symbol linking. All edges in CodeGraph must be backed by a verified tree-sitter AST parse node or explicit language import resolution.

Deterministic Hashing: Symbol identifiers (SymbolId) must always use BLAKE3 truncated to 128 bits:


$$\text{SymbolID} = \text{BLAKE3}(\text{file\_path} \parallel \text{namespace} \parallel \text{name} \parallel \text{signature})$$

Never inject non-deterministic UUIDs or random keys into the graph schema.

2.2 Performance Constraints

Parse & Diff Latency: Incremental single-file re-parse and graph update must complete in $< 5\,\text{ms}$.

No Heavy Allocations in Inner Loops: In AST traversal and Tree-sitter query matching, prefer zero-copy slices (&str, &[u8]) over cloning strings until nodes are formally upserted into the graph.

Concurrency Model:

File parsing: CPU-bound, parallelized with rayon.

MCP server & I/O: IO-bound, handled by tokio (async).

Do not run blocking filesystem or heavy Rayon computations directly inside the tokio runtime threads without tokio::task::spawn_blocking.

2.3 Incremental State Invalidation

Modifying a file must only invalidate and update symbols within that file and re-link its direct inbound/outbound edges. Do not trigger a full workspace re-scan unless explicitly requested.

3. Rust Code Guidelines & Standards

Every code generation or modification must adhere to modern idiomatic Rust:

3.1 Edition & Idioms

Target Rust 2024 Edition (minimum supported Rust version: 1.85+).

Prefer let-else statements for early returns and unwrap guards.

Adhere to clippy::pedantic wherever practical. Avoid #[allow(...)] without documenting why in a code comment.

3.2 Error Handling

Libraries & Core Modules (loom-core, loom-graph, loom-ast):

Use thiserror to define explicit, typed error enums.

Never use .unwrap() or .expect() in production paths. Bubble up errors via Result<T, LoomError>.

Daemon Binary & CLI (loom-daemon, loom-cli):

Use anyhow for top-level application error reporting and context decoration (.context(...)).

3.3 Data Structures

All graph nodes and edges must derive serde::Serialize and serde::Deserialize for cache serialization (redb) and JSON-RPC dispatch.

Maintain bidirectional lookup maps inside CodeGraph:

pub struct CodeGraph {
    pub graph: DiGraph<SymbolNode, DependencyEdge>,
    pub symbol_to_node: HashMap<SymbolId, NodeIndex>,
    pub file_to_symbols: HashMap<PathBuf, Vec<SymbolId>>,
}


4. Operational Workflow for Autonomous Agents

Follow this exact step-by-step loop for every task:

┌──────────────────────────────────────────────────────────────┐
│ 1. CONTEXT: Query GitHub MCP for issue details & scope       │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│ 2. RECONNAISSANCE: Inspect related files & local tests       │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│ 3. IMPACT ANALYSIS: Verify blast radius before editing       │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│ 4. IMPLEMENTATION: Apply minimal, clean, idiomatic edits     │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│ 5. VERIFICATION: Run cargo fmt, clippy, and cargo test       │
└──────────────────────────────┬───────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│ 6. REPORTING: Update issue / draft PR summary via GitHub MCP │
└──────────────────────────────────────────────────────────────┘


4.1 Verification Commands

Before marking any task as complete, verify that the following local checks pass:

# 1. Format check
cargo fmt --check

# 2. Strict linting
cargo clippy --all-targets --all-features -- -D warnings

# 3. Unit and integration tests
cargo test --all-targets


5. Security & Boundary Rules

No External Network Exfiltration: Code parsed by Tree-sitter and embedded locally must never leave the local environment. Any outgoing network requests require explicit user authorization.

Path Sanitization: When handling file paths from MCP client inputs, always canonicalize and verify they remain within the target repository root to prevent path traversal attacks.

Graceful Degradation: If an unknown file type or invalid syntax tree is encountered, Loom must log a warning via tracing::warn! and skip the unparseable node without crashing the daemon.
