//! Way-level tag filter implementing the highway allowlist and access drop rules from DESIGN.md §5.2.

use std::collections::{HashMap, HashSet};

use crate::osm::tags::{get_tag, tag_in};
use crate::profile::Profile;

/// Access tag values that are treated as "no access" for the public default.
const ACCESS_DENY: &[&str] = &["no", "private"];

/// Configuration controlling which ways are retained during the loading pass.
#[derive(Debug, Clone)]
pub struct WayFilter {
    /// Highway tag values to keep; derived from profile base cost keys.
    pub allowed_classes: HashSet<String>,
    /// If `true`, ways tagged `access=private` or `access=no` are kept.
    pub keep_private: bool,
    /// If `true`, the filter is operating in a motorised-vehicle context and will drop
    /// `motor_vehicle=no` ways.
    pub motorized: bool,
    /// If `true`, low-class unpaved ways (track, path, bridleway, cycleway, footway)
    /// are dropped from OSM.  Set when a USFS shapefile is being merged so that
    /// USFS geometry takes precedence over OSM's often-sparse track geometry.
    pub skip_tracks: bool,
}

impl WayFilter {
    /// Build a filter whose allowed highway classes come from the profile's base cost table.
    pub fn from_profile(profile: &Profile, keep_private: bool, motorized: bool, skip_tracks: bool) -> Self {
        Self {
            allowed_classes: profile.base.0.keys().cloned().collect(),
            keep_private,
            motorized,
            skip_tracks,
        }
    }
}

/// Highway tag values suppressed when `skip_tracks` is set.
const TRACK_CLASSES: &[&str] = &["track", "path", "bridleway", "cycleway", "footway"];

impl WayFilter {
    /// Return `true` if the way described by `tags` should be kept.
    ///
    /// Rules (applied in order):
    /// 1. `highway` must be present and in `allowed_classes` (derived from profile).
    /// 2. Unless `keep_private` is set, `access=no|private` → drop.
    /// 3. `vehicle=no` → drop (unconditional).
    /// 4. When `motorized`, `motor_vehicle=no` → drop.
    pub fn keep(&self, tags: &HashMap<String, String>) -> bool {
        // Rule 1: highway must be in the profile-derived allowlist.
        let hw = match get_tag(tags, "highway") {
            Some(hw) if self.allowed_classes.contains(hw) => hw,
            _ => return false,
        };

        // Rule 1b: when skip_tracks is set, drop low-class unpaved ways.
        if self.skip_tracks && TRACK_CLASSES.contains(&hw) {
            return false;
        }

        // Rule 2: access=no|private dropped unless keep_private.
        if !self.keep_private {
            if tag_in(tags, "access", ACCESS_DENY) {
                return false;
            }
        }

        // Rule 3: vehicle=no is always dropped.
        if tag_in(tags, "vehicle", &["no"]) {
            return false;
        }

        // Rule 4: motor_vehicle=no dropped for motorized profiles.
        if self.motorized && tag_in(tags, "motor_vehicle", &["no"]) {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::load_profile;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn default_filter() -> WayFilter {
        let profile = load_profile(None).unwrap();
        WayFilter::from_profile(&profile, false, true, false)
    }

    #[test]
    fn keeps_track() {
        assert!(default_filter().keep(&tags(&[("highway", "track")])));
    }

    #[test]
    fn drops_missing_highway() {
        assert!(!default_filter().keep(&tags(&[("name", "Some Road")])));
    }

    #[test]
    fn drops_unknown_highway() {
        assert!(!default_filter().keep(&tags(&[("highway", "proposed")])));
    }

    #[test]
    fn drops_private_access_by_default() {
        assert!(!default_filter().keep(&tags(&[("highway", "track"), ("access", "private")])));
    }

    #[test]
    fn keeps_private_when_flag_set() {
        let profile = load_profile(None).unwrap();
        let f = WayFilter::from_profile(&profile, true, true, false);
        assert!(f.keep(&tags(&[("highway", "track"), ("access", "private")])));
    }

    #[test]
    fn drops_vehicle_no() {
        let profile = load_profile(None).unwrap();
        let f = WayFilter::from_profile(&profile, true, false, false);
        assert!(!f.keep(&tags(&[("highway", "track"), ("vehicle", "no")])));
    }

    #[test]
    fn drops_motor_vehicle_no_for_motorized() {
        assert!(!default_filter().keep(&tags(&[("highway", "track"), ("motor_vehicle", "no")])));
    }

    #[test]
    fn keeps_motor_vehicle_no_for_non_motorized() {
        let profile = load_profile(None).unwrap();
        let f = WayFilter::from_profile(&profile, false, false, false);
        assert!(f.keep(&tags(&[("highway", "track"), ("motor_vehicle", "no")])));
    }

    #[test]
    fn skip_tracks_drops_track_and_path() {
        let profile = load_profile(None).unwrap();
        let f = WayFilter::from_profile(&profile, false, true, true);
        assert!(!f.keep(&tags(&[("highway", "track")])));
        assert!(!f.keep(&tags(&[("highway", "path")])));
        assert!(!f.keep(&tags(&[("highway", "footway")])));
    }

    #[test]
    fn skip_tracks_keeps_unclassified_and_above() {
        let profile = load_profile(None).unwrap();
        let f = WayFilter::from_profile(&profile, false, true, true);
        assert!(f.keep(&tags(&[("highway", "unclassified")])));
        assert!(f.keep(&tags(&[("highway", "secondary")])));
        assert!(f.keep(&tags(&[("highway", "residential")])));
    }
}
