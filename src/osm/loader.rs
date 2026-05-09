//! Streaming PBF loader with two-pass node collection.
//!
//! Pass 1: reads all ways, applies [`WayFilter`], collects the set of node IDs
//! referenced by surviving ways.
//!
//! Pass 2: reads all node blocks (dense and regular), stores coordinates only
//! for the node IDs gathered in pass 1.
//!
//! Both passes use `ElementReader::par_map_reduce` for parallel block decode
//! via rayon, keeping RAM usage low by avoiding loading the whole node table.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use indicatif::{ProgressBar, ProgressStyle};
use osmpbf::{Element, ElementReader};
use tracing::info;

use crate::graph::{FuelStation, ScenicFeature, ScenicKind};
use crate::osm::filter::WayFilter;
use crate::osm::tags::collect_tags;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single OSM way after filtering.
#[derive(Debug, Clone)]
pub struct Way {
    #[allow(dead_code)]
    pub id: i64,
    /// Ordered list of node IDs forming the way geometry.
    pub nodes: Vec<i64>,
    /// All tags present on the way.
    pub tags: HashMap<String, String>,
}

/// A map from OSM node ID to (latitude, longitude) in decimal degrees.
pub type NodeMap = HashMap<i64, (f64, f64)>;

#[derive(Debug, Clone)]
struct ScenicWayCandidate {
    id: i64,
    kind: ScenicKind,
    nodes: Vec<i64>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load ways from a `.osm.pbf` file, returning filtered ways, the
/// coordinates of all nodes they reference, and any fuel stations found.
///
/// # Arguments
/// * `path`    – path to the `.osm.pbf` extract.
/// * `filter`  – controls which ways survive (highway type + access rules).
/// * `profile` – vehicle profile (reserved for future costing integration).
///
/// # Errors
/// Returns an error if the file cannot be opened or if the PBF data is corrupt.
pub fn load_ways(
    path: &Path,
    filter: &WayFilter,
) -> anyhow::Result<(Vec<Way>, NodeMap, Vec<FuelStation>, Vec<ScenicFeature>)> {
    // ------------------------------------------------------------------
    // Pass 1 — collect surviving ways and the set of node IDs they use.
    // ------------------------------------------------------------------
    let pb1 = make_spinner("Pass 1/2: reading ways…");

    let reader1 = ElementReader::from_path(path)
        .map_err(|e| anyhow::anyhow!("failed to open PBF: {}", e))?;

    // par_map_reduce: each rayon thread processes one PBF blob independently,
    // building thread-local (ways, node_ids) accumulators that are then merged.
    let (ways, scenic_way_candidates, needed_nodes) = reader1
        .par_map_reduce(
            // map: process one Element per call, returning local accumulators.
            |element| {
                let mut local_ways: Vec<Way> = Vec::new();
                let mut local_scenic_ways: Vec<ScenicWayCandidate> = Vec::new();
                let mut local_nodes: HashSet<i64> = HashSet::new();

                if let Element::Way(w) = element {
                    let tags = collect_tags(w.tags());
                    let refs: Vec<i64> = w.refs().collect();
                    if filter.keep(&tags) {
                        local_nodes.extend(refs.iter().copied());
                        local_ways.push(Way {
                            id: w.id(),
                            nodes: refs.clone(),
                            tags: tags.clone(),
                        });
                    }
                    if let Some(kind) = classify_scenic_way(&tags) {
                        local_nodes.extend(refs.iter().copied());
                        local_scenic_ways.push(ScenicWayCandidate {
                            id: w.id(),
                            kind,
                            nodes: refs,
                        });
                    }
                }
                (local_ways, local_scenic_ways, local_nodes)
            },
            // identity: empty accumulators for the reduction tree.
            || (Vec::<Way>::new(), Vec::<ScenicWayCandidate>::new(), HashSet::<i64>::new()),
            // reduce: merge two accumulators.
            |(mut wa, mut sa, mut na), (wb, sb, nb)| {
                wa.extend(wb);
                sa.extend(sb);
                na.extend(nb);
                (wa, sa, na)
            },
        )
        .map_err(|e| anyhow::anyhow!("PBF read error (pass 1): {}", e))?;

    pb1.finish_with_message("pass 1 done");

    info!(
        ways_total = ways.len(),
        scenic_way_candidates = scenic_way_candidates.len(),
        nodes_referenced = needed_nodes.len(),
        "pass 1 complete"
    );

    // ------------------------------------------------------------------
    // Pass 2 — read node blocks, collect coordinates for needed nodes.
    // ------------------------------------------------------------------
    let pb2 = make_spinner("Pass 2/2: reading nodes…");

    let reader2 = ElementReader::from_path(path)
        .map_err(|e| anyhow::anyhow!("failed to open PBF for pass 2: {}", e))?;

    let (node_map, fuel_stations, mut scenic_features): (NodeMap, Vec<FuelStation>, Vec<ScenicFeature>) = reader2
        .par_map_reduce(
            |element| {
                let mut local_map: NodeMap = HashMap::new();
                let mut local_fuel: Vec<FuelStation> = Vec::new();
                let mut local_scenic: Vec<ScenicFeature> = Vec::new();
                match element {
                    Element::Node(n) => {
                        if needed_nodes.contains(&n.id()) {
                            local_map.insert(n.id(), (n.lat(), n.lon()));
                        }
                        let tags = collect_tags(n.tags());
                        if tags.get("amenity").map(String::as_str) == Some("fuel") {
                            local_fuel.push(FuelStation {
                                lat_e7: (n.lat() * 1e7) as i32,
                                lon_e7: (n.lon() * 1e7) as i32,
                                osm_id: n.id(),
                            });
                        }
                        if let Some(kind) = classify_scenic_node(&tags) {
                            local_scenic.push(ScenicFeature {
                                lat_e7: (n.lat() * 1e7) as i32,
                                lon_e7: (n.lon() * 1e7) as i32,
                                kind,
                                osm_id: n.id(),
                            });
                        }
                    }
                    Element::DenseNode(n) => {
                        if needed_nodes.contains(&n.id()) {
                            local_map.insert(n.id(), (n.lat(), n.lon()));
                        }
                        let tags = collect_tags(n.tags());
                        if tags.get("amenity").map(String::as_str) == Some("fuel") {
                            local_fuel.push(FuelStation {
                                lat_e7: (n.lat() * 1e7) as i32,
                                lon_e7: (n.lon() * 1e7) as i32,
                                osm_id: n.id(),
                            });
                        }
                        if let Some(kind) = classify_scenic_node(&tags) {
                            local_scenic.push(ScenicFeature {
                                lat_e7: (n.lat() * 1e7) as i32,
                                lon_e7: (n.lon() * 1e7) as i32,
                                kind,
                                osm_id: n.id(),
                            });
                        }
                    }
                    _ => {}
                }
                (local_map, local_fuel, local_scenic)
            },
            || (HashMap::new(), Vec::new(), Vec::new()),
            |(mut ma, mut fa, mut sa), (mb, fb, sb)| {
                ma.extend(mb);
                fa.extend(fb);
                sa.extend(sb);
                (ma, fa, sa)
            },
        )
        .map_err(|e| anyhow::anyhow!("PBF read error (pass 2): {}", e))?;

    pb2.finish_with_message("pass 2 done");

    info!(
        nodes_loaded = node_map.len(),
        fuel_stations_found = fuel_stations.len(),
        scenic_node_features_found = scenic_features.len(),
        "pass 2 complete"
    );

    scenic_features.extend(
        scenic_way_candidates
            .iter()
            .filter_map(|way| scenic_feature_from_way(way, &node_map))
    );

    info!(scenic_features = scenic_features.len(), "scenic features extracted");

    Ok((ways, node_map, fuel_stations, scenic_features))
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn classify_scenic_node(tags: &HashMap<String, String>) -> Option<ScenicKind> {
    classify_scenic_tags(tags, false)
}

fn classify_scenic_way(tags: &HashMap<String, String>) -> Option<ScenicKind> {
    classify_scenic_tags(tags, true)
}

fn classify_scenic_tags(tags: &HashMap<String, String>, allow_landscape_areas: bool) -> Option<ScenicKind> {
    match (
        tags.get("tourism").map(String::as_str),
        tags.get("natural").map(String::as_str),
    ) {
        (Some("viewpoint"), _) => return Some(ScenicKind::Viewpoint),
        (_, Some("peak")) => return Some(ScenicKind::Peak),
        (_, Some("saddle")) => return Some(ScenicKind::Saddle),
        (_, Some("water")) => return Some(ScenicKind::Water),
        (_, Some("glacier")) => return Some(ScenicKind::Glacier),
        (_, Some("cliff")) => return Some(ScenicKind::Cliff),
        (_, Some("wood")) if allow_landscape_areas => return Some(ScenicKind::Forest),
        _ => {}
    }

    match tags.get("waterway").map(String::as_str) {
        Some("river") => return Some(ScenicKind::River),
        Some("stream") => return Some(ScenicKind::Stream),
        _ => {}
    }

    if allow_landscape_areas {
        if tags.get("landuse").map(String::as_str) == Some("forest") {
            return Some(ScenicKind::Forest);
        }
        if tags.get("boundary").map(String::as_str) == Some("protected_area") {
            return Some(ScenicKind::ProtectedArea);
        }
        if tags.get("leisure").map(String::as_str) == Some("nature_reserve") {
            return Some(ScenicKind::NatureReserve);
        }
    }

    None
}

fn scenic_feature_from_way(way: &ScenicWayCandidate, node_map: &NodeMap) -> Option<ScenicFeature> {
    let mut count = 0i64;
    let mut lat_sum = 0.0f64;
    let mut lon_sum = 0.0f64;

    for node_id in &way.nodes {
        let Some(&(lat, lon)) = node_map.get(node_id) else { continue; };
        lat_sum += lat;
        lon_sum += lon;
        count += 1;
    }

    if count == 0 {
        return None;
    }

    Some(ScenicFeature {
        lat_e7: ((lat_sum / count as f64) * 1e7) as i32,
        lon_e7: ((lon_sum / count as f64) * 1e7) as i32,
        kind: way.kind,
        osm_id: -way.id,
    })
}

fn make_spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg} [{elapsed_precise}]")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg.to_owned());
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn classifies_scenic_nodes() {
        assert_eq!(classify_scenic_node(&tags(&[("tourism", "viewpoint")])), Some(ScenicKind::Viewpoint));
        assert_eq!(classify_scenic_node(&tags(&[("natural", "peak")])), Some(ScenicKind::Peak));
        assert_eq!(classify_scenic_node(&tags(&[("natural", "saddle")])), Some(ScenicKind::Saddle));
        assert_eq!(classify_scenic_node(&tags(&[("natural", "water")])), Some(ScenicKind::Water));
        assert_eq!(classify_scenic_node(&tags(&[("natural", "glacier")])), Some(ScenicKind::Glacier));
        assert_eq!(classify_scenic_node(&tags(&[("natural", "cliff")])), Some(ScenicKind::Cliff));
        assert_eq!(classify_scenic_node(&tags(&[("landuse", "forest")])), None);
    }

