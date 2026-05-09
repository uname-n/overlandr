use std::collections::{BinaryHeap, HashMap};
use std::cmp::Reverse;
use crate::graph::{Graph, NodeId, EdgeId, EdgeFlags};

/// A computed route through the graph.
pub struct Route {
    /// Ordered sequence of node IDs from origin to destination.
    pub nodes: Vec<NodeId>,
    /// Ordered sequence of edge IDs traversed.
    pub edges: Vec<EdgeId>,
    /// Total great-circle length in metres.
    pub length_m: f32,
    /// Raw routing cost (abstract units; used internally for penalty scaling).
    #[allow(dead_code)]
    pub cost: f32,
    /// Fraction of total length on unpaved surface (0.0 – 1.0).
    pub unpaved_fraction: f32,
    /// Number of water-crossing (ford) edges on this route.
    pub ford_count: u32,
    /// Number of 4WD-only edges on this route.
    pub fourwd_only_count: u32,
}

/// Bidirectional Dijkstra shortest-path search.
///
/// `penalties` multiplies base edge cost — a penalty of 2.0 doubles the cost
/// of that edge, making the algorithm prefer routes around it.
///
/// Returns `None` if no path exists between `from` and `to`.
pub fn dijkstra(
    g: &Graph,
    from: NodeId,
    to: NodeId,
    penalties: &HashMap<EdgeId, f32>,
) -> Option<Route> {
    let n = g.node_count;

    // Handle trivial case
    if from == to {
        return Some(Route {
            nodes: vec![from],
            edges: vec![],
            length_m: 0.0,
            cost: 0.0,
            unpaved_fraction: 0.0,
            ford_count: 0,
            fourwd_only_count: 0,
        });
    }

    // Build reverse adjacency list for backward search
    // rev_adj[v] = list of (u, edge_id) meaning there is an edge u -> v
    let mut rev_adj: Vec<Vec<(NodeId, EdgeId)>> = vec![vec![]; n];
    for u in 0..n {
        for &(v, eid) in g.neighbors(u as NodeId) {
            rev_adj[v as usize].push((u as NodeId, eid));
        }
    }

    const INF: f32 = f32::INFINITY;

    let mut dist_fwd = vec![INF; n];
    let mut dist_bwd = vec![INF; n];
    // prev_fwd[v] = (prev_node, edge_id) in the forward tree
    let mut prev_fwd: Vec<Option<(NodeId, EdgeId)>> = vec![None; n];
    // prev_bwd[v] = (next_node, edge_id) in the backward tree (v -> next_node is an original edge)
    let mut prev_bwd: Vec<Option<(NodeId, EdgeId)>> = vec![None; n];

    let mut settled_fwd = vec![false; n];
    let mut settled_bwd = vec![false; n];

    // BinaryHeap is a max-heap; wrap costs in Reverse for min-heap behavior.
    // Store cost as f32 bits (u32) — valid since costs are non-negative.
    let mut heap_fwd: BinaryHeap<Reverse<(u32, NodeId)>> = BinaryHeap::new();
    let mut heap_bwd: BinaryHeap<Reverse<(u32, NodeId)>> = BinaryHeap::new();

    dist_fwd[from as usize] = 0.0;
    dist_bwd[to as usize] = 0.0;
    heap_fwd.push(Reverse((0f32.to_bits(), from)));
    heap_bwd.push(Reverse((0f32.to_bits(), to)));

    let mut best = INF;
    let mut meeting_node: Option<NodeId> = None;

    // Alternate between forward and backward steps until both heaps are empty
    // or we can prove the best path is found.
    loop {
        let fwd_done = heap_fwd.is_empty();
        let bwd_done = heap_bwd.is_empty();
        if fwd_done && bwd_done {
            break;
        }

        // Peek at the top cost of each heap to decide which to expand
        let fwd_top = heap_fwd.peek().map(|Reverse((c, _))| f32::from_bits(*c)).unwrap_or(INF);
        let bwd_top = heap_bwd.peek().map(|Reverse((c, _))| f32::from_bits(*c)).unwrap_or(INF);

        // Termination condition: if both tops exceed best, we're done
        if fwd_top + bwd_top >= best {
            break;
        }

        // Expand the cheaper frontier
        if fwd_top <= bwd_top {
            if let Some(Reverse((cost_bits, u))) = heap_fwd.pop() {
                let cost_u = f32::from_bits(cost_bits);
                if settled_fwd[u as usize] {
                    continue;
                }
                if cost_u > dist_fwd[u as usize] {
                    continue;
                }
                settled_fwd[u as usize] = true;

                // If this node was already settled by backward search, we have a candidate
                if settled_bwd[u as usize] {
                    let total = dist_fwd[u as usize] + dist_bwd[u as usize];
                    if total < best {
                        best = total;
                        meeting_node = Some(u);
                    }
                }

                for &(v, eid) in g.neighbors(u) {
                    let edge = &g.edges[eid as usize];
                    let penalty = penalties.get(&eid).copied().unwrap_or(1.0);
                    let new_cost = cost_u + edge.cost * penalty;
                    if new_cost < dist_fwd[v as usize] {
                        dist_fwd[v as usize] = new_cost;
                        prev_fwd[v as usize] = Some((u, eid));
                        heap_fwd.push(Reverse((new_cost.to_bits(), v)));
                    }
                }
            }
        } else {
            if let Some(Reverse((cost_bits, v))) = heap_bwd.pop() {
                let cost_v = f32::from_bits(cost_bits);
                if settled_bwd[v as usize] {
                    continue;
                }
                if cost_v > dist_bwd[v as usize] {
                    continue;
                }
                settled_bwd[v as usize] = true;

                // If this node was already settled by forward search, we have a candidate
                if settled_fwd[v as usize] {
                    let total = dist_fwd[v as usize] + dist_bwd[v as usize];
                    if total < best {
                        best = total;
                        meeting_node = Some(v);
                    }
                }

                for &(u, eid) in &rev_adj[v as usize] {
                    let edge = &g.edges[eid as usize];
                    let penalty = penalties.get(&eid).copied().unwrap_or(1.0);
                    let new_cost = cost_v + edge.cost * penalty;
                    if new_cost < dist_bwd[u as usize] {
                        dist_bwd[u as usize] = new_cost;
                        // Store (v, eid): the original edge goes u -> v
                        prev_bwd[u as usize] = Some((v, eid));
                        heap_bwd.push(Reverse((new_cost.to_bits(), u)));
                    }
                }
            }
        }
    }

    // Also scan all nodes that were settled by both to catch the actual best meeting point.
    // The loop above may have missed some due to alternating order; do a final pass.
    for i in 0..n {
        if settled_fwd[i] && settled_bwd[i] {
            let total = dist_fwd[i] + dist_bwd[i];
            if total < best {
                best = total;
                meeting_node = Some(i as NodeId);
            }
        }
    }

    let mid = meeting_node?;

    // Reconstruct forward path: from -> ... -> mid
    let mut fwd_nodes = vec![mid];
    let mut fwd_edges = vec![];
    let mut cur = mid;
    while let Some((prev, eid)) = prev_fwd[cur as usize] {
        fwd_edges.push(eid);
        fwd_nodes.push(prev);
        cur = prev;
    }
    fwd_nodes.reverse();
    fwd_edges.reverse();

    // Reconstruct backward path: mid -> ... -> to
    // prev_bwd[u] = Some((v, eid)) means original edge u -> v existed
    let mut bwd_nodes = vec![];
    let mut bwd_edges = vec![];
    cur = mid;
    while let Some((next, eid)) = prev_bwd[cur as usize] {
        bwd_edges.push(eid);
        bwd_nodes.push(next);
        cur = next;
    }

    // Combine: fwd_nodes already ends at mid; append bwd_nodes (which goes mid -> to)
    let mut nodes = fwd_nodes;
    nodes.extend_from_slice(&bwd_nodes);
    let mut edges = fwd_edges;
    edges.extend_from_slice(&bwd_edges);

    // Compute route statistics
    let mut total_length = 0.0f32;
    let mut total_cost = 0.0f32;
    let mut unpaved_length = 0.0f32;
    let mut ford_count = 0u32;
    let mut fourwd_only_count = 0u32;

    for &eid in &edges {
        let edge = &g.edges[eid as usize];
        let penalty = penalties.get(&eid).copied().unwrap_or(1.0);
        total_length += edge.length_m;
        total_cost += edge.cost * penalty;
        if !edge.flags.contains(EdgeFlags::PAVED) {
            unpaved_length += edge.length_m;
        }
        if edge.flags.contains(EdgeFlags::FORD) {
            ford_count += 1;
        }
        if edge.flags.contains(EdgeFlags::FOURWD_ONLY) {
            fourwd_only_count += 1;
        }
    }

    let unpaved_fraction = if total_length > 0.0 {
        unpaved_length / total_length
    } else {
        0.0
    };

    Some(Route {
        nodes,
        edges,
        length_m: total_length,
        cost: total_cost,
        unpaved_fraction,
        ford_count,
        fourwd_only_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, NodeData, EdgeData, EdgeFlags};
    use std::collections::HashMap;

    /// Build a Graph from a list of directed edges.
    /// Edges are specified as (from, to, cost, length_m, flags).
    /// The graph is made bidirectional by adding reverse edges automatically.
    fn build_test_graph(
        node_count: usize,
        directed_edges: &[(usize, usize, f32, f32, EdgeFlags)],
    ) -> Graph {
        // Build forward and reverse edges
        let mut all_edges: Vec<(usize, usize, f32, f32, EdgeFlags)> = Vec::new();
        for &(u, v, cost, len, flags) in directed_edges {
            all_edges.push((u, v, cost, len, flags));
            all_edges.push((v, u, cost, len, flags)); // bidirectional
        }

        let edge_count = all_edges.len();
        let mut edges: Vec<EdgeData> = Vec::with_capacity(edge_count);
        // adjacency: node -> Vec<(target, edge_id)>
        let mut adj: Vec<Vec<(NodeId, EdgeId)>> = vec![vec![]; node_count];

        for (eid, &(u, v, cost, length_m, flags)) in all_edges.iter().enumerate() {
            adj[u].push((v as NodeId, eid as EdgeId));
            edges.push(EdgeData {
                cost,
                length_m,
                flags,
                scenic_score: 0,
                polyline: vec![],
            });
        }

        // Build CSR
        let mut offsets = vec![0u32; node_count + 1];
        for u in 0..node_count {
            offsets[u + 1] = offsets[u] + adj[u].len() as u32;
        }
        let mut neighbors: Vec<(NodeId, EdgeId)> = Vec::new();
        for u in 0..node_count {
            neighbors.extend_from_slice(&adj[u]);
        }

        let nodes: Vec<NodeData> = (0..node_count)
            .map(|_| NodeData { lat_e7: 0, lon_e7: 0 })
            .collect();

        Graph {
            node_count,
            edge_count,
            nodes,
            offsets,
            neighbors,
            edges,
            fuel_stations: vec![],
        }
    }

    #[test]
    fn test_linear_3_node_graph() {
        // A(0) --e0--> B(1) --e1--> C(2), bidirectional
        // Direct edges: 0->1, 1->2; plus reverses 1->0, 2->1
        let g = build_test_graph(3, &[
            (0, 1, 1.0, 100.0, EdgeFlags::PAVED),
            (1, 2, 1.0, 100.0, EdgeFlags::PAVED),
        ]);

        let route = dijkstra(&g, 0, 2, &HashMap::new());
        assert!(route.is_some(), "Expected a route from A to C");
        let route = route.unwrap();
        assert_eq!(route.nodes.first(), Some(&0));
        assert_eq!(route.nodes.last(), Some(&2));
        assert_eq!(route.edges.len(), 2, "Expected 2 edges in the route");
        assert!((route.length_m - 200.0).abs() < 1e-3);
    }

    #[test]
    fn test_disconnected_graph() {
        // Two isolated nodes with no edges
        let g = build_test_graph(2, &[]);
        let route = dijkstra(&g, 0, 1, &HashMap::new());
        assert!(route.is_none(), "Expected None for disconnected graph");
    }

    #[test]
    fn test_penalty_reroutes_around_edge() {
        // Graph: A(0) -- direct --> C(2), cost 1.0
        //        A(0) --> B(1) --> C(2), cost 1.0 + 1.0 = 2.0
        // With no penalty: direct A->C is chosen (lower cost)
        // With penalty 100.0 on A->C edge: route goes A->B->C
        //
        // Build: edges (0,2) direct, (0,1) and (1,2) indirect
        // We need to identify which edge_id corresponds to A->C.
        // In build_test_graph, edges are added as pairs (fwd, rev).
        // directed_edges[0] = (0,2): eid 0 = 0->2, eid 1 = 2->0
        // directed_edges[1] = (0,1): eid 2 = 0->1, eid 3 = 1->0
        // directed_edges[2] = (1,2): eid 4 = 1->2, eid 5 = 2->1
        let g = build_test_graph(3, &[
            (0, 2, 1.0, 50.0, EdgeFlags::PAVED),   // direct, short
            (0, 1, 1.0, 100.0, EdgeFlags::PAVED),  // indirect via B
            (1, 2, 1.0, 100.0, EdgeFlags::PAVED),
        ]);

        // Without penalty: route should use direct edge (cheaper cost)
        let no_penalty = dijkstra(&g, 0, 2, &HashMap::new());
        assert!(no_penalty.is_some());
        let no_penalty = no_penalty.unwrap();
        assert_eq!(no_penalty.edges.len(), 1, "Direct route should use 1 edge");

        // Penalize the direct A->C edge (edge id 0) heavily
        let mut penalties = HashMap::new();
        penalties.insert(0u32, 100.0f32); // eid 0: 0->2 forward
        penalties.insert(1u32, 100.0f32); // eid 1: 2->0 reverse (used in backward search)

        let penalized = dijkstra(&g, 0, 2, &penalties);
        assert!(penalized.is_some(), "Should still find a route via B");
        let penalized = penalized.unwrap();
        assert_eq!(penalized.edges.len(), 2, "Penalized route should go via B (2 edges)");
    }

    #[test]
    fn test_unpaved_fraction_and_flags() {
        // A(0) --paved--> B(1) --unpaved+ford--> C(2)
        let g = build_test_graph(3, &[
            (0, 1, 1.0, 100.0, EdgeFlags::PAVED),
            (1, 2, 1.0, 100.0, EdgeFlags(EdgeFlags::FORD.0 | EdgeFlags::FOURWD_ONLY.0)),
        ]);

        let route = dijkstra(&g, 0, 2, &HashMap::new()).unwrap();
        assert!((route.unpaved_fraction - 0.5).abs() < 1e-3,
            "Half the route is unpaved, got {}", route.unpaved_fraction);
        assert_eq!(route.ford_count, 1);
        assert_eq!(route.fourwd_only_count, 1);
    }

    #[test]
    fn test_same_node() {
        let g = build_test_graph(3, &[
            (0, 1, 1.0, 100.0, EdgeFlags::PAVED),
        ]);
        let route = dijkstra(&g, 1, 1, &HashMap::new());
        assert!(route.is_some());
        let route = route.unwrap();
        assert_eq!(route.nodes, vec![1]);
        assert!(route.edges.is_empty());
        assert_eq!(route.length_m, 0.0);
    }
}
