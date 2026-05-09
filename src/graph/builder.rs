//! Converts raw OSM ways into a CSR [`Graph`] with edge costing.

use std::collections::HashMap;

use rstar::{AABB, PointDistance, RTree, RTreeObject};

use crate::geom::haversine_m;
use crate::osm::loader::{NodeMap, Way};
use crate::profile::Profile;

use super::{EdgeData, EdgeFlags, Graph, NodeData, NodeId, ScenicFeature, ScenicKind};

// ---------------------------------------------------------------------------
// Public option types
// ---------------------------------------------------------------------------

/// Axis-aligned bounding box in WGS-84 degrees.
#[derive(Debug, Clone)]
pub struct BBox {
    pub min_lon: f64,
    pub min_lat: f64,
    pub max_lon: f64,
    pub max_lat: f64,
}

impl BBox {
    fn contains(&self, lat: f64, lon: f64) -> bool {
        lat >= self.min_lat
            && lat <= self.max_lat
            && lon >= self.min_lon
            && lon <= self.max_lon
    }
}

/// Options controlling graph construction.
#[derive(Debug, Clone)]
pub struct BuildOpts {
    /// Keep edges whose way carries `access=private`.
    pub keep_private: bool,
    /// If set, only nodes within this box are included.
    pub bbox: Option<BBox>,
}

impl Default for BuildOpts {
    fn default() -> Self {
        Self { keep_private: false, bbox: None }
    }
}

// ---------------------------------------------------------------------------
// Internal edge buffer (before CSR sort)
// ---------------------------------------------------------------------------

struct RawEdge {
    src: NodeId,
    dst: NodeId,
    data: EdgeData,
}

#[derive(Clone)]
struct ScenicRef {
    lat: f64,
    lon: f64,
    kind: ScenicKind,
}

impl RTreeObject for ScenicRef {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_point([self.lon, self.lat])
    }
}

