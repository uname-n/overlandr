//! Degree-2 graph contraction with polyline preservation.
//!
//! Long chains of intermediate (degree-2) nodes — common on forest-service
//! roads encoded in OSM — are collapsed into single contracted edges.  The
//! intermediate node coordinates are stored in `EdgeData::polyline` so that
//! GPX output retains full geometric fidelity.

use std::collections::{HashMap, HashSet};

use super::{EdgeData, EdgeFlags, Graph, NodeData, NodeId};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Contract all degree-2 chains in `g` and return the simplified graph.
///
/// A *degree-2 node* is one whose combined set of distinct neighbours
/// (considering both forward and reverse edges) contains exactly 2 members
/// and which is not an endpoint of any chain (i.e. it is purely internal).
///
/// For each maximal chain `[A, d2₁, d2₂, …, B]` the function emits a single
/// contracted edge `A→B` (and `B→A` when the chain is bidirectional) whose:
/// - `cost`     = sum of individual edge costs,
/// - `length_m` = sum of individual lengths,
/// - `flags`    = bitwise OR of all flags,
/// - `polyline` = lat_e7/lon_e7 coordinates of every intermediate node.
///
/// Degree-2 nodes are then dropped and the CSR is rebuilt.
pub fn contract(g: Graph) -> Graph {
    let n = g.node_count;
    let fuel_stations = g.fuel_stations.clone();
    tracing::info!(before = n, "contracting graph");

    // ------------------------------------------------------------------
    // 1. Build undirected neighbour sets from the CSR adjacency list.
    //    We need to know, for each node, all *distinct* neighbours
    //    regardless of edge direction.
    // ------------------------------------------------------------------
    // undirected_neighbors[u] = set of nodes that share at least one
    // directed edge with u (in either direction).
    let mut undirected: Vec<HashSet<NodeId>> = vec![HashSet::new(); n];

    for u in 0..n as NodeId {
        for &(v, _eid) in g.neighbors(u) {
            undirected[u as usize].insert(v);
            undirected[v as usize].insert(u);
        }
    }

    // ------------------------------------------------------------------
    // 2. Classify nodes.
    //    A node is degree-2 iff its undirected degree is exactly 2.
    //    Nodes with degree != 2 are *endpoints* (chain anchors).
    // ------------------------------------------------------------------
    let is_d2: Vec<bool> = (0..n).map(|u| undirected[u].len() == 2).collect();

    // ------------------------------------------------------------------
    // 3. Build a directed edge lookup so we can cheaply retrieve an
    //    EdgeData for (src, dst).  There may be multiple parallel edges;
    //    we keep the first one found (contraction only collapses simple
    //    chains, which have unique edges between consecutive pairs).
    // ------------------------------------------------------------------
    // edge_map[(src,dst)] = EdgeData (cloned)
    let mut edge_map: HashMap<(NodeId, NodeId), EdgeData> = HashMap::new();
    for u in 0..n as NodeId {
        for &(v, eid) in g.neighbors(u) {
            edge_map.entry((u, v)).or_insert_with(|| g.edges[eid as usize].clone());
        }
    }

    // ------------------------------------------------------------------
    // 4. Find and contract all maximal degree-2 chains.
    //    We iterate over all *endpoint* nodes and walk their degree-2
    //    neighbours to discover chains.
    // ------------------------------------------------------------------
    let mut contracted_edges: Vec<(NodeId, NodeId, EdgeData)> = Vec::new();
    // Track which (src,dst) pairs we've already contracted so we don't
    // emit duplicates.
    let mut seen_chains: HashSet<(NodeId, NodeId)> = HashSet::new();
    // Track degree-2 nodes that are actually consumed by a valid chain.
    // Degree-2 nodes in pure cycles (no non-d2 endpoint) must be kept.
    let mut consumed_d2: HashSet<NodeId> = HashSet::new();

    for start in 0..n as NodeId {
        if is_d2[start as usize] {
            continue; // only start chains at endpoints
        }
        for &next in &undirected[start as usize] {
            if !is_d2[next as usize] {
                continue; // neighbour is also an endpoint — no chain here
            }
            // Walk the chain: start → next → … → end_node
            let mut chain: Vec<NodeId> = vec![start, next];
            let mut prev = start;
            let mut cur = next;
            loop {
                // cur is degree-2; its two neighbours are prev and one other.
                let next_in_chain = undirected[cur as usize]
                    .iter()
                    .copied()
                    .find(|&nb| nb != prev)
                    .expect("degree-2 node must have exactly 2 neighbours");
                chain.push(next_in_chain);
                if !is_d2[next_in_chain as usize] {
                    break; // reached the other endpoint
                }
                prev = cur;
                cur = next_in_chain;
            }

            let endpoint_a = *chain.first().unwrap();
            let endpoint_b = *chain.last().unwrap();

            // Normalise order so we don't emit A→B and B→A separately
            // for the same chain (we handle directionality below).
            let key = if endpoint_a <= endpoint_b {
                (endpoint_a, endpoint_b)
            } else {
                (endpoint_b, endpoint_a)
            };
            if seen_chains.contains(&key) {
                continue;
            }
            seen_chains.insert(key);

            // Mark all interior (degree-2) nodes as consumed.
            for &mid in &chain[1..chain.len() - 1] {
                consumed_d2.insert(mid);
            }

            // Check whether the chain is bidirectional.
            // A chain is bidirectional iff every consecutive pair has edges
            // in both directions.
            let pairs_forward: Vec<(NodeId, NodeId)> =
                chain.windows(2).map(|w| (w[0], w[1])).collect();
            let pairs_reverse: Vec<(NodeId, NodeId)> =
                chain.windows(2).map(|w| (w[1], w[0])).collect();

            let has_forward = pairs_forward.iter().all(|p| edge_map.contains_key(p));
            let has_reverse = pairs_reverse.iter().all(|p| edge_map.contains_key(p));

            // Intermediate node coordinates (polyline = chain[1..chain.len()-1]).
            let polyline: Vec<(i32, i32)> = chain[1..chain.len() - 1]
                .iter()
                .map(|&nid| {
                    let nd = &g.nodes[nid as usize];
                    (nd.lat_e7, nd.lon_e7)
                })
                .collect();

            // Helper: accumulate cost/length/flags/scenery along a sequence of pairs.
            let accumulate = |pairs: &[(NodeId, NodeId)]| -> (f32, f32, EdgeFlags, u8) {
                let mut cost = 0.0f32;
                let mut length_m = 0.0f32;
                let mut flags = EdgeFlags::default();
                let mut scenic_score = 0u8;
                for p in pairs {
                    if let Some(ed) = edge_map.get(p) {
                        cost += ed.cost;
                        length_m += ed.length_m;
                        flags |= ed.flags;
                        scenic_score = scenic_score.max(ed.scenic_score);
                    }
                }
                (cost, length_m, flags, scenic_score)
            };

            if has_forward {
                let (cost, length_m, flags, scenic_score) = accumulate(&pairs_forward);
                contracted_edges.push((
                    endpoint_a,
                    endpoint_b,
                    EdgeData { cost, length_m, flags, scenic_score, polyline: polyline.clone() },
                ));
            }
            if has_reverse {
                let (cost, length_m, flags, scenic_score) = accumulate(&pairs_reverse);
                let rev_polyline: Vec<(i32, i32)> = polyline.iter().copied().rev().collect();
                contracted_edges.push((
                    endpoint_b,
                    endpoint_a,
                    EdgeData { cost, length_m, flags, scenic_score, polyline: rev_polyline },
                ));
            }
        }
    }

    // Also carry over all edges that connect two surviving (non-consumed) nodes
    // (i.e. edges that were never part of a contracted chain).
    for u in 0..n as NodeId {
        if consumed_d2.contains(&u) {
            continue;
        }
        for &(v, eid) in g.neighbors(u) {
            if consumed_d2.contains(&v) {
                continue; // this edge is part of a chain — already handled
            }
            contracted_edges.push((u, v, g.edges[eid as usize].clone()));
        }
    }

    // ------------------------------------------------------------------
    // 5. Renumber surviving nodes (nodes not consumed by a chain) and
    //    rebuild the graph in CSR format.
    //    Note: degree-2 nodes in pure cycles are NOT consumed and survive.
    // ------------------------------------------------------------------
    // old_to_new[old_id] = Some(new_id) if the node survives.
    let mut old_to_new: Vec<Option<NodeId>> = vec![None; n];
    let mut new_nodes: Vec<NodeData> = Vec::new();

    for old in 0..n as NodeId {
        if !consumed_d2.contains(&old) {
            let new_id = new_nodes.len() as NodeId;
            old_to_new[old as usize] = Some(new_id);
            new_nodes.push(g.nodes[old as usize]);
        }
    }

    let new_node_count = new_nodes.len();

    // Translate contracted edges to new node IDs, dropping any that lost
    // their endpoints (should not happen, but guard defensively).
    let mut raw: Vec<(NodeId, NodeId, EdgeData)> = Vec::new();
    for (src_old, dst_old, ed) in contracted_edges {
        if let (Some(src_new), Some(dst_new)) =
            (old_to_new[src_old as usize], old_to_new[dst_old as usize])
        {
            raw.push((src_new, dst_new, ed));
        }
    }

    // Sort by source for CSR construction.
    raw.sort_unstable_by_key(|&(src, _, _)| src);

    let new_edge_count = raw.len();
    let mut offsets = vec![0u32; new_node_count + 1];
    let mut neighbors: Vec<(NodeId, u32)> = Vec::with_capacity(new_edge_count);
    let mut edges: Vec<EdgeData> = Vec::with_capacity(new_edge_count);

    for &(src, _, _) in &raw {
        offsets[src as usize + 1] += 1;
    }
    for i in 1..=new_node_count {
        offsets[i] += offsets[i - 1];
    }

    for (eid, (_, dst, ed)) in raw.into_iter().enumerate() {
        neighbors.push((dst, eid as u32));
        edges.push(ed);
    }

    tracing::info!(after = new_node_count, "contraction complete");

    Graph {
        nodes: new_nodes,
        offsets,
        neighbors,
        edges,
        node_count: new_node_count,
        edge_count: new_edge_count,
        fuel_stations,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a graph manually from (src, dst, cost, length_m) tuples.
    /// All edges are bidirectional.
    fn build_linear_graph(node_count: usize, edges: &[(usize, usize, f32, f32)]) -> Graph {
        let nodes: Vec<NodeData> = (0..node_count)
            .map(|i| NodeData {
                lat_e7: (i as i32) * 10_000_000,
                lon_e7: 0,
            })
            .collect();

        let mut raw: Vec<(NodeId, NodeId, EdgeData)> = Vec::new();
        for &(src, dst, cost, length_m) in edges {
            let ed = EdgeData {
                cost,
                length_m,
                flags: EdgeFlags::default(),
                scenic_score: 0,
                polyline: Vec::new(),
            };
            raw.push((src as NodeId, dst as NodeId, ed.clone()));
            raw.push((dst as NodeId, src as NodeId, ed));
        }

        raw.sort_unstable_by_key(|&(s, _, _)| s);

        let edge_count = raw.len();
        let mut offsets = vec![0u32; node_count + 1];
        let mut neighbors: Vec<(NodeId, u32)> = Vec::with_capacity(edge_count);
        let mut graph_edges: Vec<EdgeData> = Vec::with_capacity(edge_count);

        for &(src, _, _) in &raw {
            offsets[src as usize + 1] += 1;
        }
        for i in 1..=node_count {
            offsets[i] += offsets[i - 1];
        }
        for (eid, (_, dst, ed)) in raw.into_iter().enumerate() {
            neighbors.push((dst, eid as u32));
            graph_edges.push(ed);
        }

        Graph {
            nodes,
            offsets,
            neighbors,
            edges: graph_edges,
            node_count,
            edge_count,
            fuel_stations: Vec::new(),
        }
    }

    /// A → B → C → D → E (all bidirectional).
    /// B, C, D are pure degree-2 nodes.
    /// After contraction only A (0) and E (4) should survive,
    /// connected by 2 edges (A→E and E→A).
    #[test]
    fn linear_chain_contracts_to_two_nodes() {
        // nodes: 0=A, 1=B, 2=C, 3=D, 4=E
        // edges: A-B, B-C, C-D, D-E  (each with cost=1, length=100)
        let g = build_linear_graph(
            5,
            &[
                (0, 1, 1.0, 100.0),
                (1, 2, 1.0, 100.0),
                (2, 3, 1.0, 100.0),
                (3, 4, 1.0, 100.0),
            ],
        );

        assert_eq!(g.node_count, 5);
        assert_eq!(g.edge_count, 8); // 4 bidirectional pairs

        let cg = contract(g);

        // Only A and E survive.
        assert_eq!(cg.node_count, 2, "only endpoints A and E should remain");
        // Two directed edges: new_A→new_E and new_E→new_A.
        assert_eq!(cg.edge_count, 2, "exactly 2 directed edges expected");

        // Verify accumulated cost and length on the A→E direction.
        let edge_ae = cg
            .neighbors
            .iter()
            .zip(cg.edges.iter())
            .find(|_| true) // either edge is fine; check both
            .map(|(_, ed)| ed)
            .expect("at least one edge");
        assert!(
            (edge_ae.cost - 4.0).abs() < 1e-5,
            "contracted cost should be 4.0, got {}",
            edge_ae.cost
        );
        assert!(
            (edge_ae.length_m - 400.0).abs() < 1e-5,
            "contracted length_m should be 400.0, got {}",
            edge_ae.length_m
        );

        // Polyline should contain the 3 intermediate nodes B, C, D.
        let max_polyline_len = cg.edges.iter().map(|e| e.polyline.len()).max().unwrap_or(0);
        assert_eq!(
            max_polyline_len, 3,
            "polyline should contain 3 intermediate nodes (B, C, D)"
        );
    }

    /// A graph with no degree-2 nodes (triangle A-B-C) should be unchanged.
    #[test]
    fn triangle_is_unchanged() {
        let g = build_linear_graph(
            3,
            &[(0, 1, 1.0, 100.0), (1, 2, 1.0, 100.0), (0, 2, 1.0, 100.0)],
        );
        let nc_before = g.node_count;
        let ec_before = g.edge_count;

        let cg = contract(g);

        assert_eq!(cg.node_count, nc_before, "triangle nodes should be unchanged");
        assert_eq!(cg.edge_count, ec_before, "triangle edges should be unchanged");
    }
}
