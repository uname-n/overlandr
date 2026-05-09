pub mod builder;
#[allow(unused_imports)]
pub use builder::{build_graph, BBox, BuildOpts};

pub mod contract;
#[allow(unused_imports)]
pub use contract::contract;

pub mod spatial;
pub use spatial::SpatialIndex;

pub mod cache;

// ---------------------------------------------------------------------------
// Core type aliases
// ---------------------------------------------------------------------------

/// Index into `Graph::nodes`.
pub type NodeId = u32;

/// Index into `Graph::edges`.
pub type EdgeId = u32;

// ---------------------------------------------------------------------------
// Node and edge data
// ---------------------------------------------------------------------------

/// Coordinates of a graph node, stored as integer micro-degrees (×10⁷).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct NodeData {
    pub lat_e7: i32,
    pub lon_e7: i32,
}

/// Bitfield encoding surface and access properties of an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct EdgeFlags(pub u8);

impl EdgeFlags {
    /// Surface is paved (asphalt, concrete, etc.).
    pub const PAVED: EdgeFlags = EdgeFlags(0x01);
    /// Edge crosses water (ford).
    pub const FORD: EdgeFlags = EdgeFlags(0x02);
    /// Edge requires four-wheel drive.
    pub const FOURWD_ONLY: EdgeFlags = EdgeFlags(0x04);
    /// Edge is seasonal (may be impassable in winter).
    pub const SEASONAL: EdgeFlags = EdgeFlags(0x08);
    /// Access is private; included only when `--keep-private` is set.
    pub const PRIVATE: EdgeFlags = EdgeFlags(0x10);
    /// Smoothness is very_bad or worse — blocks stock-suv vehicles.
    pub const SMOOTHNESS_ROUGH: EdgeFlags = EdgeFlags(0x20);
    /// Smoothness is horrible or worse — blocks high-clearance vehicles.
    pub const SMOOTHNESS_VERY_ROUGH: EdgeFlags = EdgeFlags(0x40);

    /// Returns `true` if `self` contains all bits set in `flag`.
    #[inline]
    pub fn contains(self, flag: EdgeFlags) -> bool {
        (self.0 & flag.0) == flag.0
    }
}

impl std::ops::BitOr for EdgeFlags {
    type Output = EdgeFlags;
    fn bitor(self, rhs: EdgeFlags) -> EdgeFlags {
        EdgeFlags(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for EdgeFlags {
    fn bitor_assign(&mut self, rhs: EdgeFlags) {
        self.0 |= rhs.0;
    }
}

/// Per-edge routing data.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EdgeData {
    /// Routing cost (seconds or abstract units).
    pub cost: f32,
    /// Great-circle length in metres.
    pub length_m: f32,
    /// Surface and access flags.
    pub flags: EdgeFlags,
    /// Scenic desirability score in the range 0..=255.
    #[serde(default)]
    pub scenic_score: u8,
    /// Geometry for contracted (shortcut) edges; empty for plain edges.
    pub polyline: Vec<(i32, i32)>,
}

// ---------------------------------------------------------------------------
// Fuel stations and scenic features
// ---------------------------------------------------------------------------

/// A fuel station extracted from OSM `amenity=fuel` nodes.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FuelStation {
    /// Latitude in integer micro-degrees (×10⁷).
    pub lat_e7: i32,
    /// Longitude in integer micro-degrees (×10⁷).
    pub lon_e7: i32,
    /// OSM node ID.
    pub osm_id: i64,
}

/// Scenic feature classes harvested from OSM for later edge scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ScenicKind {
    Viewpoint,
    Peak,
    Saddle,
    Water,
    River,
    Stream,
    Forest,
    ProtectedArea,
    NatureReserve,
    Glacier,
    Cliff,
}

/// Compact point representation of a scenic feature.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScenicFeature {
    /// Latitude in integer micro-degrees (×10⁷).
    pub lat_e7: i32,
    /// Longitude in integer micro-degrees (×10⁷).
    pub lon_e7: i32,
    /// Feature class.
    pub kind: ScenicKind,
    /// OSM entity ID, positive for nodes and negative for ways.
    pub osm_id: i64,
}

// ---------------------------------------------------------------------------
// Graph (CSR)
// ---------------------------------------------------------------------------

/// Directed graph in Compressed Sparse Row (CSR) format.
///
/// `offsets[u]..offsets[u+1]` is the slice of `neighbors` belonging to node `u`.
/// Each entry in `neighbors` is a `(target NodeId, EdgeId)` pair.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Graph {
    /// All node coordinates, indexed by [`NodeId`].
    pub nodes: Vec<NodeData>,
    /// CSR offset array; length == `node_count + 1`.
    pub offsets: Vec<u32>,
    /// Adjacency list stored as `(target, edge_id)` pairs.
    pub neighbors: Vec<(NodeId, EdgeId)>,
    /// All edge data, indexed by [`EdgeId`].
    pub edges: Vec<EdgeData>,
    /// Total number of nodes.
    pub node_count: usize,
    /// Total number of directed edges.
    pub edge_count: usize,
    /// Fuel stations (`amenity=fuel`) collected during graph build.
    #[serde(default)]
    pub fuel_stations: Vec<FuelStation>,
}

impl Graph {
    /// Returns the outgoing adjacency list of `node` as `(target, edge_id)` pairs.
    pub fn neighbors(&self, node: NodeId) -> &[(NodeId, EdgeId)] {
        let lo = self.offsets[node as usize] as usize;
        let hi = self.offsets[node as usize + 1] as usize;
        &self.neighbors[lo..hi]
    }
}
