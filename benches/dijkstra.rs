use criterion::{criterion_group, criterion_main, Criterion};

use overlandr::graph::{EdgeData, EdgeFlags, Graph, NodeData};
use overlandr::routing::dijkstra::dijkstra;

// ---------------------------------------------------------------------------
// Helper: build a bidirectional linear graph with `n` nodes.
//
//   0 ↔ 1 ↔ 2 ↔ … ↔ (n-1)
//
// Each edge has cost=1.0 and length_m=100.0.
// ---------------------------------------------------------------------------
fn build_linear_graph(n: usize) -> Graph {
    // Directed edges: forward and backward for each consecutive pair.
    let mut raw: Vec<(usize, usize)> = Vec::with_capacity((n - 1) * 2);
    for i in 0..n - 1 {
        raw.push((i, i + 1));
        raw.push((i + 1, i));
    }

    let edge_count = raw.len();
    let mut edges: Vec<EdgeData> = Vec::with_capacity(edge_count);
    let mut adj: Vec<Vec<(u32, u32)>> = vec![vec![]; n];

    for (eid, &(u, v)) in raw.iter().enumerate() {
        adj[u].push((v as u32, eid as u32));
        edges.push(EdgeData {
            cost: 1.0,
            length_m: 100.0,
            flags: EdgeFlags::default(),
            scenic_score: 0,
            polyline: vec![],
        });
    }

    let mut offsets = vec![0u32; n + 1];
    for u in 0..n {
        offsets[u + 1] = offsets[u] + adj[u].len() as u32;
    }
    let mut neighbors: Vec<(u32, u32)> = Vec::new();
    for u in 0..n {
        neighbors.extend_from_slice(&adj[u]);
    }

    let nodes: Vec<NodeData> = (0..n)
        .map(|i| NodeData {
            lat_e7: (i as i32) * 10_000,
            lon_e7: 0,
        })
        .collect();

    Graph { node_count: n, edge_count, nodes, offsets, neighbors, edges, fuel_stations: vec![] }
}

// ---------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------

fn bench_dijkstra(c: &mut Criterion) {
    let graph = build_linear_graph(1000);
    let penalties = std::collections::HashMap::new();

    c.bench_function("dijkstra_linear_1000", |b| {
        b.iter(|| dijkstra(&graph, 0, 999, &penalties))
    });
}

criterion_group!(benches, bench_dijkstra);
criterion_main!(benches);
