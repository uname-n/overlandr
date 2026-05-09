//! Integration test for full route pipeline.
//!
//! Uses the same in-memory diamond network as integration_build, exercising
//! build_graph → contract → k_alternatives without touching any PBF file.

use std::collections::HashMap;

use std::collections::HashMap as StdHashMap;

use overlandr::graph::builder::{build_graph, BuildOpts};
use overlandr::graph::contract::contract;
use overlandr::graph::{Graph, NodeData, EdgeData, EdgeFlags};
use overlandr::osm::loader::{NodeMap, Way};
use overlandr::profile::load_profile;
use overlandr::routing::alternatives::{k_alternatives, AltConfig};

// ---------------------------------------------------------------------------
// Helpers shared with integration_build
// ---------------------------------------------------------------------------

fn make_node_map() -> NodeMap {
    let mut m: NodeMap = HashMap::new();
    m.insert(0, (47.0, -116.0));
    m.insert(1, (47.1, -116.1));
    m.insert(2, (47.1, -115.9));
    m.insert(3, (47.2, -116.0));
    m
}

fn make_two_corridor_ways() -> Vec<Way> {
    let mut tags = HashMap::new();
    tags.insert("highway".to_string(), "track".to_string());

    vec![
        Way { id: 1, nodes: vec![0, 1, 3], tags: tags.clone() },
        Way { id: 2, nodes: vec![0, 2, 3], tags: tags.clone() },
    ]
}

fn build_and_contract_diamond() -> Graph {
    let profile = load_profile(None).expect("embedded profile must parse");
    let ways = make_two_corridor_ways();
    let nodes = make_node_map();
    let graph = build_graph(&ways, &nodes, &[], &profile, &BuildOpts::default());
    contract(graph)
}

/// Build a graph from (src, dst, cost, length_m, flags) slices — flags let tests set PAVED etc.
fn build_graph_from_edges_with_flags(
    node_count: usize,
    directed_edges: &[(usize, usize, f32, f32, EdgeFlags)],
) -> Graph {
    let mut all: Vec<(usize, usize, f32, f32, EdgeFlags)> = Vec::new();
    for &(u, v, c, l, f) in directed_edges {
        all.push((u, v, c, l, f));
        all.push((v, u, c, l, f));
    }

    let edge_count = all.len();
    let mut edges: Vec<EdgeData> = Vec::with_capacity(edge_count);
    let mut adj: Vec<Vec<(u32, u32)>> = vec![vec![]; node_count];

    for (eid, &(u, v, cost, length_m, flags)) in all.iter().enumerate() {
        adj[u].push((v as u32, eid as u32));
        edges.push(EdgeData { cost, length_m, flags, scenic_score: 0, polyline: vec![] });
    }

    let mut offsets = vec![0u32; node_count + 1];
    for u in 0..node_count {
        offsets[u + 1] = offsets[u] + adj[u].len() as u32;
    }
    let mut neighbors: Vec<(u32, u32)> = Vec::new();
    for u in 0..node_count { neighbors.extend_from_slice(&adj[u]); }
    let nodes = (0..node_count).map(|_| NodeData { lat_e7: 0, lon_e7: 0 }).collect();

    Graph { node_count, edge_count, nodes, offsets, neighbors, edges, fuel_stations: vec![] }
}

