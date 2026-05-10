# Overlandr Algorithm

Overlandr is an offline routing engine that finds multiple topologically distinct route alternatives between two GPS coordinates using OpenStreetMap data. It favors unpaved roads, forest service tracks, and gravel corridors over paved highways, and outputs a multi-track GPX file.

---

## Pipeline Overview

```
OSM PBF
  ↓
Load routable ways, fuel stations, and scenic features (2-pass streaming, parallel)
  ↓
[Optional: conflate with USFS shapefile]
  ↓
Build CSR graph + per-edge scenic scores  →  Contract degree-2 chains  →  Build R-Tree spatial index
  ↓
Serialize to disk (.bin, zstd+bincode)
  ↓
── Route request ──────────────────────────────────────────
Snap coordinates to nearest node via R-Tree
  ↓
k-alternatives: iterative penalty Dijkstra
  ↓
Diversity + detour filtering
  ↓
Score and rank by offroad appeal
  ↓
[Optional: fuel stop planning]
  ↓
GPX output (one <trk> per alternative)
```

---

## Phase 1: Graph Construction (Build)

### OSM Loading

Two-pass streaming over a `.osm.pbf` extract:

- **Pass 1** — scan ways in parallel, filter by `WayFilter` (highway type + access rules), collect referenced node IDs, and keep scenic way candidates such as rivers, streams, forests, protected areas, nature reserves, glaciers, and cliffs
- **Pass 2** — scan nodes in parallel, store coordinates only for nodes referenced in Pass 1; extract `amenity=fuel` stations and scenic node features such as viewpoints, peaks, saddles, water, glaciers, and cliffs

Scenic ways are reduced to representative centroid points after node coordinates are known, producing a compact feature set used later during edge scoring.

### Tag-Based Edge Costing

Each edge gets a cost multiplier based on OSM tags:

```
cost = length_m × base_factor(highway)
               × surface_factor(surface)
               × tracktype_factor(tracktype)
               × smoothness_factor(smoothness)
               × ford_penalty
```

Lower multipliers bias Dijkstra toward those edges. These base/tag multipliers are compiled into each edge during graph build, so changing the costing profile requires rebuilding the graph. Request-time preferences such as scenic bias, avoid-paved, avoid-fords, and vehicle-specific penalties are layered on later during routing.

Default `[base]` highway factors (lower = preferred):

Non-dirtbike vehicles also hard-block edges flagged as `TRAIL`; the `dirtbike` profile keeps those classes available.

| Highway class | Factor | Notes |
|---|---|---|
| `track` | 0.6 | Forest service roads, the primary target |
| `unclassified` | 1.0 | Neutral baseline |
| `tertiary` | 2.0 | Small urban/rural through-road |
| `service` | 8.0 | Parking lots, driveways — last resort |
| `secondary` | 4.0 | |
| `residential` | 5.0 | Avoids neighborhood routing in cities |
| `primary` | 8.0 | |
| `trunk` | 6.0 | Viable for cutting through urban areas |
| `motorway` | 12.0 | Freeway — used to transit cities quickly |
| `path` | 5.0 | Only when no alternative exists |

Residential and service roads are intentionally penalized above secondary/tertiary so the router uses arterials and freeways when transiting urban areas rather than wandering through neighborhoods or parking lots.

### Edge Flags

Each edge carries a `u8` bitfield:

| Flag | Bit | Meaning |
|------|-----|---------|
| `PAVED` | 0x01 | asphalt, concrete, paved |
| `FORD` | 0x02 | water crossing |
| `FOURWD_ONLY` | 0x04 | 4WD-required tags |
| `SEASONAL` | 0x08 | seasonal/winter closure |
| `PRIVATE` | 0x10 | private access |
| `SMOOTHNESS_ROUGH` | 0x20 | rough smoothness tag |
| `SMOOTHNESS_VERY_ROUGH` | 0x40 | very rough smoothness tag |
| `TRAIL` | 0x80 | non-road trail class (`path`, `footway`, `bridleway`, `cycleway`) |

### CSR Graph Construction

1. Assign compact `NodeId` (u32) to every OSM node referenced by accepted ways
2. Emit bidirectional edges (or unidirectional for `oneway=yes`)
3. Sort edges by source node
4. Fill offset array for O(1) adjacency list lookup

### Graph Contraction (Degree-2 Elimination)

Chains of intermediate nodes with exactly 2 neighbors are collapsed into single edges. Intermediate coordinates are preserved in a per-edge polyline field so output geometry is lossless. On real extracts this can substantially reduce node count and improve Dijkstra throughput.

### Scenic Edge Scoring

After edges are created, overlandr scores each one against nearby scenic features. Each feature class contributes within a bounded radius, with stronger weights for high-value features like viewpoints, protected areas, and glaciers, and broader radii for landscape-scale features like forests and protected areas. The accumulated score is quantized into `0..=255` and stored on `EdgeData::scenic_score`.

At route time, `prefer_scenic=true` converts that score into a soft multiplier that can reduce edge cost by up to 35%, clamped so scenic preference never overwhelms harder access or safety penalties.

### Spatial Index

