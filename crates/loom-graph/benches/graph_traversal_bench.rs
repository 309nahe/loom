//! Criterion benchmarks for `CodeGraph` traversal performance at scale.
//!
//! Builds synthetic branching DAGs of 1k / 10k / 50k nodes and measures the three queries the
//! MCP tools expose (5-level transitive callers, transitive callees, shortest path). These
//! numbers are the evidence behind the < 2 ms interactive latency budget; a regression here
//! means the analysis layer can no longer answer in real time on large repositories.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use loom_graph::CodeGraph;

fn generate_synthetic_graph(
    node_count: usize,
    branching_factor: usize,
) -> (CodeGraph, Vec<SymbolId>) {
    let mut graph = CodeGraph::new();
    let mut symbol_ids = Vec::with_capacity(node_count);

    for i in 0..node_count {
        let file_path = format!("src/module_{}.rs", i / 100);
        let name = format!("fn_{i}");
        let id = SymbolId::derive(&file_path, &["synthetic"], &name, "fn()");
        let node = SymbolNode::new(
            id,
            &name,
            SymbolKind::Function,
            &file_path,
            (0, 50),
            (1, 10),
            None,
            "fn()",
            i % 10 == 0,
            1,
        );
        graph.upsert_symbol(node);
        symbol_ids.push(id);
    }

    // Connect nodes in a synthetic multi-level DAG with branching
    for (i, &from_id) in symbol_ids.iter().enumerate() {
        for b in 1..=branching_factor {
            let target_idx = i * branching_factor + b;
            if target_idx < node_count {
                let to_id = symbol_ids[target_idx];
                let edge = DependencyEdge::new(EdgeKind::Calls, 5, false);
                let _ = graph.add_edge(from_id, to_id, edge);
            }
        }
    }

    (graph, symbol_ids)
}

fn bench_transitive_traversals(c: &mut Criterion) {
    let mut group = c.benchmark_group("transitive_reachability");
    group.measurement_time(Duration::from_secs(5));

    for &size in &[1_000, 10_000, 50_000] {
        let (graph, symbol_ids) = generate_synthetic_graph(size, 3);
        let root_id = symbol_ids[0];
        let deep_id = symbol_ids[size.min(100)];

        group.bench_with_input(
            BenchmarkId::new("transitive_callees_depth_5", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let results = graph.find_transitive_callees(black_box(&root_id), black_box(5));
                    black_box(results);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("transitive_callers_depth_5", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let results = graph.find_transitive_callers(black_box(&deep_id), black_box(5));
                    black_box(results);
                });
            },
        );

        group.bench_with_input(BenchmarkId::new("shortest_path", size), &size, |b, _| {
            let target_id = symbol_ids[size.min(250)];
            b.iter(|| {
                let path = graph.find_shortest_path(black_box(&root_id), black_box(&target_id));
                black_box(path);
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_transitive_traversals);
criterion_main!(benches);
