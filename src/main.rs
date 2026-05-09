mod cli;
mod conflation;
mod geom;
mod gpx;
mod graph;
mod osm;
mod profile;
mod routing;
mod serve;
mod usfs;

use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;

use cli::{Cli, Command};
use graph::{BBox, BuildOpts, EdgeId, SpatialIndex};

const BLOCKED_COST: f32 = 9999.0;
const CONFLATION_RADIUS_M: f64 = 50.0;
const CONFLATION_MIN_COVERAGE: f64 = 0.5;
use graph::cache;
use graph::EdgeFlags;
use osm::filter::WayFilter;
use osm::loader::load_ways;
use profile::load_profile;
use routing::alternatives::{k_alternatives, AltConfig};
use routing::dijkstra::Route;
use routing::fuel::{plan_fuel_stops, FuelStop};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialise tracing from --log level.
    let level: tracing::Level = cli.log.parse().unwrap_or(tracing::Level::INFO);
    tracing_subscriber::fmt().with_max_level(level).init();

    // Apply optional thread-pool size.
    if let Some(n) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .ok(); // ignore error if pool already built
    }

    match cli.command {
        Command::Build {
            pbf,
            out,
            profile,
            bbox,
            keep_private,
            no_simplify,
            usfs,
            usfs_snap,
        } => cmd_build(
            pbf,
            out,
            profile,
            bbox,
            keep_private,
            no_simplify,
            usfs,
            usfs_snap,
        ),
        Command::Serve { port, host } => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(serve::run_server(&cli.graph, &host, port)),
        Command::Inspect => cmd_inspect(&cli.graph),
        Command::Tags { profile } => cmd_tags(profile),
    }
}

// ---------------------------------------------------------------------------
// cmd_build
// ---------------------------------------------------------------------------

fn cmd_build(
    pbf: PathBuf,
    out: PathBuf,
    profile_path: Option<PathBuf>,
    bbox_str: Option<String>,
    keep_private: bool,
    no_simplify: bool,
    usfs_path: Option<PathBuf>,
    usfs_snap: f32,
) -> anyhow::Result<()> {
    // Parse optional bounding box.
    let bbox: Option<BBox> = bbox_str
        .as_deref()
        .map(parse_bbox)
        .transpose()
        .context("invalid --bbox format; expected \"minlon,minlat,maxlon,maxlat\"")?;

    // Load profile.
    let profile = load_profile(profile_path.as_deref())
        .map_err(|e| anyhow::anyhow!("failed to load profile: {}", e))?;

    // Load ways from PBF.
    let filter = WayFilter::from_profile(&profile, keep_private, true, false);
    let (mut ways, mut nodes, fuel_stations, scenic_features) =
        load_ways(&pbf, &filter).context("failed to read PBF")?;

    tracing::info!(ways = ways.len(), nodes = nodes.len(), "OSM data loaded");
    tracing::info!(
        fuel_stations = fuel_stations.len(),
        scenic_features = scenic_features.len(),
        "point features loaded"
    );

    // Optionally load and merge USFS road data.
    if let Some(ref shp) = usfs_path {
        // Derive clip bbox from the OSM node extents so the national USFS
        // shapefile is automatically trimmed to the PBF's coverage area.
        // A user-supplied --bbox takes precedence if provided.
        let derived_bbox;
        let usfs_bbox: Option<&BBox> = if bbox.is_some() {
            bbox.as_ref()
        } else {
            derived_bbox = pbf_bbox(&nodes);
            Some(&derived_bbox)
        };
        let (usfs_ways, usfs_nodes) = usfs::load_usfs_roads(shp, &nodes, usfs_bbox, usfs_snap)
            .context("failed to load USFS shapefile")?;
        tracing::info!(
            ways = usfs_ways.len(),
            nodes = usfs_nodes.len(),
            snap_m = usfs_snap,
            "USFS roads loaded"
        );
        // Drop OSM tracks whose geometry is already covered by USFS.
        ways = conflation::filter_covered_by_usfs(
            ways,
            &nodes,
            &usfs_ways,
            &usfs_nodes,
            CONFLATION_RADIUS_M,
            CONFLATION_MIN_COVERAGE,
            profile.routing.grid_step,
        );
        ways.extend(usfs_ways);
        nodes.extend(usfs_nodes);
    }

    // Build graph.
    let opts = BuildOpts { keep_private, bbox };
    let mut g = graph::builder::build_graph(&ways, &nodes, &scenic_features, &profile, &opts);
    g.fuel_stations = fuel_stations;

    tracing::info!(nodes = g.node_count, edges = g.edge_count, "graph built");

    // Optionally contract degree-2 chains.
    if !no_simplify {
        g = graph::contract::contract(g);
        tracing::info!(
            nodes = g.node_count,
            edges = g.edge_count,
            "graph contracted"
        );
    }

    // Build spatial index.
    let index = SpatialIndex::build(&g);

    // Save to disk.
    cache::save(&g, &index, profile.fingerprint(), None, &out)
        .with_context(|| format!("failed to write graph to {:?}", out))?;

    println!("Build complete:");
    println!("  nodes : {}", g.node_count);
    println!("  edges : {}", g.edge_count);
    println!("  output: {:?}", out);

    Ok(())
}

