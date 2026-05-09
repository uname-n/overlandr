use rstar::{RTree, RTreeObject, AABB, PointDistance};
use serde::{Serialize, Deserialize, Serializer, Deserializer};
use crate::graph::{Graph, NodeId};
use crate::geom::haversine_m;

#[derive(Clone, Serialize, Deserialize)]
struct NodeRef {
    node_id: NodeId,
    lat: f64,
    lon: f64,
}

impl RTreeObject for NodeRef {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        AABB::from_point([self.lon, self.lat])
    }
}

impl PointDistance for NodeRef {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        let dlat = self.lat - point[1];
        let dlon = self.lon - point[0];
        dlat * dlat + dlon * dlon
    }
}

pub struct SpatialIndex {
    tree: RTree<NodeRef>,
}

// Custom serde: store nodes as a Vec, rebuild RTree on deserialize.
impl Serialize for SpatialIndex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let nodes: Vec<&NodeRef> = self.tree.iter().collect();
        nodes.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SpatialIndex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let nodes: Vec<NodeRef> = Vec::deserialize(deserializer)?;
        Ok(SpatialIndex {
            tree: RTree::bulk_load(nodes),
        })
    }
}

impl SpatialIndex {
    /// Build a spatial index from all nodes in `graph`.
    pub fn build(graph: &Graph) -> SpatialIndex {
        let nodes: Vec<NodeRef> = graph
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| NodeRef {
                node_id: i as NodeId,
                lat: n.lat_e7 as f64 / 1e7,
                lon: n.lon_e7 as f64 / 1e7,
            })
            .collect();
        SpatialIndex {
            tree: RTree::bulk_load(nodes),
        }
    }

    /// Returns the `NodeId` of the nearest node within `max_dist_m` metres,
    /// or an error if the nearest node exceeds that distance.
    pub fn nearest(&self, lat: f64, lon: f64, max_dist_m: f64) -> anyhow::Result<NodeId> {
        match self.tree.nearest_neighbor(&[lon, lat]) {
            None => anyhow::bail!("snap point too far: no graph nodes found"),
            Some(nr) => {
                let dist = haversine_m(lat, lon, nr.lat, nr.lon);
                anyhow::ensure!(
                    dist <= max_dist_m,
                    "snap point too far: {dist:.1} m from nearest graph node"
                );
                Ok(nr.node_id)
            }
        }
    }
}
