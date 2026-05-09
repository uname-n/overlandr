mod toml;

#[allow(unused_imports)]
pub use toml::{
    BaseFactors, Profile, RoutingConfig, SmoothnessFactors, SurfaceFactors, TracktypeFactors,
    VehicleProfile,
};

use std::path::Path;

/// Load a costing profile from a TOML file.
///
/// If `path` is `None`, the embedded `profiles/overland.toml` default is used.
/// Returns a fully-parsed [`Profile`] ready for cost lookups.
pub fn load_profile(path: Option<&Path>) -> Result<Profile, Box<dyn std::error::Error>> {
    let source = match path {
        Some(p) => {
            if p.extension().and_then(|e| e.to_str()) != Some("toml") {
                return Err("profile path must have a .toml extension".into());
            }
            if p.components().any(|c| c == std::path::Component::ParentDir) {
                return Err("profile path must not contain '..'".into());
            }
            std::fs::read_to_string(p)?
        }
        None => include_str!("../../profiles/overland.toml").to_owned(),
    };
    let profile: Profile = ::toml::from_str(&source)?;
    Ok(profile)
}
