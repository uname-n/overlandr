//! Native loader for USFS National Forest System Roads shapefile.
//!
//! Reads the NFS Roads shapefile distributed by the USFS Enterprise Data
//! Warehouse and converts road segments into the same `Way` / `NodeMap` types
//! that the OSM loader produces, so both datasets can be merged before
//! `build_graph`.
//!
//! # Attribute mapping
//!
//! | USFS field    | Value               | OSM tag emitted               |
//! |---------------|---------------------|-------------------------------|
//! | ROUTE_STAT    | not "EX - EXISTING" | (segment skipped)             |
//! | OPER_MAINT    | 0/1                 | highway=track, 4wd_only=yes, smoothness=very_bad |
//! | OPER_MAINT    | 2 (High Clearance)  | highway=track, 4wd_only=yes   |
//! | OPER_MAINT    | 3 (Passenger Car)   | highway=track                 |
//! | OPER_MAINT    | 4 (Passenger Car+)  | highway=unclassified          |
//! | OPER_MAINT    | 5 (High Standard)   | highway=secondary             |
//! | OPENFORUSE    | ADMIN               | access=private                |
//! | OPENFORUSE    | ALL/PUBLIC          | (no access restriction tag)   |
//! | OBJECTIVE_    | D - DECOMMISSION    | access=no                     |
//! | SURFACE_TY    | ASPH/PAVE/CONC/BST  | surface=asphalt               |
//! | SURFACE_TY    | AGG/GRAV/CRUS       | surface=gravel                |

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context;
use indicatif::{ProgressBar, ProgressStyle};
use tracing::info;

use crate::geom::{haversine_m, grid_cell};
use crate::graph::BBox;
use crate::osm::loader::{NodeMap, Way};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load USFS NFS Roads from `path` (`.shp` file) and return ways + nodes
/// ready to be merged with OSM data before `build_graph`.
///
/// `osm_nodes` — the OSM node map already loaded; USFS road endpoints within
/// `snap_m` metres of any OSM node are assigned that OSM node's ID so the two
/// networks connect at entry/exit points of national forests.
///
/// `bbox` — optional bounding box; segments with no vertex inside the box are
/// skipped to avoid loading the full national dataset into memory.
pub fn load_usfs_roads(
    path: &Path,
    osm_nodes: &NodeMap,
    bbox: Option<&BBox>,
    snap_m: f32,
) -> anyhow::Result<(Vec<Way>, NodeMap)> {
    let pb = make_spinner("Loading USFS NFS roads…");

    let grid = build_snap_grid(osm_nodes);

    let mut reader = shapefile::Reader::from_path(path)
        .with_context(|| format!("failed to open shapefile {:?}", path))?;

    let mut ways: Vec<Way> = Vec::new();
    let mut usfs_nodes: NodeMap = HashMap::new();
    // Deduplicate interior vertices by ~0.1 m grid so adjacent USFS segments
    // that share a vertex connect at the same NodeId.
    let mut coord_index: HashMap<(i64, i64), i64> = HashMap::new();
    // Synthetic IDs occupy the negative i64 range so they never collide with
    // positive OSM IDs.
    let mut next_id: i64 = -1i64;
    let mut bbox_clipped: usize = 0;

    for result in reader.iter_shapes_and_records() {
        let (shape, record) = result.context("shapefile read error")?;

        // Only keep existing roads.
        let route_stat = field_str(&record, "ROUTE_STAT").unwrap_or_default();
        if !route_stat.starts_with("EX") {
            continue;
        }

        let oper_maint = field_str(&record, "OPER_MAINT").unwrap_or_default();
        let open_for_use = field_str(&record, "OPENFORUSE").unwrap_or_default();
        let objective = field_str(&record, "OBJECTIVE_").unwrap_or_default();
        let surface_ty = field_str(&record, "SURFACE_TY").unwrap_or_default();
        let name = field_str(&record, "NAME").unwrap_or_default();
        let tags = build_tags(&oper_maint, &open_for_use, &objective, &surface_ty, &name);

        for part in extract_parts(&shape) {
            if part.len() < 2 {
                continue;
            }

            // Skip if no vertex falls inside the requested bbox.
            if let Some(bb) = bbox {
                let any_inside = part.iter().any(|&(lon, lat)| {
                    lat >= bb.min_lat
                        && lat <= bb.max_lat
                        && lon >= bb.min_lon
                        && lon <= bb.max_lon
                });
                if !any_inside {
                    bbox_clipped += 1;
                    continue;
                }
            }

            let way_id = next_id;
            next_id -= 1;

            let last_idx = part.len() - 1;
            let mut node_ids: Vec<i64> = Vec::with_capacity(part.len());

            for (i, &(lon, lat)) in part.iter().enumerate() {
                let is_endpoint = i == 0 || i == last_idx;
                let id = if is_endpoint {
                    // Try to snap to an existing OSM node first.
                    snap_to_osm(&grid, osm_nodes, lat, lon, snap_m as f64)
                        .unwrap_or_else(|| {
                            get_or_create(
                                &mut coord_index,
                                &mut usfs_nodes,
                                &mut next_id,
                                lat,
                                lon,
                            )
                        })
                } else {
                    get_or_create(&mut coord_index, &mut usfs_nodes, &mut next_id, lat, lon)
                };
                node_ids.push(id);
            }

            ways.push(Way { id: way_id, nodes: node_ids, tags: tags.clone() });
        }
    }

    pb.finish_with_message(format!(
        "USFS: {} segments, {} new nodes",
        ways.len(),
        usfs_nodes.len()
    ));
    if bbox_clipped > 0 {
        tracing::warn!(bbox_clipped, "USFS segments skipped (outside bbox)");
    }
    info!(ways = ways.len(), nodes = usfs_nodes.len(), "USFS roads loaded");

    Ok((ways, usfs_nodes))
}