impl PointDistance for ScenicRef {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let dlat = self.lat - point[1];
        let dlon = self.lon - point[0];
        dlat * dlat + dlon * dlon
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a [`Graph`] from the output of the OSM loader.
///
/// Algorithm (two-phase CSR build):
/// 1. Assign compact [`NodeId`]s to all OSM node IDs referenced by filtered ways.
/// 2. For each way, for each consecutive node pair emit one or two directed
///    edges (both directions unless `oneway=yes`).
/// 3. Sort edges by source node and fill CSR offset array.
pub fn build_graph(
    ways: &[Way],
    nodes: &NodeMap,
    scenic_features: &[ScenicFeature],
    profile: &Profile,
    opts: &BuildOpts,
) -> Graph {
    // ------------------------------------------------------------------
    // Phase 1 — assign compact NodeIds
    // ------------------------------------------------------------------
    let mut osm_to_node: HashMap<i64, NodeId> = HashMap::new();
    let mut node_data: Vec<NodeData> = Vec::new();

    for way in ways {
        for &osm_id in &way.nodes {
            if osm_to_node.contains_key(&osm_id) {
                continue;
            }
            let Some(&(lat, lon)) = nodes.get(&osm_id) else {
                continue; // node not in NodeMap
            };
            if let Some(bbox) = &opts.bbox {
                if !bbox.contains(lat, lon) {
                    continue;
                }
            }
            let nid = node_data.len() as NodeId;
            osm_to_node.insert(osm_id, nid);
            node_data.push(NodeData {
                lat_e7: (lat * 1e7) as i32,
                lon_e7: (lon * 1e7) as i32,
            });
        }
    }

    let node_count = node_data.len();
    let scenic_index = build_scenic_index(scenic_features, opts.bbox.as_ref());

    // ------------------------------------------------------------------
    // Phase 2 — emit edges
    // ------------------------------------------------------------------
    let mut raw_edges: Vec<RawEdge> = Vec::new();
    let mut dropped_segments: usize = 0;

    for way in ways {
        // Skip private ways unless opts say otherwise.
        let is_private = way.tags.get("access").map_or(false, |v| v == "private");
        if is_private && !opts.keep_private {
            continue;
        }

        let oneway = way.tags.get("oneway").map_or(false, |v| v == "yes");

        // Derive edge flags from tags.
        let flags = compute_flags(&way.tags, &profile.routing);

        let highway = way.tags.get("highway").map(String::as_str).unwrap_or("");
        let surface = way.tags.get("surface").map(String::as_str).unwrap_or("");
        let tracktype = way.tags.get("tracktype").map(String::as_str).unwrap_or("");
        let smoothness = way.tags.get("smoothness").map(String::as_str).unwrap_or("");

        let base = profile.base_factor(highway);
        let surf = profile.surface_factor(surface);
        let tt = profile.tracktype_factor(tracktype);
        let sm = profile.smoothness_factor(smoothness);
        let ford_factor = if flags.contains(EdgeFlags::FORD) { profile.routing.ford_penalty } else { 1.0 };

        for pair in way.nodes.windows(2) {
            let osm_a = pair[0];
            let osm_b = pair[1];

            let (Some(&src), Some(&dst)) = (osm_to_node.get(&osm_a), osm_to_node.get(&osm_b))
            else {
                dropped_segments += 1;
                continue;
            };

            let nd_a = &node_data[src as usize];
            let nd_b = &node_data[dst as usize];
            let lat_a = nd_a.lat_e7 as f64 * 1e-7;
            let lon_a = nd_a.lon_e7 as f64 * 1e-7;
            let lat_b = nd_b.lat_e7 as f64 * 1e-7;
            let lon_b = nd_b.lon_e7 as f64 * 1e-7;

            let length_m = haversine_m(lat_a, lon_a, lat_b, lon_b) as f32;
            let cost = length_m * base * surf * tt * sm * ford_factor;
            let scenic_score = score_edge_scenery(lat_a, lon_a, lat_b, lon_b, scenic_index.as_ref());

            raw_edges.push(RawEdge {
                src,
                dst,
                data: EdgeData {
                    cost,
                    length_m,
                    flags,
                    scenic_score,
                    polyline: Vec::new(),
                },
            });

            if !oneway {
                raw_edges.push(RawEdge {
                    src: dst,
                    dst: src,
                    data: EdgeData {
                        cost,
                        length_m,
                        flags,
                        scenic_score,
                        polyline: Vec::new(),
                    },
                });
            }
        }
    }

    if dropped_segments > 0 {
        tracing::debug!(dropped_segments, "way-segments skipped: OSM node not loaded");
    }

    // ------------------------------------------------------------------
    // Phase 3 — build CSR
    // ------------------------------------------------------------------
    // Sort by source node so we can fill the offset array.
    raw_edges.sort_unstable_by_key(|e| e.src);

    let edge_count = raw_edges.len();
    let mut offsets = vec![0u32; node_count + 1];
    let mut neighbors: Vec<(NodeId, u32)> = Vec::with_capacity(edge_count);
    let mut edges: Vec<EdgeData> = Vec::with_capacity(edge_count);

    // Count out-degree for each node.
    for e in &raw_edges {
        offsets[e.src as usize + 1] += 1;
    }
    // Prefix-sum.
    for i in 1..=node_count {
        offsets[i] += offsets[i - 1];
    }

    // Fill neighbor and edge arrays.
    for (edge_id, e) in raw_edges.into_iter().enumerate() {
        neighbors.push((e.dst, edge_id as u32));
        edges.push(e.data);
    }

    Graph {
        nodes: node_data,
        offsets,
        neighbors,
        edges,
        node_count,
        edge_count,
        fuel_stations: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn build_scenic_index(features: &[ScenicFeature], bbox: Option<&BBox>) -> Option<RTree<ScenicRef>> {
    let refs: Vec<ScenicRef> = features
        .iter()
        .filter_map(|feature| {
            let lat = feature.lat_e7 as f64 / 1e7;
            let lon = feature.lon_e7 as f64 / 1e7;
            if let Some(b) = bbox {
                if !b.contains(lat, lon) {
                    return None;
                }
            }
            Some(ScenicRef { lat, lon, kind: feature.kind })
        })
        .collect();

    if refs.is_empty() {
        None
    } else {
        Some(RTree::bulk_load(refs))
    }
}

fn score_edge_scenery(
    lat_a: f64,
    lon_a: f64,
    lat_b: f64,
    lon_b: f64,
    scenic_index: Option<&RTree<ScenicRef>>,
) -> u8 {
    let Some(index) = scenic_index else { return 0; };

    let mid_lat = (lat_a + lat_b) * 0.5;
    let mid_lon = (lon_a + lon_b) * 0.5;
    let query_radius_deg = MAX_SCENIC_RADIUS_M / 111_000.0;
    let envelope = AABB::from_corners(
        [mid_lon - query_radius_deg, mid_lat - query_radius_deg],
        [mid_lon + query_radius_deg, mid_lat + query_radius_deg],
    );

    let mut score = 0.0f32;
    for feature in index.locate_in_envelope_intersecting(&envelope) {
        let dist_m = haversine_m(mid_lat, mid_lon, feature.lat, feature.lon) as f32;
        let Some(max_dist_m) = scenic_radius_m(feature.kind) else { continue; };
        if dist_m > max_dist_m {
            continue;
        }
        let decay = 1.0 - (dist_m / max_dist_m);
        score += scenic_weight(feature.kind) * decay.max(0.0);
    }

    ((score.min(MAX_SCENIC_SCORE) / MAX_SCENIC_SCORE) * 255.0).round() as u8
}

const MAX_SCENIC_SCORE: f32 = 0.9;
const MAX_SCENIC_RADIUS_M: f64 = 15_000.0;

fn scenic_radius_m(kind: ScenicKind) -> Option<f32> {
    match kind {
        ScenicKind::Viewpoint => Some(5_000.0),
        ScenicKind::Peak => Some(4_000.0),
        ScenicKind::Saddle => Some(3_000.0),
        ScenicKind::Water => Some(1_500.0),
        ScenicKind::River => Some(1_200.0),
        ScenicKind::Stream => Some(600.0),
        ScenicKind::Forest => Some(12_000.0),
        ScenicKind::ProtectedArea => Some(15_000.0),
        ScenicKind::NatureReserve => Some(10_000.0),
        ScenicKind::Glacier => Some(8_000.0),
        ScenicKind::Cliff => Some(4_000.0),
    }
}

fn scenic_weight(kind: ScenicKind) -> f32 {
    match kind {
        ScenicKind::Viewpoint => 0.30,
        ScenicKind::Peak => 0.20,
        ScenicKind::Saddle => 0.12,
        ScenicKind::Water => 0.16,
        ScenicKind::River => 0.14,
        ScenicKind::Stream => 0.08,
        ScenicKind::Forest => 0.18,
        ScenicKind::ProtectedArea => 0.22,
        ScenicKind::NatureReserve => 0.20,
        ScenicKind::Glacier => 0.24,
        ScenicKind::Cliff => 0.14,
    }
}

fn compute_flags(tags: &HashMap<String, String>, routing: &crate::profile::RoutingConfig) -> EdgeFlags {
    let mut flags = EdgeFlags::default();

    if let Some(surface) = tags.get("surface") {
        if routing.paved_surfaces.iter().any(|s| s == surface) {
            flags |= EdgeFlags::PAVED;
        }
    }
    if tags.get("ford").map_or(false, |v| v == "yes") {
        flags |= EdgeFlags::FORD;
    }
    if tags.get("4wd_only").map_or(false, |v| v == "yes") {
        flags |= EdgeFlags::FOURWD_ONLY;
    }
    if tags.get("seasonal").map_or(false, |v| v == "yes") {
        flags |= EdgeFlags::SEASONAL;
    }
    if tags.get("access").map_or(false, |v| v == "private") {
        flags |= EdgeFlags::PRIVATE;
    }

    // Smoothness threshold flags for vehicle-profile enforcement at routing time.
    if let Some(sm) = tags.get("smoothness") {
        if routing.smoothness_rough.iter().any(|s| s == sm) {
            flags |= EdgeFlags::SMOOTHNESS_ROUGH;
        }
        if routing.smoothness_very_rough.iter().any(|s| s == sm) {
            flags |= EdgeFlags::SMOOTHNESS_VERY_ROUGH;
        }
    }

    flags
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::load_profile;

    /// Build a minimal NodeMap for three OSM nodes arranged roughly 1 km apart.
    fn three_node_setup() -> (Vec<Way>, NodeMap) {
        let mut nodes: NodeMap = HashMap::new();
        nodes.insert(100, (47.000_000, -116.000_000));
        nodes.insert(101, (47.009_000, -116.000_000));
        nodes.insert(102, (47.018_000, -116.000_000));

        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "track".to_string());

        let ways = vec![Way { id: 1, nodes: vec![100, 101, 102], tags }];
        (ways, nodes)
    }

    #[test]
    fn three_node_way_produces_four_edges() {
        let (ways, nodes) = three_node_setup();
        let profile = load_profile(None).expect("embedded profile must parse");
        let opts = BuildOpts { keep_private: false, bbox: None };
        let graph = build_graph(&ways, &nodes, &[], &profile, &opts);
        assert_eq!(graph.edge_count, 4);
        assert_eq!(graph.node_count, 3);
    }

    #[test]
    fn track_cost_less_than_length() {
        let (ways, nodes) = three_node_setup();
        let profile = load_profile(None).expect("embedded profile must parse");
        let opts = BuildOpts { keep_private: false, bbox: None };
        let graph = build_graph(&ways, &nodes, &[], &profile, &opts);
        for edge in &graph.edges {
            assert!(edge.cost < edge.length_m);
        }
    }

    #[test]
    fn oneway_produces_two_edges() {
        let mut nodes: NodeMap = HashMap::new();
        nodes.insert(1, (47.0, -116.0));
        nodes.insert(2, (47.009, -116.0));

        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "track".to_string());
        tags.insert("oneway".to_string(), "yes".to_string());

        let ways = vec![Way { id: 10, nodes: vec![1, 2], tags }];
        let profile = load_profile(None).expect("embedded profile must parse");
        let opts = BuildOpts { keep_private: false, bbox: None };
        let graph = build_graph(&ways, &nodes, &[], &profile, &opts);
        assert_eq!(graph.edge_count, 1);
    }

    #[test]
    fn paved_flag_set_for_asphalt() {
        let mut nodes: NodeMap = HashMap::new();
        nodes.insert(1, (47.0, -116.0));
        nodes.insert(2, (47.009, -116.0));

        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "road".to_string());
        tags.insert("surface".to_string(), "asphalt".to_string());

        let ways = vec![Way { id: 20, nodes: vec![1, 2], tags }];
        let profile = load_profile(None).expect("embedded profile must parse");
        let opts = BuildOpts { keep_private: false, bbox: None };
        let graph = build_graph(&ways, &nodes, &[], &profile, &opts);

        for edge in &graph.edges {
            assert!(edge.flags.contains(EdgeFlags::PAVED));
        }
    }

