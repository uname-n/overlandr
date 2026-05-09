use std::path::Path;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use crate::graph::{Graph, SpatialIndex};

const CACHE_MAGIC: u32 = 0x4F564C52; // b"OVLR"
const CACHE_VERSION: u32 = 1;
const CACHE_STALENESS_DAYS: i64 = 180;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    graph: Graph,
    index: SpatialIndex,
    profile_fingerprint: [u8; 32],
    pbf_timestamp: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
struct CacheFileRef<'a> {
    graph: &'a Graph,
    index: &'a SpatialIndex,
    profile_fingerprint: [u8; 32],
    pbf_timestamp: Option<DateTime<Utc>>,
}

/// Serialize `graph` + `index` + metadata to `path`, compressed with zstd level 3.
///
/// File layout: `[magic: u32 LE][version: u32 LE][zstd-compressed bincode body]`
pub fn save(
    graph: &Graph,
    index: &SpatialIndex,
    profile_fingerprint: [u8; 32],
    pbf_timestamp: Option<DateTime<Utc>>,
    path: &Path,
) -> anyhow::Result<()> {
    let cache = CacheFileRef { graph, index, profile_fingerprint, pbf_timestamp };
    let body = bincode::serialize(&cache)?;
    let compressed = zstd::encode_all(body.as_slice(), 3)?;

    let mut out = Vec::with_capacity(8 + compressed.len());
    out.extend_from_slice(&CACHE_MAGIC.to_le_bytes());
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&compressed);
    std::fs::write(path, out)?;
    Ok(())
}

/// Load and decompress a cache file written by `save`.
///
/// Returns an error if the file header doesn't match the expected magic/version,
/// prompting the user to rebuild the cache.
///
/// Emits a warning if the embedded PBF timestamp is older than 6 months.
pub fn load(path: &Path) -> anyhow::Result<(Graph, SpatialIndex, [u8; 32], Option<DateTime<Utc>>)> {
    let data = std::fs::read(path)?;
    if data.len() < 8 {
        anyhow::bail!("cache format mismatch — please rebuild with 'overlandr build'");
    }
    let magic = u32::from_le_bytes(data[..4].try_into().unwrap());
    let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if magic != CACHE_MAGIC || version != CACHE_VERSION {
        anyhow::bail!("cache format mismatch — please rebuild with 'overlandr build'");
    }
    let bytes = zstd::decode_all(&data[8..])?;
    let cache: CacheFile = bincode::deserialize(&bytes)?;
    if let Some(ts) = cache.pbf_timestamp {
        if Utc::now() - ts > chrono::Duration::days(CACHE_STALENESS_DAYS) {
            tracing::warn!("PBF extract is >6 months old");
        }
    }
    Ok((cache.graph, cache.index, cache.profile_fingerprint, cache.pbf_timestamp))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{EdgeData, EdgeFlags, NodeData, SpatialIndex};

    fn make_small_graph() -> Graph {
        Graph {
            nodes: vec![
                NodeData { lat_e7: 477_000_000, lon_e7: -1_160_000_000 },
                NodeData { lat_e7: 477_100_000, lon_e7: -1_160_100_000 },
            ],
            offsets: vec![0, 1, 1],
            neighbors: vec![(1, 0)],
            edges: vec![EdgeData {
                cost: 10.0,
                length_m: 1000.0,
                flags: EdgeFlags::PAVED,
                scenic_score: 123,
                polyline: vec![],
            }],
            node_count: 2,
            edge_count: 1,
            fuel_stations: vec![],
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let graph = make_small_graph();
        let index = SpatialIndex::build(&graph);
        let fingerprint = [0u8; 32];

        let tmp_path = std::env::temp_dir().join("overlandr_cache_test.bin");
        save(&graph, &index, fingerprint, None, &tmp_path).expect("save");
        let (g2, _idx2, fp2, ts2) = load(&tmp_path).expect("load");
        let _ = std::fs::remove_file(&tmp_path);

        assert_eq!(g2.node_count, 2);
        assert_eq!(g2.nodes.len(), 2);
        assert_eq!(g2.edges.len(), 1);
        assert_eq!(g2.edges[0].scenic_score, 123);
        assert_eq!(fp2, fingerprint);
        assert!(ts2.is_none());
    }
}
