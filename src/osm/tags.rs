//! Helpers for reading string tag values off OSM elements.

use std::collections::HashMap;

/// Extract all tags from a raw key/value pair iterator into a `HashMap`.
///
/// Accepts the iterator form that osmpbf `raw_tags()` returns: `(&str, &str)` tuples.
pub fn collect_tags<'a>(pairs: impl Iterator<Item = (&'a str, &'a str)>) -> HashMap<String, String> {
    pairs.map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

/// Look up a tag by key, returning `None` if the key is absent or the value is empty.
pub fn get_tag<'a>(tags: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    tags.get(key).map(|s| s.as_str()).filter(|s| !s.is_empty())
}

/// Return `true` if the tag key is present and its value is contained in `values`.
pub fn tag_in(tags: &HashMap<String, String>, key: &str, values: &[&str]) -> bool {
    match tags.get(key) {
        Some(v) => values.contains(&v.as_str()),
        None => false,
    }
}
