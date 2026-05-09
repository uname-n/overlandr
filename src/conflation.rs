//! Spatial conflation: drop OSM ways that are already covered by USFS geometry.
//!
//! For each OSM track/path way, we sample the midpoint of every edge and check
//! whether it falls within `radius_m` of any USFS road node.  If at least
//! `min_coverage` fraction of edge midpoints are covered, the OSM way is
//! considered a duplicate and dropped.  Ways with insufficient coverage are
//! kept — they represent roads that USFS doesn't have (state forests, BLM,
//! private timber, etc.).
//!
//! # Spatial index
//!
//! We bucket all USFS nodes into a grid of ~111 m cells.  Each query checks
//! a 3×3 neighbourhood of cells; a cheap rectangular pre-filter discards most
//! candidates before the more expensive Haversine check runs.

use std::collections::HashMap;

use crate::geom::{haversine_m, METRES_PER_DEGREE};
use crate::osm::loader::{NodeMap, Way};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Filter `osm_ways`, removing any low-class road whose geometry is already
/// represented in the USFS dataset.
///
/// Only `highway` values in [`TRACK_CLASSES`] are eligible for removal; all
/// other highway classes are kept unconditionally.
///
/// `radius_m` — maximum distance (metres) between an OSM edge midpoint and a
/// USFS node for the edge to be considered "covered" (default: 50 m).
///
/// `min_coverage` — fraction of edges that must be covered before the whole
/// way is dropped (default: 0.5).
pub fn filter_covered_by_usfs(
    osm_ways: Vec<Way>,
    osm_nodes: &NodeMap,
    usfs_ways: &[Way],
    usfs_nodes: &NodeMap,
    radius_m: f64,
    min_coverage: f64,
    grid_step: f64,
) -> Vec<Way> {
    let grid = build_grid(usfs_ways, usfs_nodes, grid_step);

    let mut kept = 0usize;
    let mut dropped = 0usize;

    let result = osm_ways
        .into_iter()
        .filter(|way| {
            let hw = way.tags.get("highway").map(String::as_str).unwrap_or("");
            if !TRACK_CLASSES.contains(&hw) {
                kept += 1;
                return true;
            }
            let cov = edge_coverage(way, osm_nodes, &grid, radius_m, grid_step);
            if cov >= min_coverage {
                dropped += 1;
                false
            } else {
                kept += 1;
                true
            }
        })
        .collect();

    tracing::info!(kept, dropped, "conflation: OSM ways dropped (covered by USFS)");
    result
}

// ---------------------------------------------------------------------------
// Highway classes eligible for conflation
// ---------------------------------------------------------------------------

const TRACK_CLASSES: &[&str] = &["track", "path", "bridleway", "cycleway", "footway"];

// ---------------------------------------------------------------------------
// Spatial grid over USFS nodes
// ---------------------------------------------------------------------------

/// (lat_cell, lon_cell) → list of (lat, lon) coordinates of USFS nodes.
type Grid = HashMap<(i64, i64), Vec<(f64, f64)>>;

fn cell(lat: f64, lon: f64, step: f64) -> (i64, i64) {
    ((lat / step).floor() as i64, (lon / step).floor() as i64)
}

fn build_grid(ways: &[Way], nodes: &NodeMap, grid_step: f64) -> Grid {
    let mut grid: Grid = HashMap::new();
    for way in ways {
        for &id in &way.nodes {
            if let Some(&(lat, lon)) = nodes.get(&id) {
                grid.entry(cell(lat, lon, grid_step)).or_default().push((lat, lon));
            }
        }
    }
    grid
}

// ---------------------------------------------------------------------------
// Coverage computation
// ---------------------------------------------------------------------------

/// Return the fraction of edges in `way` whose midpoint is within `radius_m`
/// of any USFS node.
fn edge_coverage(way: &Way, nodes: &NodeMap, grid: &Grid, radius_m: f64, grid_step: f64) -> f64 {
    if way.nodes.len() < 2 {
        return 0.0;
    }

    let mut total = 0usize;
    let mut covered = 0usize;

    for pair in way.nodes.windows(2) {
        let Some(&(a_lat, a_lon)) = nodes.get(&pair[0]) else { continue };
        let Some(&(b_lat, b_lon)) = nodes.get(&pair[1]) else { continue };

        let mid_lat = (a_lat + b_lat) * 0.5;
        let mid_lon = (a_lon + b_lon) * 0.5;

        total += 1;
        if within_radius(grid, mid_lat, mid_lon, radius_m, grid_step) {
            covered += 1;
        }
    }

    if total == 0 { 0.0 } else { covered as f64 / total as f64 }
}

/// Return `true` if any USFS node is within `radius_m` metres of `(lat, lon)`.
fn within_radius(grid: &Grid, lat: f64, lon: f64, radius_m: f64, grid_step: f64) -> bool {
    // Degree-space bounds for quick rectangular pre-filter.
    let dlat_max = radius_m / METRES_PER_DEGREE;
    let dlon_max = radius_m / (METRES_PER_DEGREE * (lat.to_radians().cos()).max(0.1));

    let (ci, cj) = cell(lat, lon, grid_step);
    for di in -1i64..=1 {
        for dj in -1i64..=1 {
            let Some(pts) = grid.get(&(ci + di, cj + dj)) else { continue };
            for &(nlat, nlon) in pts {
                // Cheap rectangular pre-filter before calling haversine.
                if (nlat - lat).abs() > dlat_max || (nlon - lon).abs() > dlon_max {
                    continue;
                }
                if haversine_m(lat, lon, nlat, nlon) <= radius_m {
                    return true;
                }
            }
        }
    }
    false
}
