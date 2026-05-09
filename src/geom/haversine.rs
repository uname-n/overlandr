use geo::{HaversineDistance, Point};

/// Returns the haversine distance in metres between two WGS-84 coordinates.
pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let a = Point::new(lon1, lat1);
    let b = Point::new(lon2, lat2);
    a.haversine_distance(&b)
}

#[cfg(test)]
mod tests {
    use super::haversine_m;

    #[test]
    fn same_point_is_zero() {
        assert_eq!(haversine_m(47.0, -116.0, 47.0, -116.0), 0.0);
    }

    #[test]
    fn one_degree_lat_approx_111_195_m() {
        let d = haversine_m(0.0, 0.0, 1.0, 0.0);
        let expected = 111_195.0_f64;
        assert!(
            (d - expected).abs() < 100.0,
            "expected ~{expected} m, got {d} m"
        );
    }
}
