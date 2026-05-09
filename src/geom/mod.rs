mod haversine;
pub use haversine::haversine_m;

/// Metres per degree of latitude (approximate, at the equator).
pub const METRES_PER_DEGREE: f64 = 111_111.0;

/// Grid cell size in degrees (~111 m per cell at the equator).
/// Shared by the conflation and USFS snap grids.
pub const GRID_STEP: f64 = 0.001;

/// Map `(lat, lon)` to a `(row, col)` grid cell index using [`GRID_STEP`].
pub fn grid_cell(lat: f64, lon: f64) -> (i64, i64) {
    ((lat / GRID_STEP).floor() as i64, (lon / GRID_STEP).floor() as i64)
}

