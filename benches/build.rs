use criterion::{criterion_group, criterion_main, Criterion};

use overlandr::graph::builder::{build_graph, BuildOpts};
use overlandr::osm::loader::{NodeMap, Way};
use overlandr::profile::load_profile;

// ---------------------------------------------------------------------------
// Helper: build a synthetic set of ways for benchmarking.
//
// Creates `n` two-node ways arranged in a star pattern:
//   - Hub node 0 at (47.0, -116.0)
//   - Spoke node k at (47.0 + k*0.001, -116.0) for k in 1..=n
//   - Each way: hub → spoke_k with highway=track
// ---------------------------------------------------------------------------
fn make_synthetic_ways(n: usize) -> (Vec<Way>, NodeMap) {
    let mut nodes: NodeMap = std::collections::HashMap::with_capacity(n + 1);
    nodes.insert(0, (47.0, -116.0));

    let mut ways: Vec<Way> = Vec::with_capacity(n);
    let mut tags = std::collections::HashMap::new();
    tags.insert("highway".to_string(), "track".to_string());

    for k in 1..=n {
        let spoke_id = k as i64;
        let lat = 47.0 + (k as f64) * 0.001;
        nodes.insert(spoke_id, (lat, -116.0));

        ways.push(Way {
            id: spoke_id,
            nodes: vec![0, spoke_id],
            tags: tags.clone(),
        });
    }

    (ways, nodes)
}

// ---------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------

fn bench_build_graph(c: &mut Criterion) {
    let profile = load_profile(None).expect("embedded profile must parse");
    let (ways, nodes) = make_synthetic_ways(500);
    let scenic_features = Vec::new();

    c.bench_function("build_graph_500_ways", |b| {
        b.iter(|| build_graph(&ways, &nodes, &scenic_features, &profile, &BuildOpts::default()))
    });
}

criterion_group!(benches, bench_build_graph);
criterion_main!(benches);
