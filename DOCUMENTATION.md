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

## 2. Technical Decisions & Swaps in Ideas

- **Tool Protocol Enforcement**: Adhered to the `AGENTS.md` directive prioritizing the GitHub MCP protocol (`github-mcp-server`) for repository creation, file commits, and issue generation over raw unauthenticated shell calls.
- **Modularity of Phase 1**: Separated the core symbol primitives (`SymbolId`, `SymbolNode`) from the graph structure (`CodeGraph`) and parser layer (`tree-sitter`) to enable isolated unit testing and clean crate/module layering.

---

## 3. Challenges Faced & Resolutions

| Challenge | Impact | Resolution |
| :--- | :--- | :--- |
| **Git Push HTTPS Authentication** | Standard `git push` over HTTPS prompted interactively for credentials. | Utilized GitHub MCP `push_files` to push commit directly and deterministically through the authenticated API. |
| **Sandbox Connection Reset** | Occasional connection reset when launching subprocesses in the default sandbox. | Executed local git initialization and verified repository status with standard tool retries while relying on GitHub MCP tools for remote state. |

---

## 4. Changelog

### [2026-09-25] - Repository Initialization & Phase 1 Planning
- **Added**: [IDEA.md](IDEA.md) architecture blueprint and roadmap.
- **Added**: [AGENTS.md](AGENTS.md) guidelines, performance constraints, and MCP protocol priority rules.
- **Created**: GitHub repository `309nahe/loom` on GitHub.
- **Created**: GitHub issues [#1](https://github.com/309nahe/loom/issues/1), [#2](https://github.com/309nahe/loom/issues/2), [#3](https://github.com/309nahe/loom/issues/3), and [#4](https://github.com/309nahe/loom/issues/4) covering Phase 1 milestones.
- **Added**: [DOCUMENTATION.md](DOCUMENTATION.md) tracking development decisions and changelog.