// ---------------------------------------------------------------------------
// RouteParams — parameters for a single routing request
// ---------------------------------------------------------------------------

/// All parameters needed to execute a route search.
///
/// Decouples the routing core from both the CLI and the HTTP handler.
pub struct RouteParams {
    pub from_lat: f64,
    pub from_lon: f64,
    pub to_lat: f64,
    pub to_lon: f64,
    pub alternatives: usize,
    pub diversity: f32,
    pub max_detour: f32,
    pub avoid_paved: bool,
    pub avoid_fords: bool,
    pub prefer_scenic: bool,
    pub scenic_weight: f32,
    pub tank_range: Option<f32>,
    pub lambda: f32,
    pub fuel_buffer: f32,
    pub vehicle: String,
}

// ---------------------------------------------------------------------------
// run_route — routing core called by the HTTP serve handler
// ---------------------------------------------------------------------------

pub(crate) fn run_route(
    g: &graph::Graph,
    index: &SpatialIndex,
    profile: &profile::Profile,
    params: &RouteParams,
) -> anyhow::Result<(Vec<Route>, Vec<Vec<FuelStop>>)> {
    let from_node = index
        .nearest(params.from_lat, params.from_lon, 1000.0)
        .map_err(|e| {
            anyhow::anyhow!(
                "origin snap failed ({}); move the point closer to a mapped road",
                e
            )
        })?;
    let to_node = index
        .nearest(params.to_lat, params.to_lon, 1000.0)
        .map_err(|e| {
            anyhow::anyhow!(
                "destination snap failed ({}); move the point closer to a mapped road",
                e
            )
        })?;

    // Look up vehicle profile; fall back to high-clearance if unknown.
    let vp = profile
        .vehicle
        .get(&params.vehicle)
        .or_else(|| profile.vehicle.get("high-clearance"));

    let initial_penalties = build_initial_penalties(g, params, vp);

    let cfg = AltConfig {
        lambda: params.lambda,
        max_retries: 4,
        min_jaccard_distance: params.diversity,
        max_detour: params.max_detour,
        initial_penalties,
    };

    let routes = k_alternatives(g, from_node, to_node, params.alternatives, &cfg);
    if routes.is_empty() {
        anyhow::bail!("no path found between the two coordinates");
    }

    let fuel_stops: Vec<Vec<FuelStop>> = if let Some(range_km) = params.tank_range {
        if g.fuel_stations.is_empty() {
            tracing::warn!("no fuel stations in graph; rebuild with current OSM data");
            routes.iter().map(|_| vec![]).collect()
        } else {
            routes
                .iter()
                .map(|r| plan_fuel_stops(r, g, range_km, params.fuel_buffer))
                .collect()
        }
    } else {
        routes.iter().map(|_| vec![]).collect()
    };

    Ok((routes, fuel_stops))
}

