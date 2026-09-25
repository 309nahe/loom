//! Criterion benchmarks for the `loom-analysis` engines.
//!
//! Measures `BlastRadiusCalculator` and `DeadCodeDetector` over 1k and 10k node synthetic
//! graphs. Both engines are read-only passes over the graph, so these numbers also serve as a
//! proxy for "cost of one MCP analysis request" and must stay negligible next to a keystroke.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use loom_analysis::blast_radius::BlastRadiusCalculator;
use loom_analysis::dead_code::DeadCodeDetector;
use loom_core::edge::{DependencyEdge, EdgeKind};
use loom_core::id::SymbolId;
use loom_core::symbol::{SymbolKind, SymbolNode};
use loom_graph::CodeGraph;

fn generate_synthetic_graph(node_count: usize) -> (CodeGraph, Vec<SymbolId>) {
    let mut graph = CodeGraph::new();
    let mut symbol_ids = Vec::with_capacity(node_count);

    for i in 0..node_count {
        let file_path = if i % 20 == 0 {
            format!("tests/module_{}.rs", i / 50)
        } else {
            format!("src/module_{}.rs", i / 50)
        };
        let name = if i % 20 == 0 {
            format!("test_fn_{i}")
        } else {
            format!("fn_{i}")
        };
        let id = SymbolId::derive(&file_path, &["bench"], &name, "fn()");
        let node = SymbolNode::new(
            id,
            &name,
            SymbolKind::Function,
            &file_path,
            (0, 50),
            (1, 10),
            None,
            "fn()",
            i % 5 == 0,
            1,
        );
        graph.upsert_symbol(node);
        symbol_ids.push(id);
    }

    for (i, &from_id) in symbol_ids.iter().enumerate() {
        for b in 1..=3 {
            let target_idx = i * 2 + b;
            if target_idx < node_count {
                let _ = graph.add_edge(
                    from_id,
                    symbol_ids[target_idx],
                    DependencyEdge::new(EdgeKind::Calls, 5, false),
                );
            }
        }
    }

    (graph, symbol_ids)
}

fn bench_analysis(c: &mut Criterion) {
    let mut group = c.benchmark_group("analysis_engine");
    group.measurement_time(Duration::from_secs(5));

    for &size in &[1_000, 10_000] {
        let (graph, symbol_ids) = generate_synthetic_graph(size);
        let target_sym = symbol_ids[size / 2];

        group.bench_with_input(
            BenchmarkId::new("blast_radius_depth_5", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let calculator = BlastRadiusCalculator::new(black_box(&graph));
                    let report = calculator.calculate(black_box(&target_sym), black_box(5));
                    black_box(report);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("dead_code_detection", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let detector = DeadCodeDetector::new(black_box(&graph));
                    let report = detector.find_dead_symbols();
                    black_box(report);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_analysis);
criterion_main!(benches);
