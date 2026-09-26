# Loom — Usage Guide

**Loom** is a local, deterministic code-topography daemon. It watches a repository, parses
every supported source file with Tree-sitter, and maintains an in-memory directed graph of
the repository's structural dependencies. It exists to answer structural questions —
*"what calls this?"*, *"what breaks if I change it?"*, *"what is dead?"* — with verified
graph facts instead of an LLM's guess.

> **Project status: Phases 1 and 2 are complete. Phase 3 (MCP server) is not started.**
> The engine, graph, and analysis layers are implemented, tested, and fast. There is
> **no MCP server and no query command line yet** — the CLI indexes and watches, and
> nothing can query it from the outside. See [Status](#status) and
> [Known limitations](#known-limitations) before planning around this.

---

## Contents

- [The problem Loom solves](#the-problem-loom-solves)
- [Goals and design pillars](#goals-and-design-pillars)
- [Status](#status)
- [Architecture](#architecture)
- [Building](#building)
- [Usage](#usage)
  - [As a CLI](#as-a-cli-the-daemon)
  - [As a library](#as-a-library)
- [What gets extracted](#what-gets-extracted)
- [Measured performance](#measured-performance)
- [Configuration](#configuration)
- [Known limitations](#known-limitations)
- [Roadmap](#roadmap)
- [Development](#development)

---

## The problem Loom solves

AI coding agents produce code faster than any human can hold the whole system in their head.
When such an agent needs to know how a change propagates, its only options are bad:

| Approach | Failure mode |
| --- | --- |
| `grep` / shell search loops | Regex cannot resolve aliasing, shadowing, or call chains. Misses cross-file effects. 3–10 s per query. |
| Ask an LLM to "figure out the call graph" | Hallucinated edges, silently dropped dependencies, non-reproducible answers, token waste. |
| Read whole files into context | Floods the context window with irrelevant text, diluting reasoning and inflating cost. |

Loom replaces all three with a **precomputed, mathematically verified dependency graph** that
answers the same questions in well under a millisecond, returning only the exact subgraph
that matters.

Crucially, Loom is *deterministic by construction*: every edge in the graph is backed by a
real Tree-sitter parse node, and every symbol's identity is a content hash. The same
repository always produces the same graph, and a symbol never silently changes identity
across restarts.

---

## Goals and design pillars

### 1. Determinism over hallucination

Relationships are extracted by declarative Tree-sitter queries (`.scm`), never inferred.
Symbol identity is a truncated 128-bit BLAKE3 hash:

```
SymbolId = BLAKE3(file_path ‖ namespace_hierarchy ‖ symbol_name ‖ signature)
```

No UUIDs, no memory addresses, no auto-increment counters. The same source always yields the
same ID, which is what makes incremental re-indexing, caching, and diffing possible at all.
Components are framed with `\0` and `::` delimiters so that `("ab","c")` and `("a","bc")`
cannot collide.

### 2. Sub-millisecond incremental invalidation

Editing one file must not rescan the repository. Loom tracks dirty files, re-parses only
those, and reconciles just their symbols and incident edges. Editing `auth.rs` leaves the
other 44 files' topology completely untouched.

### 3. Blast-radius analysis

When something is about to be changed, Loom computes the full upstream impact: direct
callers, transitive callers with their topological depth, and the specific test functions
that cover the affected paths — plus a reproducible risk score.

### 4. Local-first, no data exfiltration

Parsed code stays on the machine. There is no telemetry, no remote index, and no network
listener.

---

## Status

| Area | State | Notes |
| --- | --- | --- |
| Deterministic core schemas + BLAKE3 IDs | **Done** | `loom-core` |
| `CodeGraph` with petgraph + bidirectional indexes | **Done** | `loom-graph` |
| Multi-language Tree-sitter extraction | **Done** | `loom-ast` (Rust, TypeScript, TSX, Python) |
| Debounced watcher + incremental re-indexing | **Done** (with a bug, [see below](#known-limitations)) | `loom-daemon` |
| Transitive traversal + shortest path | **Done** | `loom-graph` |
| Blast radius + risk scoring | **Done** | `loom-analysis` |
| Dead code detection | **Done** | `loom-analysis` |
| **MCP server (stdio JSON-RPC)** | **Not started** | [Issue #13](https://github.com/309nahe/loom/issues/13) |
| **`loom_get_blast_radius` / `loom_get_symbol_context`** | **Not started** | [Issue #14](https://github.com/309nahe/loom/issues/14) |
| **`loom_find_dead_symbols`** | **Not started** | [Issue #15](https://github.com/309nahe/loom/issues/15) |
| **Client integration + config templates** | **Not started** | [Issue #16](https://github.com/309nahe/loom/issues/16) |
| Persistence (`redb`) | Not started | Phase 4 |
| Semantic / vector search (`fastembed`) | Not started | Phase 4, optional |

**Consequence:** today Loom is usable as a **library** and as an **indexing daemon**. The
"ask an agent" workflow described throughout the design blueprint is Phase 3 and is not
available yet.

---

## Architecture

```
   [ Working Tree ]
         │  file saved
         ▼
  ┌──────────────────────────────────────┐
  │ DaemonWatcher   (notify-debouncer)  │  50 ms aggregation window
  └───────────────────┬──────────────────┘
                      ▼
  ┌──────────────────────────────────────┐
  │ IndexingPipeline                    │
  │   rayon parse (thread-local engines) │  CPU-bound, no locks held
  │   invalidate file → upsert → link    │  write lock held only here
  └───────────────────┬──────────────────┘
                      ▼
  ┌──────────────────────────────────────┐
  │ CodeGraph  (Arc<RwLock<…>>)          │  read lock for all queries
  │   DiGraph<SymbolNode, DependencyEdge> │
  │   symbol_to_node, file_to_symbols    │
  └────────┬──────────────────────┬──────┘
           ▼                      ▼
   ┌───────────────┐      ┌──────────────────┐
   │ BlastRadius   │      │ DeadCodeDetector │  loom-analysis
   │ Calculator    │      │                  │
   └───────────────┘      └──────────────────┘
```

### Crates

| Crate | Responsibility | Key types |
| --- | --- | --- |
| `loom-core` | Domain schemas and deterministic hashing. No internal deps. | `SymbolId`, `SymbolNode`, `SymbolKind`, `DependencyEdge`, `EdgeKind`, `LoomError` |
| `loom-graph` | Directed graph storage, bidirectional indexes, traversals. | `CodeGraph` |
| `loom-ast` | Tree-sitter parsing and symbol/call extraction. | `AstEngine`, `Language`, `ParsedFile`, `RawCallReference` |
| `loom-analysis` | Impact analysis over a read-only graph. | `BlastRadiusCalculator`, `DeadCodeDetector` |
| `loom-daemon` | Runtime, file watching, indexing orchestration, CLI binary. | `IndexingPipeline`, `DaemonWatcher` |

Dependencies flow strictly downward: `daemon → analysis → graph → ast → core`. The graph
and AST layers know nothing about risk analysis, and `loom-analysis` never mutates topology,
so it can run against a read-locked graph while the watcher keeps indexing.

---

## Building

Requires **Rust 1.85+** (edition 2024).

```bash
git clone https://github.com/309nahe/loom.git
cd loom
cargo build --release
```

The binary lands at `target/release/loom-daemon`.

---

## Usage

### As a CLI (the daemon)

The CLI has exactly one job: index a workspace and keep the graph current.

```bash
# Index the current directory
cargo run -p loom-daemon

# Index a specific workspace
cargo run -p loom-daemon -- --workspace /path/to/repo
# short form
cargo run -p loom-daemon -- -w /path/to/repo
```

Real output from indexing this repository (45 files):

```text
INFO loom_daemon: Starting Loom Daemon for workspace: "/home/nana/dev/loom"
INFO loom_daemon: Found 45 candidate source files to index
INFO loom_daemon::pipeline: Batch indexing of 45 files completed in 496.694864ms
INFO loom_daemon: Initial indexing complete: 210 symbols, 2781 dependency edges
INFO loom_daemon::watcher: Daemon file watcher initialized on "/home/nana/dev/loom"
```

The process then **runs in the foreground indefinitely**, watching for changes. Press
`Ctrl-C` to stop. There is no query subcommand — see [Status](#status).

To watch per-file incremental re-index latency:

```bash
RUST_LOG=loom_daemon=debug cargo run -p loom-daemon -- -w /path/to/repo
```

```text
DEBUG loom_daemon::pipeline: Incremental re-index for "/path/to/repo/src/auth.rs" completed in 215.615µs
DEBUG loom_daemon::pipeline: Invalidated 1 symbols for removed file "/path/to/repo/src/old.rs"
```

### As a library

The supported way to consume Loom today. Add the crates you need:

```toml
[dependencies]
loom-core    = { path = "crates/loom-core" }
loom-graph   = { path = "crates/loom-graph" }
loom-ast     = { path = "crates/loom-ast" }
loom-analysis = { path = "crates/loom-analysis" }
loom-daemon  = { path = "crates/loom-daemon" }
```

Index a workspace, then query the graph:

```rust
use std::sync::{Arc, RwLock};

use loom_analysis::blast_radius::BlastRadiusCalculator;
use loom_daemon::IndexingPipeline;
use loom_graph::CodeGraph;

let graph = Arc::new(RwLock::new(CodeGraph::new()));
let pipeline = IndexingPipeline::new(Arc::clone(&graph));

// 1. Index (parallel; also available as `index_file` for a single file)
let elapsed = pipeline.index_batch(&["demo/src/auth.rs".into(), "demo/src/billing.rs".into()]);
println!("indexed in {elapsed:?}");

let read = graph.read().expect("read lock");

// 2. Resolve a symbol by name.
let candidates = read.get_symbols_by_name("validate");
let target = candidates.first().expect("validate must exist");

// 3. What breaks if I change it?
let report = BlastRadiusCalculator::new(&read)
    .calculate(&target.id, 5)
    .expect("report");

println!("{:?} score={:.2}", report.risk_level, report.risk_score_raw);
for caller in &report.direct_callers {
    println!("  caller: {} (line {})", caller.name, caller.call_site_line);
}

// 4. Traversal primitives.
for (node, edge) in read.get_callers(&target.id) {
    println!("{} calls it at line {}", node.name, edge.call_site_line);
}
for (node, depth) in read.find_transitive_callers(&target.id, 3) {
    println!("depth {depth}: {}", node.name);
}

// 5. Dead code.
let dead = loom_analysis::dead_code::DeadCodeDetector::new(&read).find_dead_symbols();
println!("{} dead symbols", dead.total_dead_count);
```

Actual output for a two-file demo (`auth.rs` defines `validate`, called by `login` and
`charge`):

```text
indexed in 243.963843ms
5 symbols, 2 edges, 2 files

blast radius for `validate`
  risk           : Critical
  raw score      : 24.00
  affected       : 2
  caller         : charge (line 2)
  caller         : login (line 8)
  rationale      : CRITICAL: Modifying exported interface impacting 2 total symbols across public boundary

dead symbols: 1
  "never_called" (UnreferencedInternalSymbol)
```

#### Useful APIs

| Goal | Call |
| --- | --- |
| Index one file | `IndexingPipeline::index_file(&path)` |
| Index many files in parallel | `IndexingPipeline::index_batch(&paths)` |
| Drop a deleted file | `IndexingPipeline::remove_file(&path)` |
| Look up by ID | `graph.get_symbol(&id)` |
| Look up by name (linear scan) | `graph.get_symbols_by_name("name")` |
| Symbols defined in a file | `graph.get_symbols_for_file(&path)` |
| Direct callers / callees | `graph.get_callers(&id)` / `get_callees(&id)` |
| Upstream / downstream closure | `find_transitive_callers(&id, depth)` / `find_transitive_callees(&id, depth)` |
| Shortest dependency path | `graph.find_shortest_path(&from, &to)` |
| Impact + risk score | `BlastRadiusCalculator::new(&graph).calculate(&id, depth)` |
| Dead code | `DeadCodeDetector::new(&graph).find_dead_symbols()` |
| Deterministic ID | `SymbolId::derive(path, &[ns], name, signature)` |

#### `SymbolId` is not derivable from a name alone

`SymbolId::derive` hashes the **signature** as well as the path and name, and the signature
is not known when you only have a name. To resolve a symbol, go through the graph's reverse
index (`get_symbols_by_name` / `get_symbols_for_file`) rather than trying to recompute the
hash. This is the same constraint that Issue #14 has to solve for the MCP tools.

---

## What gets extracted

| Language | Extensions | Definitions | Call syntax |
| --- | --- | --- | --- |
| Rust | `.rs` | `function_item`, `struct_item`, `enum_item`, `trait_item`, `type_item`, `mod_item`, `const_item` | `identifier`, `field_expression`, `scoped_identifier` |
| TypeScript | `.ts` | `function_declaration`, `method_definition`, `class_declaration`, `interface_declaration`, `type_alias_declaration`, `enum_declaration` | `identifier`, `member_expression` |
| TSX | `.tsx` | same as TypeScript | same as TypeScript |
| Python | `.py` | `function_definition`, `class_definition` | `identifier`, `attribute` |

Per symbol, Loom records: name, kind, file path, byte range, line range, signature,
`is_exported`, and an epoch counter. Per call, the callee name, line, and byte range.

Call sites are attributed to the **innermost enclosing callable**, so a call inside a method
nested in a class is attributed to the method, not the class.

`is_exported` is derived per language: `pub` (Rust), `export` (TypeScript/TSX), and
"does not start with `_`" (Python). This directly determines whether dead-code analysis
considers a symbol live.

---

## Measured performance

Verified on this machine against this repository (rustc 1.98.1, debug build unless noted):

| Operation | Measured |
| --- | --- |
| Cold batch index, 45 files | ~497 ms |
| Cold batch index, 1–2 files | ~230–255 ms (dominated by per-thread query compilation) |
| **Incremental single-file re-index** | **~200–480 µs** (budget: < 5 ms) |
| Transitive traversal, 5 levels, 10k nodes | < 2 ms asserted in tests |
| Transitive traversal, 5 levels, 50k nodes | < 2 ms asserted in tests |

The cold-start figure is dominated by one-off Tree-sitter query compilation (~10–15 ms per
language per thread). Queries are compiled once and reused, which is what brings steady-state
parsing below 0.15 ms per file. The incremental path is enforced by an assertion in
`test_single_file_incremental_indexing_latency`.

---

## Configuration

| Setting | How | Effect |
| --- | --- | --- |
| Workspace root | `--workspace` / `-w` (default `.`) | Directory to index and watch. Canonicalized at startup. |
| Log level | `RUST_LOG`, e.g. `RUST_LOG=loom_daemon=debug` | Defaults to `INFO`. `debug` adds per-file re-index timings. |

**Directories skipped during the initial scan:** anything starting with `.` (so `.git`),
plus `target` and `node_modules`. The watcher does **not** apply these exclusions — it
observes the whole tree recursively and lets the pipeline skip files it cannot parse.

Note that `tracing` writes to **stdout** today. This is harmless for the CLI but must change
before an MCP server shares stdout as its protocol channel (tracked in Issue #13).

---

## Known limitations

### The file watcher re-indexes in a loop

**This is a real, reproducible bug.** A single file creation or write causes the daemon to
re-index that file roughly **20 times per second, indefinitely**, even after all activity
stops. Idle time is clean — the loop only starts after a content event.

```bash
mkdir -p /tmp/loop/src && echo 'pub fn a() {}' > /tmp/loop/src/a.rs
RUST_LOG=loom_daemon=debug ./target/release/loom-daemon -w /tmp/loop > /tmp/loop.log 2>&1 &
sleep 3
grep -c 'Incremental re-index' /tmp/loop.log   # 0 — idle is clean
echo 'pub fn b() {}' > /tmp/loop/src/b.rs     # one single write
sleep 10
grep -c 'Incremental re-index' /tmp/loop.log   # ~198 — should be ~1
kill %1
```

Individually each re-index is fast (~250 µs), so the graph stays correct; the cost is
sustained CPU burn and a flooded log. The likely cause is the dependency set:
`notify-debouncer-mini` 0.5 pulls in `notify` 7.0.0 while the workspace also resolves
`notify` 8.2.0, and the direct `notify` dependency is not actually used by any source file.
Until this is fixed, treat the daemon as a **one-shot indexer** (`index_batch` output is
correct and fast) rather than a long-running watcher.

### Call edges are resolved by name, repo-wide

Call resolution matches `callee_name` against **every** symbol with that name in the whole
graph, with no module or import scoping. Consequences:

- **False edges.** `charge()` in one module links to an unrelated `charge()` elsewhere.
- **Edge fan-out.** This repository indexes to 2781 edges for 210 symbols (~13 per symbol),
  which is far more than real call structure — a direct consequence of this behaviour.
- **No overload handling.** Two same-named symbols in one file are indistinguishable.

This is the largest correctness gap in the engine and the main thing to fix before trusting
blast-radius numbers on a real codebase.

### Other gaps

- **No docstrings.** `SymbolNode::docstring` is always `None`; extraction is not implemented.
- **`is_conditional` is always `false`.** The schema field exists but no parent-node
  inspection populates it.
- **Rust methods are not distinguished at parse time.** Rust has no `method_definition` node,
  so methods arrive as `function_item` and are classified as `Function` rather than `Method`.
- **Dead code treats `pub`/exported symbols as live roots.** A `pub fn` that nothing calls is
  reported as *not dead*, because external consumers cannot be seen. This is deliberate (it
  prevents deleting public API), but it means dead-code reports under-report in library
  crates.
- **No persistence.** Restarting re-indexes from scratch; the `redb` cache is Phase 4.
- **No MCP server, no query CLI.** See [Status](#status).
- **Duplicate `notify` versions** in the dependency tree (7.0.0 and 8.2.0).

---

## Roadmap

**Phase 3 — MCP server & agent integration** (in progress, issues open):

- [#13](https://github.com/309nahe/loom/issues/13) — JSON-RPC 2.0 stdio server loop
- [#14](https://github.com/309nahe/loom/issues/14) — `loom_get_blast_radius`, `loom_get_symbol_context`
- [#15](https://github.com/309nahe/loom/issues/15) — `loom_find_dead_symbols`
- [#16](https://github.com/309nahe/loom/issues/16) — Cursor / Claude Code integration + E2E tests

**Phase 4 — persistence & hybrid search:** `redb` resume, optional `fastembed` semantic
layer, `ratatui` terminal visualizer.

Engineering decisions, trade-offs, and the full changelog live in
[DOCUMENTATION.md](DOCUMENTATION.md). The original design blueprint is
[IDEA.md](IDEA.md); contributor rules are in [AGENTS.md](AGENTS.md).

---

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

All three must pass before a change is considered complete. The workspace compiles under
`#![warn(clippy::pedantic)]` and `#![warn(missing_docs)]`.

```bash
# Benchmarks
cargo bench -p loom-graph
cargo bench -p loom-analysis

# Run one test suite
cargo test -p loom-graph --test reachability_and_pathfinding_tests
```

### Layout

```
crates/
  loom-core/      domain schemas, deterministic BLAKE3 IDs
  loom-graph/     CodeGraph, traversals, pathfinding
  loom-ast/       Tree-sitter engine, SCM queries
  loom-analysis/  blast radius, risk scoring, dead code
  loom-daemon/    watcher, indexing pipeline, CLI binary
```

### Testing approach

Unit tests live beside the code they cover; integration tests live in each crate's `tests/`
directory, and each file's module header names the invariant it guards. Benchmarks use
Criterion over synthetic 1k/10k/50k node graphs.
