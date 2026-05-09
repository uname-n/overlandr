use std::io::Write;

use chrono::Utc;
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};

use crate::graph::Graph;
use crate::routing::dijkstra::Route;
use crate::routing::fuel::FuelStop;
use crate::routing::score::UNPAVED_WEIGHT;

/// Serialize routes to GPX bytes (in-memory).
pub fn write_gpx_to_bytes(routes: &[Route], graph: &Graph, name: &str, fuel_stops: &[Vec<FuelStop>]) -> anyhow::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut w = Writer::new_with_indent(&mut buf, b' ', 2);
    write_gpx_inner(&mut w, routes, graph, name, fuel_stops)?;
    Ok(buf)
}

fn write_gpx_inner<W: Write>(mut w: &mut Writer<W>, routes: &[Route], graph: &Graph, name: &str, fuel_stops: &[Vec<FuelStop>]) -> anyhow::Result<()> {
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;

    let mut gpx_start = BytesStart::new("gpx");
    gpx_start.push_attribute(("version", "1.1"));
    gpx_start.push_attribute(("creator", concat!("overlandr ", env!("CARGO_PKG_VERSION"))));
    gpx_start.push_attribute(("xmlns", "http://www.topografix.com/GPX/1/1"));
    w.write_event(Event::Start(gpx_start))?;

    w.write_event(Event::Start(BytesStart::new("metadata")))?;
    w.write_event(Event::Start(BytesStart::new("name")))?;
    w.write_event(Event::Text(BytesText::new(name)))?;
    w.write_event(Event::End(BytesEnd::new("name")))?;
    let ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    w.write_event(Event::Start(BytesStart::new("time")))?;
    w.write_event(Event::Text(BytesText::new(&ts)))?;
    w.write_event(Event::End(BytesEnd::new("time")))?;
    w.write_event(Event::End(BytesEnd::new("metadata")))?;

    // Per GPX 1.1 spec: <wpt> before <trk>
    for (route_idx, stops) in fuel_stops.iter().enumerate() {
        for (stop_idx, stop) in stops.iter().enumerate() {
            let lat = stop.lat_e7 as f64 / 1e7;
            let lon = stop.lon_e7 as f64 / 1e7;
            let wpt_name = format!("Fuel Stop {} (Route {})", stop_idx + 1, route_idx + 1);
            let mut wpt = BytesStart::new("wpt");
            let lat_s = format!("{:.7}", lat);
            let lon_s = format!("{:.7}", lon);
            wpt.push_attribute(("lat", lat_s.as_str()));
            wpt.push_attribute(("lon", lon_s.as_str()));
            w.write_event(Event::Start(wpt))?;
            w.write_event(Event::Start(BytesStart::new("name")))?;
            w.write_event(Event::Text(BytesText::new(&wpt_name)))?;
            w.write_event(Event::End(BytesEnd::new("name")))?;
            let desc = format!("{:.1} km along route", stop.at_distance_m / 1000.0);
            w.write_event(Event::Start(BytesStart::new("desc")))?;
            w.write_event(Event::Text(BytesText::new(&desc)))?;
            w.write_event(Event::End(BytesEnd::new("desc")))?;
            w.write_event(Event::Start(BytesStart::new("sym")))?;
            w.write_event(Event::Text(BytesText::new("fuel")))?;
            w.write_event(Event::End(BytesEnd::new("sym")))?;
            w.write_event(Event::Start(BytesStart::new("type")))?;
            w.write_event(Event::Text(BytesText::new("fuel")))?;
            w.write_event(Event::End(BytesEnd::new("type")))?;
            w.write_event(Event::End(BytesEnd::new("wpt")))?;
        }
    }

    for (idx, route) in routes.iter().enumerate() {
        let stop_count = fuel_stops.get(idx).map_or(0, |s| s.len());
        write_track(&mut w, route, graph, idx + 1, stop_count)?;
    }

    w.write_event(Event::End(BytesEnd::new("gpx")))?;
    Ok(())
}

