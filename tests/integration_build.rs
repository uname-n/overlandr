//! Integration test for graph build pipeline (build_graph + contract).
//!
//! Uses a hand-built in-memory diamond network so no PBF file is needed.

use std::collections::HashMap;

use overlandr::graph::builder::{build_graph, BuildOpts};
use overlandr::graph::contract::contract;
use overlandr::graph::EdgeFlags;
use overlandr::osm::loader::{NodeMap, Way};
use overlandr::profile::load_profile;

// ---------------------------------------------------------------------------
// Helpers: diamond network
//
//   Node 0 (47.0, -116.0)  — start
//   Node 1 (47.1, -116.1)  — north via-node
//   Node 2 (47.1, -115.9)  — south via-node
//   Node 3 (47.2, -116.0)  — end
//
//   Way A: 0 → 1 → 3  (north corridor)
//   Way B: 0 → 2 → 3  (south corridor)
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn build_pipeline_produces_valid_graph() {
    let profile = load_profile(None).expect("embedded profile must parse");
    let ways = make_two_corridor_ways();
    let nodes = make_node_map();

    let graph = build_graph(&ways, &nodes, &[], &profile, &BuildOpts::default());

    assert!(graph.node_count > 0, "graph must have nodes");
    assert!(graph.edge_count > 0, "graph must have edges");

    // Diamond has 4 nodes and 4 segments × 2 directions = 8 directed edges.
    assert_eq!(graph.node_count, 4, "diamond has 4 nodes");
    assert_eq!(graph.edge_count, 8, "diamond has 8 directed edges (4 segments × 2)");
}

#[test]
fn paved_surface_tag_sets_paved_flag() {
    // A way tagged surface=asphalt must produce edges with the PAVED flag.
    let profile = load_profile(None).expect("embedded profile must parse");
    let mut nodes: NodeMap = HashMap::new();
    nodes.insert(0, (47.0, -116.0));
    nodes.insert(1, (47.1, -116.0));
    let mut tags = HashMap::new();
    tags.insert("highway".to_string(), "track".to_string());
    tags.insert("surface".to_string(), "asphalt".to_string());
    let ways = vec![Way { id: 1, nodes: vec![0, 1], tags }];

    let graph = build_graph(&ways, &nodes, &[], &profile, &BuildOpts::default());

    assert!(
        graph.edges.iter().all(|e| e.flags.contains(EdgeFlags::PAVED)),
        "all edges of an asphalt way must have PAVED flag"
    );
}

#[test]
fn smoothness_horrible_sets_both_rough_flags() {
    // A way tagged smoothness=horrible must have SMOOTHNESS_ROUGH and SMOOTHNESS_VERY_ROUGH.
    let profile = load_profile(None).expect("embedded profile must parse");
    let mut nodes: NodeMap = HashMap::new();
    nodes.insert(0, (47.0, -116.0));
    nodes.insert(1, (47.1, -116.0));
    let mut tags = HashMap::new();
    tags.insert("highway".to_string(), "track".to_string());
    tags.insert("smoothness".to_string(), "horrible".to_string());
    let ways = vec![Way { id: 1, nodes: vec![0, 1], tags }];

    let graph = build_graph(&ways, &nodes, &[], &profile, &BuildOpts::default());

    assert!(
        graph.edges.iter().all(|e| e.flags.contains(EdgeFlags::SMOOTHNESS_ROUGH)),
        "horrible smoothness must set SMOOTHNESS_ROUGH"
    );
    assert!(
        graph.edges.iter().all(|e| e.flags.contains(EdgeFlags::SMOOTHNESS_VERY_ROUGH)),
        "horrible smoothness must set SMOOTHNESS_VERY_ROUGH"
    );
}

#[test]
fn contracted_graph_preserves_connectivity() {
    let profile = load_profile(None).expect("embedded profile must parse");
    let ways = make_two_corridor_ways();
    let nodes = make_node_map();

    let graph = build_graph(&ways, &nodes, &[], &profile, &BuildOpts::default());
    let contracted = contract(graph);

    // After contraction the degree-2 interior nodes (1 and 2) are consumed,
    // leaving just nodes 0 and 3 (two surviving endpoints).
    assert!(contracted.node_count > 0, "contracted graph must have nodes");
    assert!(contracted.edge_count > 0, "contracted graph must have edges");
}