    #[test]
    fn bbox_filters_nodes() {
        let mut nodes: NodeMap = HashMap::new();
        nodes.insert(1, (47.0, -116.0));
        nodes.insert(2, (47.009, -116.0));

        let mut tags = HashMap::new();
        tags.insert("highway".to_string(), "track".to_string());

        let ways = vec![Way { id: 30, nodes: vec![1, 2], tags }];
        let profile = load_profile(None).expect("embedded profile must parse");
        let opts = BuildOpts {
            keep_private: false,
            bbox: Some(BBox {
                min_lat: 46.9,
                max_lat: 47.005,
                min_lon: -116.1,
                max_lon: -115.9,
            }),
        };

        let graph = build_graph(&ways, &nodes, &[], &profile, &opts);
        assert_eq!(graph.edge_count, 0);
    }

    #[test]
    fn scenic_features_raise_edge_score() {
        let (ways, nodes) = three_node_setup();
        let profile = load_profile(None).expect("embedded profile must parse");
        let opts = BuildOpts { keep_private: false, bbox: None };
        let scenic = vec![ScenicFeature {
            lat_e7: 470045000,
            lon_e7: -1160000000,
            kind: ScenicKind::Viewpoint,
            osm_id: 99,
        }];

        let graph = build_graph(&ways, &nodes, &scenic, &profile, &opts);
        assert!(graph.edges.iter().any(|e| e.scenic_score > 0));
    }

