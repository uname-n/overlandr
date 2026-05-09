use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Per-highway-class base cost multipliers.
///
/// Keys are OSM `highway` tag values (e.g. `"track"`, `"unclassified"`).
/// Values < 1.0 make the algorithm prefer that road class; values > 1.0 discourage it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BaseFactors(pub BTreeMap<String, f32>);

/// Per-surface-tag cost multipliers.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SurfaceFactors(pub BTreeMap<String, f32>);

/// Per-tracktype-tag cost multipliers.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TracktypeFactors(pub BTreeMap<String, f32>);

/// Per-smoothness-tag cost multipliers.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SmoothnessFactors(pub BTreeMap<String, f32>);

/// Per-vehicle overrides and penalties.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VehicleProfile {
    pub fourwd_only_penalty: f32,
    pub min_smoothness: String,
    #[serde(default)]
    pub narrow_path_bonus: Option<f32>,
}

/// Tunable routing algorithm constants exposed via TOML.
///
/// All fields have sensible defaults so existing profiles without a `[routing]`
/// section continue to work unchanged.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoutingConfig {
    /// Penalty growth factor applied to accepted-route edges so future
    /// searches explore different corridors. Default: `1.5`.
    #[serde(default = "default_lambda")]
    pub lambda: f32,

    /// Fraction of tank range kept as a buffer; a fuel stop is triggered when
    /// remaining range drops to `fuel_buffer * tank_range`. Default: `0.20`.
    #[serde(default = "default_fuel_buffer")]
    pub fuel_buffer: f32,

    /// Spatial grid cell size in degrees used for conflation (~111 m per cell).
    /// Default: `0.001`.
    #[serde(default = "default_grid_step")]
    pub grid_step: f64,

    /// Cost multiplier applied to edges tagged `ford=yes` at graph build time.
    /// Fords are passable but naturally more expensive than dry road.
    /// Default: `3.0`.
    #[serde(default = "default_ford_penalty")]
    pub ford_penalty: f32,

    /// OSM `surface` tag values that classify a way as paved.
    /// Paved ways receive the `PAVED` edge flag and are penalised by
    /// `avoid_paved` routing. Default matches the most common paved values.
    #[serde(default = "default_paved_surfaces")]
    pub paved_surfaces: Vec<String>,

    /// OSM `smoothness` values that set the `SMOOTHNESS_ROUGH` edge flag.
    #[serde(default = "default_smoothness_rough")]
    pub smoothness_rough: Vec<String>,

    /// OSM `smoothness` values that set the `SMOOTHNESS_VERY_ROUGH` edge flag.
    #[serde(default = "default_smoothness_very_rough")]
    pub smoothness_very_rough: Vec<String>,
}

fn default_lambda() -> f32 { 1.5 }
fn default_fuel_buffer() -> f32 { 0.20 }
fn default_grid_step() -> f64 { 0.001 }
fn default_ford_penalty() -> f32 { 3.0 }
fn default_paved_surfaces() -> Vec<String> {
    ["asphalt", "paved", "concrete", "cobblestone", "sett"]
        .iter().map(|s| s.to_string()).collect()
}
fn default_smoothness_rough() -> Vec<String> {
    ["very_bad", "horrible", "very_horrible", "impassable"]
        .iter().map(|s| s.to_string()).collect()
}
fn default_smoothness_very_rough() -> Vec<String> {
    ["horrible", "very_horrible", "impassable"]
        .iter().map(|s| s.to_string()).collect()
}

impl Default for RoutingConfig {
    fn default() -> Self {
        RoutingConfig {
            lambda: default_lambda(),
            fuel_buffer: default_fuel_buffer(),
            grid_step: default_grid_step(),
            ford_penalty: default_ford_penalty(),
            paved_surfaces: default_paved_surfaces(),
            smoothness_rough: default_smoothness_rough(),
            smoothness_very_rough: default_smoothness_very_rough(),
        }
    }
}

/// A complete costing profile deserialized from TOML.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Profile {
    pub base: BaseFactors,
    pub surface: SurfaceFactors,
    pub tracktype: TracktypeFactors,
    pub smoothness: SmoothnessFactors,
    pub vehicle: BTreeMap<String, VehicleProfile>,
    /// Tunable routing algorithm constants. All fields have defaults.
    #[serde(default)]
    pub routing: RoutingConfig,
}

impl Profile {
    /// SHA-256 fingerprint of the serialized TOML bytes.
    /// Used for cache-invalidation: the graph stores this hash so that
    /// `route` can refuse to mix incompatible profiles.
    pub fn fingerprint(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        // Re-serialize to a canonical byte string for hashing.
        let bytes = toml::to_string(self)
            .expect("Profile must be serializable back to TOML")
            .into_bytes();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        hasher.finalize().into()
    }

    /// Cost multiplier for a `highway` tag value. Returns `1.0` for unknown keys.
    pub fn base_factor(&self, highway: &str) -> f32 {
        self.base.0.get(highway).copied().unwrap_or(1.0)
    }

    /// Cost multiplier for a `surface` tag value. Returns `1.0` for unknown keys.
    pub fn surface_factor(&self, surface: &str) -> f32 {
        self.surface.0.get(surface).copied().unwrap_or(1.0)
    }

    /// Cost multiplier for a `tracktype` tag value. Returns `1.0` for unknown keys.
    pub fn tracktype_factor(&self, tt: &str) -> f32 {
        self.tracktype.0.get(tt).copied().unwrap_or(1.0)
    }

    /// Cost multiplier for a `smoothness` tag value. Returns `1.0` for unknown keys.
    pub fn smoothness_factor(&self, sm: &str) -> f32 {
        self.smoothness.0.get(sm).copied().unwrap_or(1.0)
    }
}

#[cfg(test)]
mod tests {
    use crate::profile::load_profile;

    #[test]
    fn embedded_profile_deserializes() {
        let profile = load_profile(None).expect("embedded profile must parse");
        assert_eq!(
            profile.base_factor("track"),
            0.6,
            "track base factor should be 0.6"
        );
    }

    #[test]
    fn unknown_keys_return_default() {
        let profile = load_profile(None).expect("embedded profile must parse");
        assert_eq!(profile.base_factor("nonexistent_highway"), 1.0);
        assert_eq!(profile.surface_factor("nonexistent_surface"), 1.0);
        assert_eq!(profile.tracktype_factor("nonexistent_grade"), 1.0);
        assert_eq!(profile.smoothness_factor("nonexistent_smooth"), 1.0);
    }

    #[test]
    fn vehicle_profiles_present() {
        let profile = load_profile(None).expect("embedded profile must parse");
        assert!(profile.vehicle.contains_key("stock-suv"));
        assert!(profile.vehicle.contains_key("high-clearance"));
        assert!(profile.vehicle.contains_key("4x4"));
        assert!(profile.vehicle.contains_key("dirtbike"));
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let p1 = load_profile(None).expect("parse");
        let p2 = load_profile(None).expect("parse");
        assert_eq!(p1.fingerprint(), p2.fingerprint());
    }

    #[test]
    fn dirtbike_has_narrow_path_bonus() {
        let profile = load_profile(None).expect("embedded profile must parse");
        let db = &profile.vehicle["dirtbike"];
        assert_eq!(db.narrow_path_bonus, Some(0.5));
    }
}