// ---------------------------------------------------------------------------
// Spatial snap helpers
// ---------------------------------------------------------------------------

type SnapGrid = HashMap<(i64, i64), Vec<(i64, f64, f64)>>;

fn build_snap_grid(nodes: &NodeMap) -> SnapGrid {
    let mut grid: SnapGrid = HashMap::new();
    for (&id, &(lat, lon)) in nodes {
        grid.entry(grid_cell(lat, lon)).or_default().push((id, lat, lon));
    }
    grid
}

fn snap_to_osm(
    grid: &SnapGrid,
    _nodes: &NodeMap,
    lat: f64,
    lon: f64,
    snap_m: f64,
) -> Option<i64> {
    let (gi, gj) = grid_cell(lat, lon);
    let mut best: Option<(f64, i64)> = None;

    for di in -1i64..=1 {
        for dj in -1i64..=1 {
            if let Some(candidates) = grid.get(&(gi + di, gj + dj)) {
                for &(id, nlat, nlon) in candidates {
                    let d = haversine_m(lat, lon, nlat, nlon);
                    if d <= snap_m && best.map_or(true, |(bd, _)| d < bd) {
                        best = Some((d, id));
                    }
                }
            }
        }
    }

    best.map(|(_, id)| id)
}

fn get_or_create(
    index: &mut HashMap<(i64, i64), i64>,
    nodes: &mut NodeMap,
    next_id: &mut i64,
    lat: f64,
    lon: f64,
) -> i64 {
    // ~0.1 m precision deduplication key.
    let key = ((lat * 1e6).round() as i64, (lon * 1e6).round() as i64);
    *index.entry(key).or_insert_with(|| {
        let id = *next_id;
        *next_id -= 1;
        nodes.insert(id, (lat, lon));
        id
    })
}

// ---------------------------------------------------------------------------
// Shapefile geometry extraction
// ---------------------------------------------------------------------------