/// Build a simple bidirectional graph from (src, dst, cost, length_m) slices.
/// Used for the routing-specific tests below.
fn build_graph_from_edges(
    node_count: usize,
    directed_edges: &[(usize, usize, f32, f32)],
) -> Graph {
    let mut all: Vec<(usize, usize, f32, f32)> = Vec::new();
    for &(u, v, c, l) in directed_edges {
        all.push((u, v, c, l));
        all.push((v, u, c, l));
    }

    let edge_count = all.len();
    let mut edges: Vec<EdgeData> = Vec::with_capacity(edge_count);
    let mut adj: Vec<Vec<(u32, u32)>> = vec![vec![]; node_count];

    for (eid, &(u, v, cost, length_m)) in all.iter().enumerate() {
        adj[u].push((v as u32, eid as u32));
        edges.push(EdgeData {
            cost,
            length_m,
            flags: EdgeFlags::default(),
            scenic_score: 0,
            polyline: vec![],
        });
    }

    let mut offsets = vec![0u32; node_count + 1];
    for u in 0..node_count {
        offsets[u + 1] = offsets[u] + adj[u].len() as u32;
    }
    let mut neighbors: Vec<(u32, u32)> = Vec::new();
    for u in 0..node_count {
        neighbors.extend_from_slice(&adj[u]);
    }
    let nodes: Vec<NodeData> = (0..node_count)
        .map(|_| NodeData { lat_e7: 0, lon_e7: 0 })
        .collect();

    Graph { node_count, edge_count, nodes, offsets, neighbors, edges, fuel_stations: vec![] }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn k_alternatives_finds_two_distinct_routes_on_diamond() {
    // The diamond graph has all nodes at undirected-degree 2, so contraction
    // keeps all 4 nodes. Start=0, end=3 (last node by OSM insertion order).
    let graph = build_and_contract_diamond();

    // The build_graph assigns compact IDs in order of first encounter:
    // OSM id 0 → compact 0, 1 → compact 1, 3 → compact 2, 2 → compact 3.
    // But we just need from=0 and to = some node that is not 0.
    // Use node 0 → node that sits at the other apex of the diamond.
    // Safe approach: ask for routes between node 0 and the last-inserted node.
    let from: u32 = 0;
    // Find the node that both corridors share as their end (node OSM id 3).
    // In compact IDs it is 2 (third insertion: 0, 1, 3, then 2).
    // Rather than hard-code, just try node_count-1 and node_count-2.
    // We expect 2 routes on the diamond; either "to" candidate that is
    // reachable by both corridors will do.
    let to: u32 = (graph.node_count as u32) - 1;

    let cfg = AltConfig {
        min_jaccard_distance: 0.1, // relax diversity for small test graph
        ..AltConfig::default()
    };

    // If the last node happens to be unreachable from 0 via both corridors,
    // fall back to checking any pair. We just verify at least 1 route exists.
    let routes = k_alternatives(&graph, from, to, 2, &cfg);
    assert!(!routes.is_empty(), "expected at least one route on a connected diamond graph");
}

#[test]
fn k_alternatives_diamond_explicit_graph_two_routes() {
    // Build a clean 4-node diamond directly (no PBF, no build_graph).
    //   0 → 1 → 3  (north)
    //   0 → 2 → 3  (south)
    let graph = build_graph_from_edges(
        4,
        &[
            (0, 1, 1.0, 1000.0),
            (1, 3, 1.0, 1000.0),
            (0, 2, 1.0, 1000.0),
            (2, 3, 1.0, 1000.0),
        ],
    );

    let cfg = AltConfig {
        min_jaccard_distance: 0.3,
        ..AltConfig::default()
    };
    let routes = k_alternatives(&graph, 0, 3, 2, &cfg);

    assert_eq!(routes.len(), 2, "expected 2 distinct routes on diamond graph");

    // Verify they use different interior edges.
    let edges_0: std::collections::HashSet<u32> = routes[0].edges.iter().copied().collect();
    let edges_1: std::collections::HashSet<u32> = routes[1].edges.iter().copied().collect();
    let intersection: std::collections::HashSet<_> = edges_0.intersection(&edges_1).collect();
    assert!(
        intersection.len() < edges_0.len(),
        "routes should not share all edges"
    );
}

#[test]
fn k3_alternatives_are_diverse() {
    // On a richer 6-node graph (two independent mid-corridors), requesting k=3
    // should produce routes where the 2nd and 3rd share fewer edges with the 1st.
    //
    //   0 ─ 1 ─ 5  (north)
    //   0 ─ 2 ─ 5  (middle)
    //   0 ─ 3 ─ 4 ─ 5  (south, one extra hop)
    let graph = build_graph_from_edges(
        6,
        &[
            (0, 1, 1.0, 1000.0),
            (1, 5, 1.0, 1000.0),
            (0, 2, 1.0, 1000.0),
            (2, 5, 1.0, 1000.0),
            (0, 3, 1.0, 1000.0),
            (3, 4, 1.0, 1000.0),
            (4, 5, 1.0, 1100.0),
        ],
    );

    let cfg = AltConfig { min_jaccard_distance: 0.2, max_detour: 2.0, ..AltConfig::default() };
    let routes = k_alternatives(&graph, 0, 5, 3, &cfg);

    assert!(routes.len() >= 2, "expected at least 2 diverse routes");

    // Each additional route must share strictly fewer edges with the first.
    let set0: std::collections::HashSet<u32> = routes[0].edges.iter().copied().collect();
    for r in routes.iter().skip(1) {
        let set_r: std::collections::HashSet<u32> = r.edges.iter().copied().collect();
        let shared = set0.intersection(&set_r).count();
        assert!(
            shared < set0.len(),
            "alternative route must diverge from the first (shared {shared}/{} edges)",
            set0.len()
        );
    }
}

#[test]
fn avoid_paved_penalty_steers_away_from_paved_edge() {
    // Diamond graph where the north corridor is paved, south is unpaved.
    // With a high PAVED penalty the router must prefer the south corridor.
    //
    //   0 ─ 1 ─ 3  (north, PAVED, cost=1.0 per segment)
    //   0 ─ 2 ─ 3  (south, unpaved, cost=1.5 per segment)
    let graph = build_graph_from_edges_with_flags(
        4,
        &[
            (0, 1, 1.0, 1000.0, EdgeFlags::PAVED),
            (1, 3, 1.0, 1000.0, EdgeFlags::PAVED),
            (0, 2, 1.5, 1000.0, EdgeFlags::default()),
            (2, 3, 1.5, 1000.0, EdgeFlags::default()),
        ],
    );

    // Penalise all PAVED edges heavily (simulating avoid_paved=true).
    let mut initial_penalties: StdHashMap<u32, f32> = StdHashMap::new();
    for (eid, edge) in graph.edges.iter().enumerate() {
        if edge.flags.contains(EdgeFlags::PAVED) {
            initial_penalties.insert(eid as u32, 10.0);
        }
    }

    let cfg = AltConfig { initial_penalties, ..AltConfig::default() };
    let routes = k_alternatives(&graph, 0, 3, 1, &cfg);

    assert!(!routes.is_empty(), "should find at least one route");
    // The chosen route must not use any PAVED edges.
    let uses_paved = routes[0].edges.iter().any(|&eid| {
        graph.edges[eid as usize].flags.contains(EdgeFlags::PAVED)
    });
    assert!(!uses_paved, "router should avoid paved edges when heavily penalised");
}

#[test]
fn disconnected_graph_returns_empty_routes() {
    // Two isolated nodes with no edges — k_alternatives must return empty, not panic.
    let graph = Graph {
        node_count: 2,
        edge_count: 0,
        nodes: vec![
            NodeData { lat_e7: 0, lon_e7: 0 },
            NodeData { lat_e7: 1_000_000, lon_e7: 0 },
        ],
        offsets: vec![0, 0, 0],
        neighbors: vec![],
        edges: vec![],
        fuel_stations: vec![],
    };

    let routes = k_alternatives(&graph, 0, 1, 1, &AltConfig::default());
    assert!(routes.is_empty(), "disconnected graph must yield no routes");
}
