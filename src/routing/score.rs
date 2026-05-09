use crate::routing::dijkstra::Route;

/// Weight applied to unpaved fraction in the route score.
pub const UNPAVED_WEIGHT: f32 = 0.6;
/// Weight applied to detour penalty in the route score.
pub const DETOUR_WEIGHT: f32 = 0.4;

/// Score each route with a composite metric and return them sorted best-first.
///
/// Score = unpaved_fraction * UNPAVED_WEIGHT - (length_m / shortest_length_m - 1.0) * DETOUR_WEIGHT
///
/// Higher score is better: more unpaved content is rewarded (overland tool),
/// and detour from the shortest route is penalised.
pub fn score_and_sort(mut routes: Vec<Route>) -> Vec<Route> {
    if routes.is_empty() {
        return routes;
    }

    let shortest = routes[0].length_m;

    routes.sort_by(|a, b| {
        let score_a = a.unpaved_fraction * UNPAVED_WEIGHT
            - (a.length_m / shortest.max(f32::EPSILON) - 1.0) * DETOUR_WEIGHT;
        let score_b = b.unpaved_fraction * UNPAVED_WEIGHT
            - (b.length_m / shortest.max(f32::EPSILON) - 1.0) * DETOUR_WEIGHT;
        score_b
            .partial_cmp(&score_a)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    routes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::dijkstra::Route;

    fn make_route(length_m: f32, unpaved_fraction: f32) -> Route {
        Route {
            nodes: vec![],
            edges: vec![],
            length_m,
            cost: length_m,
            unpaved_fraction,
            ford_count: 0,
            fourwd_only_count: 0,
        }
    }

    #[test]
    fn test_score_and_sort_prefers_unpaved() {
        // Route A: shorter but paved; Route B: same length, fully unpaved.
        // Route B should win.
        let routes = vec![
            make_route(1000.0, 0.0),  // A: paved, baseline
            make_route(1000.0, 1.0),  // B: fully unpaved
        ];
        let sorted = score_and_sort(routes);
        assert_eq!(sorted[0].unpaved_fraction, 1.0, "Unpaved route should rank first");
    }

    #[test]
    fn test_score_and_sort_penalises_detour() {
        // Route A: shortest, paved; Route B: 2× longer, fully unpaved.
        // score_A = 0.0 * 0.6 - (1.0 - 1.0) * 0.4 = 0.0
        // score_B = 1.0 * 0.6 - (2.0 - 1.0) * 0.4 = 0.6 - 0.4 = 0.2
        // B still wins here; just verify ordering is consistent.
        let routes = vec![
            make_route(1000.0, 0.0),
            make_route(2000.0, 1.0),
        ];
        let sorted = score_and_sort(routes);
        // The one with score 0.2 (unpaved/long) should beat score 0.0 (paved/short)
        assert_eq!(sorted[0].length_m, 2000.0);
    }
}