fn write_track<W: Write>(
    w: &mut Writer<W>,
    route: &Route,
    graph: &Graph,
    n: usize,
    fuel_stop_count: usize,
) -> anyhow::Result<()> {
    w.write_event(Event::Start(BytesStart::new("trk")))?;

    let track_name = format!("overlandr-route-{}", n);
    w.write_event(Event::Start(BytesStart::new("name")))?;
    w.write_event(Event::Text(BytesText::new(&track_name)))?;
    w.write_event(Event::End(BytesEnd::new("name")))?;

    let km = route.length_m / 1000.0;
    let unpaved_pct = route.unpaved_fraction * 100.0;
    let unpaved_score = route.unpaved_fraction * UNPAVED_WEIGHT;
    let mut desc = format!(
        "length={:.1}km unpaved={:.0}% fords={} 4wd_only={} score={:.2}",
        km, unpaved_pct, route.ford_count, route.fourwd_only_count, unpaved_score
    );
    if fuel_stop_count > 0 {
        desc.push_str(&format!(" fuel_stops={}", fuel_stop_count));
    }
    w.write_event(Event::Start(BytesStart::new("desc")))?;
    w.write_event(Event::Text(BytesText::new(&desc)))?;
    w.write_event(Event::End(BytesEnd::new("desc")))?;

    w.write_event(Event::Start(BytesStart::new("trkseg")))?;

    for (edge_idx, &eid) in route.edges.iter().enumerate() {
        let src_node = route.nodes[edge_idx];
        let node_data = &graph.nodes[src_node as usize];
        write_trkpt(w, node_data.lat_e7 as f64 / 1e7, node_data.lon_e7 as f64 / 1e7)?;

        let edge = &graph.edges[eid as usize];
        for &(pt_lat_e7, pt_lon_e7) in &edge.polyline {
            write_trkpt(w, pt_lat_e7 as f64 / 1e7, pt_lon_e7 as f64 / 1e7)?;
        }
    }

    if let Some(&last_node) = route.nodes.last() {
        let node_data = &graph.nodes[last_node as usize];
        write_trkpt(w, node_data.lat_e7 as f64 / 1e7, node_data.lon_e7 as f64 / 1e7)?;
    }

    w.write_event(Event::End(BytesEnd::new("trkseg")))?;
    w.write_event(Event::End(BytesEnd::new("trk")))?;
    Ok(())
}

fn write_trkpt<W: Write>(w: &mut Writer<W>, lat: f64, lon: f64) -> anyhow::Result<()> {
    debug_assert!((-90.0..=90.0).contains(&lat), "invalid lat: {lat}");
    debug_assert!((-180.0..=180.0).contains(&lon), "invalid lon: {lon}");
    let mut trkpt = BytesStart::new("trkpt");
    let lat_s = format!("{:.7}", lat);
    let lon_s = format!("{:.7}", lon);
    trkpt.push_attribute(("lat", lat_s.as_str()));
    trkpt.push_attribute(("lon", lon_s.as_str()));
    w.write_event(Event::Empty(trkpt))?;
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeData, EdgeFlags, Graph, NodeData};
    use crate::routing::dijkstra::Route;

    fn simple_graph() -> Graph {
        let nodes = vec![
            NodeData { lat_e7: 515_000_000, lon_e7: -1_000_000 },
            NodeData { lat_e7: 515_100_000, lon_e7: -900_000 },
            NodeData { lat_e7: 515_200_000, lon_e7: -800_000 },
        ];
        let edges = vec![
            EdgeData { cost: 1.0, length_m: 1000.0, flags: EdgeFlags::PAVED, scenic_score: 0, polyline: vec![] },
            EdgeData { cost: 1.5, length_m: 1500.0, flags: EdgeFlags::FORD, scenic_score: 0, polyline: vec![(515_150_000, -850_000)] },
        ];
        let offsets = vec![0u32, 1, 2, 2];
        let neighbors: Vec<(u32, u32)> = vec![(1, 0), (2, 1)];
        Graph { node_count: 3, edge_count: 2, nodes, offsets, neighbors, edges, fuel_stations: vec![] }
    }

    fn simple_route() -> Route {
        Route {
            nodes: vec![0, 1, 2],
            edges: vec![0, 1],
            length_m: 2500.0,
            cost: 2.5,
            unpaved_fraction: 0.6,
            ford_count: 1,
            fourwd_only_count: 0,
        }
    }

    #[test]
    fn test_write_gpx_basic() {
        let graph = simple_graph();
        let routes = vec![simple_route()];
        let bytes = write_gpx_to_bytes(&routes, &graph, "Test Route", &[]).expect("write_gpx_to_bytes failed");
        let contents = String::from_utf8(bytes).unwrap();
        assert!(contents.contains("<gpx"), "missing <gpx tag");
        assert!(contents.contains("<trk>"), "missing <trk> tag");
        assert!(contents.contains("overlandr-route-1"), "missing track name");
    }

}
