//! OSM data ingestion: streaming PBF reader, tag helpers, and way filter.

pub mod filter;
pub mod loader;
pub mod tags;

#[allow(unused_imports)]
pub use filter::WayFilter;
#[allow(unused_imports)]
pub use loader::{load_ways, NodeMap, Way};
#[allow(unused_imports)]
pub use tags::{collect_tags, get_tag, tag_in};