// ---------------------------------------------------------------------------
// cmd_inspect
// ---------------------------------------------------------------------------

fn cmd_inspect(graph_path: &Path) -> anyhow::Result<()> {
    let (g, _index, fingerprint, timestamp) = cache::load(graph_path)
        .with_context(|| format!("failed to load graph from {:?}", graph_path))?;

    println!("Graph cache: {:?}", graph_path);
    println!("  nodes              : {}", g.node_count);
    println!("  edges              : {}", g.edge_count);
    println!(
        "  profile fingerprint: {}",
        fingerprint
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>()
    );
    match timestamp {
        Some(ts) => println!(
            "  PBF timestamp      : {}",
            ts.format("%Y-%m-%d %H:%M:%S UTC")
        ),
        None => println!("  PBF timestamp      : (none)"),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_tags
// ---------------------------------------------------------------------------

fn cmd_tags(profile_path: Option<PathBuf>) -> anyhow::Result<()> {
    let profile = load_profile(profile_path.as_deref())
        .map_err(|e| anyhow::anyhow!("failed to load profile: {}", e))?;

    println!("=== Base (highway) factors ===");
    println!("{:<20} {:>8}", "highway", "factor");
    println!("{}", "-".repeat(30));
    for (k, v) in &profile.base.0 {
        println!("{:<20} {:>8.3}", k, v);
    }

    println!();
    println!("=== Surface factors ===");
    println!("{:<20} {:>8}", "surface", "factor");
    println!("{}", "-".repeat(30));
    for (k, v) in &profile.surface.0 {
        println!("{:<20} {:>8.3}", k, v);
    }

    println!();
    println!("=== Tracktype factors ===");
    println!("{:<20} {:>8}", "tracktype", "factor");
    println!("{}", "-".repeat(30));
    for (k, v) in &profile.tracktype.0 {
        println!("{:<20} {:>8.3}", k, v);
    }

    println!();
    println!("=== Smoothness factors ===");
    println!("{:<20} {:>8}", "smoothness", "factor");
    println!("{}", "-".repeat(30));
    for (k, v) in &profile.smoothness.0 {
        println!("{:<20} {:>8.3}", k, v);
    }

    println!();
    println!("=== Vehicle profiles ===");
    for (name, vp) in &profile.vehicle {
        println!("  [{}]", name);
        println!("    fourwd_only_penalty : {}", vp.fourwd_only_penalty);
        println!("    min_smoothness      : {}", vp.min_smoothness);
        if let Some(bonus) = vp.narrow_path_bonus {
            println!("    narrow_path_bonus   : {}", bonus);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute the bounding box of all nodes in a NodeMap.
fn pbf_bbox(nodes: &osm::loader::NodeMap) -> BBox {
    let mut min_lat = f64::INFINITY;
    let mut max_lat = f64::NEG_INFINITY;
    let mut min_lon = f64::INFINITY;
    let mut max_lon = f64::NEG_INFINITY;
    for &(lat, lon) in nodes.values() {
        if lat < min_lat {
            min_lat = lat;
        }
        if lat > max_lat {
            max_lat = lat;
        }
        if lon < min_lon {
            min_lon = lon;
        }
        if lon > max_lon {
            max_lon = lon;
        }
    }
    BBox {
        min_lon,
        min_lat,
        max_lon,
        max_lat,
    }
}

/// Parse "minlon,minlat,maxlon,maxlat" into a `BBox`.
fn scenic_multiplier(edge: &graph::EdgeData, params: &RouteParams) -> f32 {
    if !params.prefer_scenic || edge.scenic_score == 0 {
        return 1.0;
    }
    let normalized = edge.scenic_score as f32 / 255.0;
    (1.0 - params.scenic_weight * 0.35 * normalized).clamp(0.65, 1.0)
}

fn build_initial_penalties(
    g: &graph::Graph,
    params: &RouteParams,
    vp: Option<&profile::VehicleProfile>,
) -> std::collections::HashMap<EdgeId, f32> {
    let mut initial_penalties: std::collections::HashMap<EdgeId, f32> =
        std::collections::HashMap::new();
    for (eid, edge) in g.edges.iter().enumerate() {
        let scenic = scenic_multiplier(edge, params);
        if scenic < 1.0 {
            initial_penalties.insert(eid as EdgeId, scenic);
        }
        if params.avoid_paved && edge.flags.contains(EdgeFlags::PAVED) {
            let p = initial_penalties.entry(eid as EdgeId).or_insert(1.0);
            *p = p.max(10.0);
        }
        if params.avoid_fords && edge.flags.contains(EdgeFlags::FORD) {
            let p = initial_penalties.entry(eid as EdgeId).or_insert(1.0);
            *p = p.max(20.0);
        }
        if params.vehicle != "dirtbike" && edge.flags.contains(EdgeFlags::TRAIL) {
            let p = initial_penalties.entry(eid as EdgeId).or_insert(1.0);
            *p = p.max(BLOCKED_COST);
        }
        if let Some(vp) = vp {
            if edge.flags.contains(EdgeFlags::FOURWD_ONLY) {
                let p = initial_penalties.entry(eid as EdgeId).or_insert(1.0);
                *p = p.max(vp.fourwd_only_penalty);
            }
            let smoothness_penalty: f32 = match vp.min_smoothness.as_str() {
                "bad" if edge.flags.contains(EdgeFlags::SMOOTHNESS_ROUGH) => BLOCKED_COST,
                "very_bad" if edge.flags.contains(EdgeFlags::SMOOTHNESS_VERY_ROUGH) => BLOCKED_COST,
                _ => 1.0,
            };
            if smoothness_penalty > 1.0 {
                let p = initial_penalties.entry(eid as EdgeId).or_insert(1.0);
                *p = p.max(smoothness_penalty);
            }
        }
    }
    initial_penalties
}

fn parse_bbox(s: &str) -> anyhow::Result<BBox> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        anyhow::bail!("bbox must have exactly 4 comma-separated values");
    }
    let vals: Vec<f64> = parts
        .iter()
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .context("bbox values must be valid floating-point numbers")?;
    Ok(BBox {
        min_lon: vals[0],
        min_lat: vals[1],
        max_lon: vals[2],
        max_lat: vals[3],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeData, Graph, NodeData};
    use crate::routing::dijkstra::dijkstra;

    fn params() -> RouteParams {
        RouteParams {
            from_lat: 0.0,
            from_lon: 0.0,
            to_lat: 0.0,
            to_lon: 0.0,
            alternatives: 1,
            diversity: 0.35,
            max_detour: 1.6,
            avoid_paved: false,
            avoid_fords: false,
            prefer_scenic: true,
            scenic_weight: 0.5,
            tank_range: None,
            lambda: 1.5,
            fuel_buffer: 0.2,
            vehicle: "4x4".to_string(),
        }
    }

    #[test]
    fn scenic_multiplier_rewards_scored_edges() {
        let edge = EdgeData {
            cost: 1.0,
            length_m: 100.0,
            flags: EdgeFlags::default(),
            scenic_score: 255,
            polyline: vec![],
        };
        let p = scenic_multiplier(&edge, &params());
        assert!(p < 1.0);
        assert!(p >= 0.65);
    }

    #[test]
    fn scenic_penalty_does_not_override_harder_penalties() {
        let graph = Graph {
            nodes: vec![NodeData {
                lat_e7: 0,
                lon_e7: 0,
            }],
            offsets: vec![0, 0],
            neighbors: vec![],
            edges: vec![EdgeData {
                cost: 1.0,
                length_m: 100.0,
                flags: EdgeFlags::PAVED,
                scenic_score: 255,
                polyline: vec![],
            }],
            node_count: 1,
            edge_count: 1,
            fuel_stations: vec![],
        };
        let mut route_params = params();
        route_params.avoid_paved = true;
        let penalties = build_initial_penalties(&graph, &route_params, None);
        assert_eq!(penalties.get(&0), Some(&10.0));
    }

    #[test]
    fn scenic_preference_can_flip_route_choice() {
        let graph = Graph {
            nodes: vec![
                NodeData {
                    lat_e7: 0,
                    lon_e7: 0,
                },
                NodeData {
                    lat_e7: 0,
                    lon_e7: 1,
                },
                NodeData {
                    lat_e7: 0,
                    lon_e7: 2,
                },
                NodeData {
                    lat_e7: 0,
                    lon_e7: 3,
                },
            ],
            offsets: vec![0, 2, 3, 4, 4],
            neighbors: vec![(1, 0), (2, 2), (3, 1), (3, 3)],
            edges: vec![
                EdgeData {
                    cost: 2.0,
                    length_m: 100.0,
                    flags: EdgeFlags::default(),
                    scenic_score: 255,
                    polyline: vec![],
                },
                EdgeData {
                    cost: 2.0,
                    length_m: 100.0,
                    flags: EdgeFlags::default(),
                    scenic_score: 255,
                    polyline: vec![],
                },
                EdgeData {
                    cost: 1.7,
                    length_m: 100.0,
                    flags: EdgeFlags::default(),
                    scenic_score: 0,
                    polyline: vec![],
                },
                EdgeData {
                    cost: 1.7,
                    length_m: 100.0,
                    flags: EdgeFlags::default(),
                    scenic_score: 0,
                    polyline: vec![],
                },
            ],
            node_count: 4,
            edge_count: 4,
            fuel_stations: vec![],
        };

        let plain = dijkstra(&graph, 0, 3, &std::collections::HashMap::new()).expect("plain route");
        assert_eq!(plain.edges, vec![2, 3]);

        let mut scenic_params = params();
        scenic_params.scenic_weight = 1.0;
        let penalties = build_initial_penalties(&graph, &scenic_params, None);
        let scenic = dijkstra(&graph, 0, 3, &penalties).expect("scenic route");
        assert_eq!(scenic.edges, vec![0, 1]);
    }

    #[test]
    fn non_dirtbike_vehicle_blocks_trail_edges() {
        let graph = Graph {
            nodes: vec![NodeData {
                lat_e7: 0,
                lon_e7: 0,
            }],
            offsets: vec![0, 0],
            neighbors: vec![],
            edges: vec![EdgeData {
                cost: 1.0,
                length_m: 100.0,
                flags: EdgeFlags::TRAIL,
                scenic_score: 0,
                polyline: vec![],
            }],
            node_count: 1,
            edge_count: 1,
            fuel_stations: vec![],
        };

        let penalties = build_initial_penalties(&graph, &params(), None);
        assert_eq!(penalties.get(&0), Some(&BLOCKED_COST));
    }

    #[test]
    fn dirtbike_vehicle_keeps_trail_edges_available() {
        let graph = Graph {
            nodes: vec![NodeData {
                lat_e7: 0,
                lon_e7: 0,
            }],
            offsets: vec![0, 0],
            neighbors: vec![],
            edges: vec![EdgeData {
                cost: 1.0,
                length_m: 100.0,
                flags: EdgeFlags::TRAIL,
                scenic_score: 0,
                polyline: vec![],
            }],
            node_count: 1,
            edge_count: 1,
            fuel_stations: vec![],
        };

        let mut route_params = params();
        route_params.vehicle = "dirtbike".to_string();
        let penalties = build_initial_penalties(&graph, &route_params, None);
        assert!(penalties.get(&0).is_none());
    }
}