All nodes are bulk-loaded into an R-Tree. Used at routing time to snap query coordinates to the nearest road node (max snap radius: 1000 m).

### USFS Conflation (Optional)

When a USFS shapefile is provided, spatial grid bucketing (~111 m cells) identifies OSM ways that overlap with USFS roads. Low-class ways (`track`, `path`, `bridleway`, `cycleway`, `footway`) with sufficient edge coverage (≥50 m proximity fraction) are removed to avoid duplicates.

### Serialization

The finished graph, spatial index, and profile fingerprint (SHA-256) are compressed with zstd and written to a `.bin` cache. The fingerprint ensures the cache is invalidated if the profile changes.

---

## Phase 2: Routing (k-Alternatives)

### Bidirectional Dijkstra

Each call to the router runs a standard bidirectional Dijkstra:

- Expand frontiers from source and destination simultaneously
- Terminate when `fwd_top + bwd_top ≥ best_path_found`
- Reconstruct path by following parent pointers from the meeting node in both directions
- Edge penalties are applied via a `HashMap<EdgeId, f32>` lookup during relaxation

### Iterative Penalty Method

The k-alternatives algorithm steers successive queries away from previously found routes:

```
R_0 = dijkstra(src, dst, penalties={})

for i = 1 to k-1:
    apply penalty λ to all edges of R_{i-1}
    for j = 1 to max_retries:
        R = dijkstra(src, dst, current_penalties)
        if passes diversity check AND detour check:
            accept R as R_i; break
        else:
            double penalty on rejected edges; retry
    if no candidate found: stop
```

This avoids the expensive label-correcting enumeration of k-shortest-paths algorithms while still producing routes that explore different corridors.

### Diversity Check (Jaccard Distance)

Each candidate is compared against all previously accepted routes using edge-set Jaccard distance:

```
distance = 1 − |A ∩ B| / |A ∪ B|
```

Default minimum distance: **0.35**. Routes that share more than 65% of their edges with any accepted route are rejected. Edge comparison captures topological distinctness even when routes share nodes.

### Detour Check

```
route_length ≤ shortest_length × max_detour
```

Default `max_detour`: **1.6**. Prevents the penalty method from returning routes that are implausibly long.

### Route Scoring

After all alternatives are collected, they are ranked by:

```
score = unpaved_fraction × 0.6 − (length_ratio − 1.0) × 0.4
```

Higher unpaved fraction increases score; detour ratio decreases it. Routes are returned best-first.

Note that the GPX track `<desc>` exposes only the weighted unpaved component (`unpaved_fraction × 0.6`) as its `score=` field; the detour penalty is used internally for ranking, not written into the GPX description.

---

## Phase 3: Auxiliary Features

### Fuel Stop Planning

When `tank_range_km` is set:

1. Precompute cumulative distance along route nodes (O(n), Haversine)
2. Trigger a fuel search when distance since the last refuel reaches `tank_range × (1 − fuel_buffer)` (equivalently: remaining range falls to `tank_range × fuel_buffer`)
3. Query the R-Tree for the nearest fuel station within 2 km
4. Prefer stations found earlier along the route via lookback search
5. Advance past the placed stop to avoid re-triggering

### GPX Output

Each accepted alternative becomes a `<trk>` element. The `<desc>` field includes:

- Total length (km)
- Unpaved percentage
- Ford count
- 4WD-only segment count
- Weighted unpaved score (`unpaved_fraction × 0.6`)
- Optional fuel stop count

If fuel planning is enabled, individual stops are also emitted as separate `<wpt>` elements ahead of the tracks.

---

## Key Parameters

| Parameter | Default | Effect |
|-----------|---------|--------|
| `alternatives` (k) | 1 | Number of routes to find |
| `diversity` | 0.35 | Minimum Jaccard distance between routes |
| `max_detour` | 1.6 | Maximum length ratio vs. shortest path |
| `lambda` (λ) | 1.5 | Initial penalty multiplier on previous route edges |
| `max_retries` | 4 | Retry attempts per alternative before giving up |
| `fuel_buffer` | 0.20 | Reserve fraction of tank before triggering fuel search |
| `tank_range_km` | — | Vehicle range; omit to disable fuel planning |
| `prefer_scenic` | true | Enable soft scenic preference from build-time edge scores |
| `scenic_weight` | 1.0 | Strength of scenic preference, scaled into the edge multiplier |

---

## Data Structures

| Type | Description |
|------|-------------|
| `Graph` | CSR graph: `Vec<NodeData>`, `Vec<EdgeData>`, offset array, neighbor array |
| `NodeData` | Fixed-point coordinates: `lat_e7`, `lon_e7` (×10⁷ integer) |
| `EdgeData` | `cost` (f32), `length_m` (f32), `flags` (u8), `scenic_score` (u8), `polyline` (contracted coords) |
| `SpatialIndex` | R-Tree over all node coordinates |
| `Profile` | Cost multiplier maps; vehicle profiles; algorithm constants |
| `Route` | `nodes`, `edges`, `length_m`, `unpaved_fraction`, `ford_count`, `fourwd_only_count` |
