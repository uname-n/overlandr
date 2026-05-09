use std::collections::{HashMap, HashSet};
use crate::graph::{Graph, NodeId, EdgeId};
use crate::routing::dijkstra::{dijkstra, Route};
use crate::routing::score::score_and_sort;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

pub struct AltConfig {
    /// Penalty multiplier applied to edges of an accepted route so future
    /// searches are discouraged from reusing them.
    pub lambda: f32,
    /// Maximum number of Dijkstra calls per candidate slot before giving up.
    pub max_retries: usize,
    /// Minimum Jaccard *distance* (1 − similarity) required for a candidate
    /// to be considered topologically distinct from every accepted route.
    pub min_jaccard_distance: f32,
    /// Maximum detour ratio relative to the shortest route's length.
    pub max_detour: f32,
    /// Baseline penalties applied to every Dijkstra call.  Use this to
    /// implement avoid-paved / avoid-ford style preferences: set a high
    /// multiplier on the relevant edge IDs before routing begins.
    pub initial_penalties: HashMap<EdgeId, f32>,
}

impl Default for AltConfig {
    fn default() -> Self {
        AltConfig {
            lambda: 1.5,
            max_retries: 4,
            min_jaccard_distance: 0.35,
            max_detour: 1.6,
            initial_penalties: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Jaccard distance
// ---------------------------------------------------------------------------

/// Edge-set Jaccard distance between two routes.
///
/// `distance = 1 − |A ∩ B| / |A ∪ B|`
///
/// Returns `0.0` when both routes have no edges (identical empty sets).
pub fn jaccard_distance(a: &Route, b: &Route) -> f32 {
    let set_a: HashSet<EdgeId> = a.edges.iter().copied().collect();
    let set_b: HashSet<EdgeId> = b.edges.iter().copied().collect();

    let union = set_a.len() + set_b.len();
    if union == 0 {
        return 0.0;
    }

    let intersection = set_a.intersection(&set_b).count();
    // |A ∪ B| = |A| + |B| - |A ∩ B|
    let union_size = set_a.len() + set_b.len() - intersection;
    1.0 - (intersection as f32 / union_size as f32)
}

// ---------------------------------------------------------------------------
// k-alternatives (iterative penalty method)
// ---------------------------------------------------------------------------

/// Find up to `k` topologically diverse routes from `from` to `to`.
///
/// Implements the pseudocode from DESIGN.md §8.4:
/// 1. Compute the shortest route R_0 with no penalties.
/// 2. Maintain a `penalties` map; penalise edges of each accepted route by
///    multiplying their penalty by `cfg.lambda` (inserting `lambda` if absent).
/// 3. For each additional slot, retry up to `cfg.max_retries` times, each time
///    doubling a local penalty on the last candidate's edges when it fails the
///    diversity or detour check.
/// 4. Score and sort the collected routes before returning.
pub fn k_alternatives(
    g: &Graph,
    from: NodeId,
    to: NodeId,
    k: usize,
    cfg: &AltConfig,
) -> Vec<Route> {
    if k == 0 {
        return vec![];
    }

    // Step 1: find R_0 starting from the baseline penalties.
    let mut penalties: HashMap<EdgeId, f32> = cfg.initial_penalties.clone();

    let r0 = match dijkstra(g, from, to, &penalties) {
        Some(r) => r,
        None => return vec![],
    };

    let mut routes: Vec<Route> = Vec::with_capacity(k);

    // Apply base penalty to R_0's edges.
    apply_penalty(&r0, &mut penalties, cfg.lambda);
    routes.push(r0);

    // Steps 3-4: fill remaining slots.
    while routes.len() < k {
        let mut local_lambda = cfg.lambda;
        let mut accepted: Option<Route> = None;

        'retry: for _attempt in 0..cfg.max_retries {
            let cand = match dijkstra(g, from, to, &penalties) {
                Some(r) => r,
                None => break 'retry,
            };

            // Diversity check: candidate must be far enough from every prior route.
            let is_diverse = routes
                .iter()
                .all(|r| jaccard_distance(r, &cand) >= cfg.min_jaccard_distance);

            // Detour check: candidate must not be too much longer than R_0.
            let within_detour = cand.length_m <= routes[0].length_m * cfg.max_detour;

            if is_diverse && within_detour {
                accepted = Some(cand);
                break 'retry;
            }

            // Failed: increase local penalty on candidate's edges and retry.
            local_lambda *= 2.0;
            apply_penalty(&cand, &mut penalties, local_lambda);
        }

        match accepted {
            Some(route) => {
                apply_penalty(&route, &mut penalties, cfg.lambda);
                routes.push(route);
            }
            None => break, // Can't find more diverse routes.
        }
    }

    score_and_sort(routes)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Multiply each edge's penalty by `factor`, inserting `factor` if not present.
fn apply_penalty(route: &Route, penalties: &mut HashMap<EdgeId, f32>, factor: f32) {
    for &eid in &route.edges {
        let entry = penalties.entry(eid).or_insert(1.0);
        *entry *= factor;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, NodeData, EdgeData, EdgeFlags};

    /// Build a simple directed (bidirectional) graph from a list of edges.
    /// Each input edge `(u, v, cost, length_m, flags)` also generates its reverse.
    fn build_graph(
        node_count: usize,
        directed_edges: &[(usize, usize, f32, f32, EdgeFlags)],
    ) -> Graph {
        let mut all_edges: Vec<(usize, usize, f32, f32, EdgeFlags)> = Vec::new();
        for &(u, v, cost, len, flags) in directed_edges {
            all_edges.push((u, v, cost, len, flags));
            all_edges.push((v, u, cost, len, flags));
        }

        let edge_count = all_edges.len();
        let mut edges: Vec<EdgeData> = Vec::with_capacity(edge_count);
        let mut adj: Vec<Vec<(NodeId, EdgeId)>> = vec![vec![]; node_count];

        for (eid, &(u, v, cost, length_m, flags)) in all_edges.iter().enumerate() {
            adj[u].push((v as NodeId, eid as EdgeId));
            edges.push(EdgeData { cost, length_m, flags, scenic_score: 0, polyline: vec![] });
        }

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

        Graph { node_count, edge_count, nodes, offsets, neighbors, edges, fuel_stations: vec![] }
    }

    #[test]
    fn test_jaccard_distance_disjoint() {
        // Two routes with completely different edges: distance should be 1.0.
        let r_a = Route {
            nodes: vec![0, 1],
            edges: vec![0, 1],
            length_m: 200.0,
            cost: 2.0,
            unpaved_fraction: 0.0,
            ford_count: 0,
            fourwd_only_count: 0,
        };
        let r_b = Route {
            nodes: vec![2, 3],
            edges: vec![2, 3],
            length_m: 200.0,
            cost: 2.0,
            unpaved_fraction: 0.0,
            ford_count: 0,
            fourwd_only_count: 0,
        };
        assert!((jaccard_distance(&r_a, &r_b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_jaccard_distance_identical() {
        let r = Route {
            nodes: vec![0, 1],
            edges: vec![0, 1],
            length_m: 200.0,
            cost: 2.0,
            unpaved_fraction: 0.0,
            ford_count: 0,
            fourwd_only_count: 0,
        };
        assert!((jaccard_distance(&r, &r) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_k_alternatives_diamond_graph() {
        // Diamond: A(0) → B(1) → D(3)  and  A(0) → C(2) → D(3)
        // The two corridors share no interior edges, so Jaccard distance should be > 0.3.
        //
        // Node layout: 0=A, 1=B, 2=C, 3=D
        // Edges (directed, bidirectional added automatically):
        //   A→B (eid 0/1), B→D (eid 2/3), A→C (eid 4/5), C→D (eid 6/7)
        let g = build_graph(
            4,
            &[
                (0, 1, 1.0, 100.0, EdgeFlags::PAVED),  // A-B
                (1, 3, 1.0, 100.0, EdgeFlags::PAVED),  // B-D
                (0, 2, 1.0, 100.0, EdgeFlags::PAVED),  // A-C
                (2, 3, 1.0, 100.0, EdgeFlags::PAVED),  // C-D
            ],
        );

        let cfg = AltConfig::default();
        let routes = k_alternatives(&g, 0, 3, 2, &cfg);

        assert_eq!(routes.len(), 2, "Expected 2 distinct routes on a diamond graph");

        let dist = jaccard_distance(&routes[0], &routes[1]);
        assert!(
            dist > 0.3,
            "Routes should be topologically distinct; Jaccard distance = {dist}"
        );
    }

    #[test]
    fn test_k_alternatives_no_path() {
        // Disconnected graph: no edges.
        let g = build_graph(2, &[]);
        let routes = k_alternatives(&g, 0, 1, 3, &AltConfig::default());
        assert!(routes.is_empty(), "Expected empty result when no path exists");
    }

    #[test]
    fn test_k_alternatives_single_corridor() {
        // Linear graph A→B→C: only one topological corridor.
        // Requesting k=2 should return just 1 route.
        let g = build_graph(
            3,
            &[
                (0, 1, 1.0, 100.0, EdgeFlags::PAVED),
                (1, 2, 1.0, 100.0, EdgeFlags::PAVED),
            ],
        );
        let routes = k_alternatives(&g, 0, 2, 2, &AltConfig::default());
        assert_eq!(routes.len(), 1, "Only one corridor exists; should return 1 route");
    }
}
