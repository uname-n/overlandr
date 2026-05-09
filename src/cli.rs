use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "overlandr", version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[arg(long, env = "GRAPH_PATH", default_value = "./graph.bin")]
    pub graph: std::path::PathBuf,
    #[arg(long, default_value = "info")]
    pub log: String,
    #[arg(long)]
    pub threads: Option<usize>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Build {
        pbf: std::path::PathBuf,
        #[arg(long, default_value = "graph.bin")]
        out: std::path::PathBuf,
        #[arg(long)]
        profile: Option<std::path::PathBuf>,
        /// Bounding box in "minlon,minlat,maxlon,maxlat" format
        #[arg(long)]
        bbox: Option<String>,
        #[arg(long)]
        keep_private: bool,
        #[arg(long)]
        no_simplify: bool,
        /// Path to USFS NFS Roads shapefile (.shp) to merge with OSM data
        #[arg(long)]
        usfs: Option<std::path::PathBuf>,
        /// Snap radius in metres: USFS road endpoints within this distance of
        /// an OSM node are merged with it so the two networks connect
        #[arg(long, default_value = "50.0")]
        usfs_snap: f32,
    },
    Serve {
        #[arg(long, env = "PORT", default_value = "3000")]
        port: u16,
        #[arg(long, env = "HOST", default_value = "0.0.0.0")]
        host: String,
    },
    Inspect,
    Tags {
        #[arg(long)]
        profile: Option<std::path::PathBuf>,
    },
}