    #[test]
    fn classifies_scenic_ways() {
        assert_eq!(classify_scenic_way(&tags(&[("natural", "water")])), Some(ScenicKind::Water));
        assert_eq!(classify_scenic_way(&tags(&[("waterway", "river")])), Some(ScenicKind::River));
        assert_eq!(classify_scenic_way(&tags(&[("waterway", "stream")])), Some(ScenicKind::Stream));
        assert_eq!(classify_scenic_way(&tags(&[("landuse", "forest")])), Some(ScenicKind::Forest));
        assert_eq!(classify_scenic_way(&tags(&[("natural", "wood")])), Some(ScenicKind::Forest));
        assert_eq!(classify_scenic_way(&tags(&[("boundary", "protected_area")])), Some(ScenicKind::ProtectedArea));
        assert_eq!(classify_scenic_way(&tags(&[("leisure", "nature_reserve")])), Some(ScenicKind::NatureReserve));
        assert_eq!(classify_scenic_way(&tags(&[("natural", "glacier")])), Some(ScenicKind::Glacier));
        assert_eq!(classify_scenic_way(&tags(&[("natural", "cliff")])), Some(ScenicKind::Cliff));
        assert_eq!(classify_scenic_way(&tags(&[("highway", "track")])), None);
    }

    #[test]
    fn scenic_way_centroid_uses_available_nodes() {
        let candidate = ScenicWayCandidate {
            id: 42,
            kind: ScenicKind::River,
            nodes: vec![1, 2, 999],
        };
        let mut nodes: NodeMap = HashMap::new();
        nodes.insert(1, (45.0, -122.0));
        nodes.insert(2, (46.0, -124.0));

        let feature = scenic_feature_from_way(&candidate, &nodes).expect("feature");
        assert_eq!(feature.kind, ScenicKind::River);
        assert_eq!(feature.osm_id, -42);
        assert_eq!(feature.lat_e7, 455000000);
        assert_eq!(feature.lon_e7, -1230000000);
    }
}
