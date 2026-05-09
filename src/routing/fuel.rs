use rstar::RTree;
use rstar::primitives::GeomWithData;

use crate::graph::Graph;
use crate::geom::haversine_m;
use super::dijkstra::Route;

type FuelPoint = GeomWithData<[f64; 2], usize>;

/// A planned fuel stop along a route.
#[derive(Debug, Clone)]
pub struct FuelStop {
    /// Distance along the route (in metres) at which the stop is planned.
    pub at_distance_m: f32,
    pub lat_e7: i32,
    pub lon_e7: i32,
    #[allow(dead_code)]
    pub osm_id: i64,
}

/// Walk the route nodes in order and return a list of recommended fuel stops.
///
/// `fuel_buffer` — fraction of `tank_range_km` kept as a reserve; a stop is
/// triggered when remaining range drops to `fuel_buffer * tank_range_km`.
/// Typical value: `0.20` (20 % reserve).
///
/// Returns an empty Vec if `graph.fuel_stations` is empty or the route is too
/// short to require a stop.

/// Straight-line snap radius for lookback station matching. Wide enough to
/// catch a town a km or two off the trail; tight enough to exclude cities
/// separated from the route by a mountain range.
const SNAP_RADIUS_M: f32 = 2_000.0;

pub fn plan_fuel_stops(route: &Route, graph: &Graph, tank_range_km: f32, fuel_buffer: f32) -> Vec<FuelStop> {
    if graph.fuel_stations.is_empty() {
        return Vec::new();
    }

    let tank_range_m = tank_range_km * 1000.0;
    let trigger_at = tank_range_m * (1.0 - fuel_buffer);

    let tree: RTree<FuelPoint> = RTree::bulk_load(
        graph.fuel_stations.iter().enumerate()
            .map(|(i, s)| GeomWithData::new(
                [s.lon_e7 as f64 / 1e7, s.lat_e7 as f64 / 1e7],
                i,
            ))
            .collect(),
    );

    let nodes = &route.nodes;

    // Pre-compute cumulative distance at each node index.
    let mut cum: Vec<f32> = vec![0.0; nodes.len()];
    for i in 1..nodes.len() {
        let prev = &graph.nodes[nodes[i - 1] as usize];
        let curr = &graph.nodes[nodes[i] as usize];
        let seg_m = haversine_m(
            prev.lat_e7 as f64 / 1e7, prev.lon_e7 as f64 / 1e7,
            curr.lat_e7 as f64 / 1e7, curr.lon_e7 as f64 / 1e7,
        ) as f32;
        cum[i] = cum[i - 1] + seg_m;
    }

    // Pre-compute the nearest fuel station (within snap radius) for each route
    // node. One RTree query per node instead of one per lookback step, reducing
    // worst-case complexity from O(n²) to O(n).
    let nearest_fuel: Vec<Option<usize>> = nodes.iter().map(|&nid| {
        let n = &graph.nodes[nid as usize];
        let lat = n.lat_e7 as f64 / 1e7;
        let lon = n.lon_e7 as f64 / 1e7;
        tree.nearest_neighbor(&[lon, lat]).and_then(|pt| {
            let s = &graph.fuel_stations[pt.data];
            let d = haversine_m(lat, lon, s.lat_e7 as f64 / 1e7, s.lon_e7 as f64 / 1e7) as f32;
            if d <= SNAP_RADIUS_M { Some(pt.data) } else { None }
        })
    }).collect();

    let mut stops: Vec<FuelStop> = Vec::new();
    let mut last_refuel_cum: f32 = 0.0;
    let mut last_stop_i: usize = 0;

    let mut i = 1;
    while i < nodes.len() {
        if cum[i] - last_refuel_cum >= trigger_at {
            // Walk backward from the trigger node to find the most recently
            // passed station within SNAP_RADIUS_M. Using the latest match
            // (closest to trigger point) maximises how far we travel before
            // needing the next stop. The radius is wide enough to catch a
            // town the route passes a km or two from (e.g. Toledo on Hwy 20)
            // but tight enough to exclude cities separated by a mountain range
            // that are only close straight-line (e.g. Salem from a ridgeline).
            let found = (last_stop_i..=i).rev().find_map(|j| {
                nearest_fuel[j].map(|station_idx| (j, station_idx))
            });

            // Fallback: nothing within snap radius along the route — use the
            // nearest station to the trigger node (remote stretch with no
            // town nearby; accept whatever detour is needed).
            let (stop_node_i, station_idx) = found.unwrap_or_else(|| {
                let n = &graph.nodes[nodes[i] as usize];
                let lat = n.lat_e7 as f64 / 1e7;
                let lon = n.lon_e7 as f64 / 1e7;
                let pt = tree.nearest_neighbor(&[lon, lat]).expect("non-empty tree");
                (i, pt.data)
            });

            let s = &graph.fuel_stations[station_idx];
            stops.push(FuelStop {
                at_distance_m: cum[stop_node_i],
                lat_e7: s.lat_e7,
                lon_e7: s.lon_e7,
                osm_id: s.osm_id,
            });

            last_refuel_cum = cum[stop_node_i];
            last_stop_i = stop_node_i + 1;

            // Advance i past the trigger zone relative to the new refuel point
            // so we don't immediately re-fire on every subsequent node.
            while i < nodes.len() && cum[i] - last_refuel_cum >= trigger_at {
                i += 1;
            }
            continue;
        }
        i += 1;
    }

    stops
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Graph, NodeData, FuelStation};

    fn make_graph(nodes: Vec<NodeData>, fuel_stations: Vec<FuelStation>) -> Graph {
        let node_count = nodes.len();
        Graph {
            node_count,
            edge_count: 0,
            nodes,
            offsets: vec![0u32; node_count + 1],
            neighbors: vec![],
            edges: vec![],
            fuel_stations,
        }
    }

    fn make_route(node_ids: Vec<u32>) -> Route {
        Route {
            nodes: node_ids,
            edges: vec![],
            length_m: 0.0,
            cost: 0.0,
            unpaved_fraction: 0.0,
            ford_count: 0,
            fourwd_only_count: 0,
        }
    }

    #[test]
    fn no_stops_needed_for_short_route() {
        // Two nodes 10 km apart, tank range 200 km — no stop needed
        let nodes = vec![
            NodeData { lat_e7: 470_000_000, lon_e7: -1_160_000_000 },
            NodeData { lat_e7: 470_900_000, lon_e7: -1_160_000_000 }, // ~10 km north
        ];
        let fuel_stations = vec![FuelStation { lat_e7: 470_450_000, lon_e7: -1_160_000_000, osm_id: 1 }];
        let graph = make_graph(nodes, fuel_stations);
        let route = make_route(vec![0, 1]);
        let stops = plan_fuel_stops(&route, &graph, 200.0, 0.20);
        assert!(stops.is_empty(), "No stop expected for short route");
    }

    #[test]
    fn empty_fuel_stations_returns_empty() {
        let nodes = vec![
            NodeData { lat_e7: 0, lon_e7: 0 },
            NodeData { lat_e7: 100_000_000, lon_e7: 0 }, // ~11,000 km north
        ];
        let graph = make_graph(nodes, vec![]);
        let route = make_route(vec![0, 1]);
        let stops = plan_fuel_stops(&route, &graph, 50.0, 0.20);
        assert!(stops.is_empty(), "No stops when no fuel stations");
    }

    #[test]
    fn lookback_station_does_not_retrigger() {
        // 5 nodes each ~50 km apart (total ~200 km).
        // Station at node 1 (within 5 km). Tank 100 km, trigger at 80 km.
        // Trigger fires at node 2 (100 km). Lookback finds station at node 1 (50 km).
        // After refuelling at 50 km, remaining distance to node 4 = 150 km < 80 km
        // (since 200 - 50 = 150 km, but next trigger = 50 + 80 = 130 km, reached at node 3).
        // Expect exactly 2 stops total, not a flood.
        let lat_per_50km = 4_500_000_i32; // ~0.45 deg ≈ 50 km
        let nodes: Vec<NodeData> = (0..5)
            .map(|i| NodeData { lat_e7: i * lat_per_50km, lon_e7: 0 })
            .collect();
        // Station right at node 1
        let fuel_stations = vec![
            FuelStation { lat_e7: lat_per_50km, lon_e7: 0, osm_id: 10 },
            FuelStation { lat_e7: 3 * lat_per_50km, lon_e7: 0, osm_id: 20 },
        ];
        let graph = make_graph(nodes, fuel_stations);
        let route = make_route(vec![0, 1, 2, 3, 4]);
        let stops = plan_fuel_stops(&route, &graph, 100.0, 0.20);
        assert_eq!(stops.len(), 2, "expected exactly 2 stops, got {}: {:?}", stops.len(), stops.iter().map(|s| s.osm_id).collect::<Vec<_>>());
        assert_eq!(stops[0].osm_id, 10);
        assert_eq!(stops[1].osm_id, 20);
    }

    #[test]
    fn stop_is_triggered_at_80_percent() {
        // Place nodes 1 degree of latitude apart (~111 km each segment).
        // Tank range = 100 km → trigger at 80 km. After the first segment
        // (111 km) we've already exceeded 80 km, so one stop should fire.
        let nodes = vec![
            NodeData { lat_e7: 0, lon_e7: 0 },
            NodeData { lat_e7: 10_000_000, lon_e7: 0 }, // ~1111 km north
        ];
        let fuel_stations = vec![FuelStation { lat_e7: 5_000_000, lon_e7: 0, osm_id: 42 }];
        let graph = make_graph(nodes, fuel_stations);
        let route = make_route(vec![0, 1]);
        let stops = plan_fuel_stops(&route, &graph, 100.0, 0.20);
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].osm_id, 42);
    }
}