/// Extract all polyline parts as `Vec<(lon, lat)>` regardless of shape type
/// (Polyline, PolylineM, PolylineZ).
fn extract_parts(shape: &shapefile::Shape) -> Vec<Vec<(f64, f64)>> {
    match shape {
        shapefile::Shape::Polyline(p) => p
            .parts()
            .iter()
            .map(|pts| pts.iter().map(|pt| (pt.x, pt.y)).collect())
            .collect(),
        shapefile::Shape::PolylineM(p) => p
            .parts()
            .iter()
            .map(|pts| pts.iter().map(|pt| (pt.x, pt.y)).collect())
            .collect(),
        shapefile::Shape::PolylineZ(p) => p
            .parts()
            .iter()
            .map(|pts| pts.iter().map(|pt| (pt.x, pt.y)).collect())
            .collect(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Attribute helpers
// ---------------------------------------------------------------------------

fn field_str(record: &shapefile::dbase::Record, field: &str) -> Option<String> {
    match record.get(field)? {
        shapefile::dbase::FieldValue::Character(Some(s)) => Some(s.trim().to_string()),
        _ => None,
    }
}

/// Map USFS attributes to OSM-compatible tags understood by `build_graph`.
fn build_tags(
    oper_maint: &str,
    open_for_use: &str,
    objective: &str,
    surface_ty: &str,
    name: &str,
) -> HashMap<String, String> {
    let mut tags: HashMap<String, String> = HashMap::new();

    // OPER_MAINT is formatted "N - DESCRIPTION"; extract the leading digit.
    let level: u32 = oper_maint
        .split(" - ")
        .next()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(3);

    match level {
        0 | 1 => {
            tags.insert("highway".into(), "track".into());
            tags.insert("4wd_only".into(), "yes".into());
            tags.insert("smoothness".into(), "very_bad".into());
        }
        2 => {
            tags.insert("highway".into(), "track".into());
            tags.insert("4wd_only".into(), "yes".into());
        }
        3 => {
            tags.insert("highway".into(), "track".into());
        }
        4 => {
            tags.insert("highway".into(), "unclassified".into());
        }
        _ => {
            // Level 5+ = high-standard road (often paved)
            tags.insert("highway".into(), "secondary".into());
        }
    }

    match open_for_use.trim() {
        "ADMIN" => {
            tags.insert("access".into(), "private".into());
        }
        "ALL" | "PUBLIC" => {}
        _ => {}
    }

    if objective.contains("DECOMMISSION") {
        tags.insert("access".into(), "no".into());
    }

    // SURFACE_TY is formatted "CODE - DESCRIPTION"; extract the code.
    let surf = surface_ty.split(" - ").next().unwrap_or("").trim();
    match surf {
        "ASPH" | "PAVE" | "CONC" | "BST" | "CHIP" | "BITU" => {
            tags.insert("surface".into(), "asphalt".into());
        }
        "AGG" | "GRAV" | "CRUS" => {
            tags.insert("surface".into(), "gravel".into());
        }
        _ => {} // NAT (native material) and unknowns → no surface tag = unpaved
    }

    let name = name.trim();
    if !name.is_empty() {
        tags.insert("name".into(), name.to_string());
    }

    tags
}

// ---------------------------------------------------------------------------
// Progress UI
// ---------------------------------------------------------------------------

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
    use super::build_tags;

    #[test]
    fn oper_maint_1_public_is_kept_as_rough_track() {
        let tags = build_tags(
            "1 - BASIC CUSTODIAL CARE (CLOSED)",
            "ALL",
            "2 - HIGH CLEARANCE VEHICLES",
            "NAT - NATIVE MATERIAL",
            "Summit Pass Rd",
        );

        assert_eq!(tags.get("highway").map(String::as_str), Some("track"));
        assert_eq!(tags.get("4wd_only").map(String::as_str), Some("yes"));
        assert_eq!(tags.get("smoothness").map(String::as_str), Some("very_bad"));
        assert_eq!(tags.get("access").map(String::as_str), None);
        assert_eq!(tags.get("name").map(String::as_str), Some("Summit Pass Rd"));
    }

    #[test]
    fn openforuse_admin_sets_private_access() {
        let tags = build_tags(
            "2 - HIGH CLEARANCE VEHICLES",
            "ADMIN",
            "2 - HIGH CLEARANCE VEHICLES",
            "GRAV - GRAVEL",
            "Admin Spur",
        );

        assert_eq!(tags.get("access").map(String::as_str), Some("private"));
        assert_eq!(tags.get("surface").map(String::as_str), Some("gravel"));
    }

    #[test]
    fn decommission_objective_sets_access_no() {
        let tags = build_tags(
            "1 - BASIC CUSTODIAL CARE (CLOSED)",
            "ALL",
            "D - DECOMMISSION",
            "NAT - NATIVE MATERIAL",
            "Old Road",
        );

        assert_eq!(tags.get("access").map(String::as_str), Some("no"));
    }
}