    #[test]
    fn distant_scenic_features_do_not_affect_edge_score() {
        let scenic = vec![ScenicFeature {
            lat_e7: 480000000,
            lon_e7: -1160000000,
            kind: ScenicKind::Stream,
            osm_id: 1,
        }];
        let index = build_scenic_index(&scenic, None).expect("index");
        let score = score_edge_scenery(47.0, -116.0, 47.009, -116.0, Some(&index));
        assert_eq!(score, 0);
    }

    #[test]
    fn forest_and_protected_area_raise_edge_score() {
        let scenic = vec![
            ScenicFeature {
                lat_e7: 470450000,
                lon_e7: -1160000000,
                kind: ScenicKind::Forest,
                osm_id: 10,
            },
            ScenicFeature {
                lat_e7: 470500000,
                lon_e7: -1160000000,
                kind: ScenicKind::ProtectedArea,
                osm_id: 11,
            },
        ];
        let index = build_scenic_index(&scenic, None).expect("index");
        let score = score_edge_scenery(47.0, -116.0, 47.009, -116.0, Some(&index));
        assert!(score > 0, "forest/protected features should influence nearby edges");
    }

    #[test]
    fn glacier_scores_higher_than_stream_at_same_distance() {
        let glacier = score_edge_scenery(
            47.0,
            -116.0,
            47.009,
            -116.0,
            Some(&build_scenic_index(&[ScenicFeature {
                lat_e7: 470500000,
                lon_e7: -1160000000,
                kind: ScenicKind::Glacier,
                osm_id: 1,
            }], None).expect("glacier index")),
        );
        let stream = score_edge_scenery(
            47.0,
            -116.0,
            47.009,
            -116.0,
            Some(&build_scenic_index(&[ScenicFeature {
                lat_e7: 470500000,
                lon_e7: -1160000000,
                kind: ScenicKind::Stream,
                osm_id: 2,
            }], None).expect("stream index")),
        );
        assert!(glacier > stream);
    }
}
