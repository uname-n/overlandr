# Overlandr Reference

## Features

- Builds a routing graph from any `.osm.pbf` extract (state or regional)
- Sub-second k-alternatives via bidirectional Dijkstra + iterative edge penalty
- Tag-aware costing: tracktype, surface, smoothness, ford, 4WD-only
- Build-time scenic scoring from viewpoints, peaks, saddles, water, rivers, streams, forests, protected areas, nature reserves, glaciers, and cliffs
- Four built-in vehicle profiles: `stock-suv`, `high-clearance`, `4x4`, `dirtbike`
- Customizable TOML costing profiles
- Multi-track GPX output with per-route stats (`length`, `unpaved%`, `fords`, `score`)
- Fully offline, single binary, no daemon
- HTTP server mode (`serve`) for local development or cloud deployment

---

## Install

```bash
git clone https://github.com/uname-n/overlandr
cd overlandr
cargo build --release
# binary at ./target/release/overlandr
```

Requires Rust 1.80+.

---

## Quick Start

```bash
# 1. Build a graph from an OSM extract
overlandr build osm/oregon-260418.osm.pbf \
  --usfs shape/national_forest_system_roads/nfsr.shp \
  --out bin/oregon.bin

# 2. Start the server
overlandr --graph bin/oregon.bin serve --port 3000

# 3. Plan routes via HTTP
curl -X POST http://localhost:3000/route \
  -H 'Content-Type: application/json' \
  -d '{"from":[42.332,-122.872],"to":[46.190,-123.843],"tank_range_km":300}' \
  -o routes.gpx

# 4. Open routes.gpx in CalTopo, Gaia, or Garmin BaseCamp
```

OSM extracts: [Geofabrik](https://download.geofabrik.de/north-america/us.html)
USFS data: [National Forest System Roads](https://data.fs.usda.gov/geodata/edw/datasets.php?dsetCategory=transportation)

---

## Commands

### `build` — construct a routing graph

```
overlandr [--graph <OUT>] build <PBF> [OPTIONS]

Arguments:
  <PBF>                    Path to .osm.pbf extract

Options:
  --out <FILE>             Graph output path (default: graph.bin)
  --profile <FILE>         Custom TOML costing profile (must be a .toml file)
  --bbox <W,S,E,N>         Spatial pre-filter (lon/lat bbox)
  --keep-private           Include access=private ways
  --no-simplify            Skip degree-2 node contraction
  --usfs <FILE>            Merge a USFS NFS Roads shapefile (.shp) into the graph
  --usfs-snap <METRES>     Snap USFS road endpoints onto nearby OSM nodes (default: 50.0)
```

### `serve` — run as an HTTP server

```
overlandr --graph <FILE> serve [OPTIONS]

Options:
  --port <PORT>    TCP port to listen on (default: 3000, env: PORT)
  --host <HOST>    Bind address (default: 0.0.0.0, env: HOST)
```

Loads the graph once on startup and serves route queries over HTTP. The graph path also reads from the `GRAPH_PATH` environment variable.

**Server limits:** request body ≤ 64 KB · timeout 30 s · max 4 concurrent route computations · graceful shutdown on `SIGTERM`/`Ctrl-C`.

### `inspect` — print graph metadata

```
overlandr --graph <FILE> inspect
```

Prints the graph cache path, node count, edge count, profile fingerprint, and PBF timestamp.

### `tags` — print the costing table for a profile

```
overlandr tags [--profile <FILE>]
```

---

## Global Options

```
--graph <PATH>    Graph cache file (default: ./graph.bin, env: GRAPH_PATH)
--log <LEVEL>     trace | debug | info | warn | error (default: info)
--threads <N>     Thread pool size (optional; omit to use Rayon defaults)
```

---

## HTTP API

### `GET /health`

Liveness check. Returns JSON like:

```json
{"status":"ok","version":"<package-version>"}
```

### `POST /route`

Plan routes. Request body:

```json
{
  "from":          [44.919, -123.324],
  "to":            [45.715, -123.476],
  "vehicle":       "high-clearance",
  "alternatives":  3,
  "avoid_paved":   true,
  "avoid_fords":   true,
  "prefer_scenic": true,
  "scenic_weight": 1.0,
  "diversity":     0.35,
  "max_detour":    1.6,
  "tank_range_km": null,
  "lambda":        1.5,
  "fuel_buffer":   0.20
}
```

Successful responses return a GPX file (`application/gpx+xml`). Validation and routing failures return JSON error bodies.

| Field | Type | Default | Notes |
|---|---|---|---|
| `from` / `to` | `[lat, lon]` | required | WGS-84, lat ∈ [-90,90], lon ∈ [-180,180] |
| `vehicle` | string | `"high-clearance"` | Vehicle profile — see table below |
| `alternatives` | int ≥ 1 | `1` | Number of topologically distinct routes to return |
| `avoid_paved` | bool | `true` | Penalise paved edges |
| `avoid_fords` | bool | `true` | Disallow ford crossings |
| `prefer_scenic` | bool | `true` | Soft-bias toward edges near scenic OSM features such as viewpoints, peaks, water, rivers, forests, protected areas, glaciers, and cliffs |
| `scenic_weight` | float 0–1 | `1.0` | Strength of that scenic bias when `prefer_scenic=true` |
| `diversity` | float 0–1 | `0.35` | Min Jaccard distance between alternatives |
| `max_detour` | float > 0 | `1.6` | Max length ratio vs. shortest path |
| `tank_range_km` | float \| null | `null` | Trigger fuel-stop planning at this tank range |
| `lambda` | float | `1.5` | Penalty growth factor for k-alternatives |
| `fuel_buffer` | float | `0.20` | Fuel reserve fraction (20 % = stop at 80 % consumed) |

**Validation:** requests with coordinates out of range, non-positive `max_detour`, or `diversity` / `scenic_weight` outside 0–1 are rejected with HTTP 422.

---

## Vehicle Profiles

Pass the profile name as `"vehicle"` in the `/route` request body.

| Profile | 4WD penalty | Min smoothness | Notes |
|---|---|---|---|
| `stock-suv` | 9999 (blocked) | bad | Paved + easy gravel only |
| `high-clearance` | 4.0 | very_bad | **Default** |
| `4x4` | 1.0 | horrible | No penalty for technical roads |
| `dirtbike` | 1.0 | very_horrible | Prefers narrow singletrack |

The 4WD penalty multiplies the cost of `4wd_only=yes` edges. Min smoothness hard-blocks edges rougher than the vehicle can handle.

When `prefer_scenic=true`, overlandr also applies a bounded bonus to edges with higher build-time scenic scores. Those scores are derived from nearby OSM viewpoints, peaks, saddles, water, rivers, streams, forests, protected areas, nature reserves, glaciers, and cliffs. Scenic preference is soft: it can change corridor choice, but it will not override hard access or safety penalties like `avoid_paved`, `avoid_fords`, or blocked smoothness.

---

## Custom Profiles

Copy and edit `profiles/overland.toml`. The file must have a `.toml` extension and must not contain `..` in the path.

```toml
# OSM highway= tag → cost multiplier (lower = preferred).
# Keys here also define the allowlist of highway classes loaded from the PBF —
# any highway= value not listed is dropped during graph build.
[base]
track   = 0.6
primary = 8.0
motorway = 12.0

[surface]
dirt    = 0.65
asphalt = 1.5
sand    = 1.1

[vehicle.4x4]
fourwd_only_penalty = 1.0
min_smoothness      = "horrible"

# Optional: override routing algorithm constants (all have defaults)
[routing]
lambda       = 1.5    # k-alternatives penalty growth factor
fuel_buffer  = 0.20   # fuel reserve fraction (stop at 80% consumed)
grid_step    = 0.001  # USFS conflation grid cell size in degrees
ford_penalty = 3.0    # cost multiplier for ford (water crossing) edges

# OSM surface= values classified as paved (receive PAVED edge flag)
paved_surfaces = ["asphalt", "paved", "concrete", "cobblestone", "sett"]

# OSM smoothness= values that set rough / very-rough edge flags
smoothness_rough      = ["very_bad", "horrible", "very_horrible", "impassable"]
smoothness_very_rough = ["horrible", "very_horrible", "impassable"]
```

Pass with `--profile my-profile.toml` on `build`.

> The profile hash is baked into the graph file — if your profile changes, rebuild the graph.
>
> `lambda` and `fuel_buffer` can also be overridden per-request in the HTTP API without rebuilding.

---

## Output GPX

Each alternative is a separate `<trk>` in the output file. The `<desc>` element includes per-route stats:

```
length=39.7km unpaved=57% fords=0 4wd_only=0 score=0.34 fuel_stops=1
```

Here, `score` is the weighted unpaved component written into the GPX track description, and `fuel_stops` appears only when at least one stop is planned.

When `tank_range_km` is set, fuel stops are emitted as `<wpt>` elements before the tracks. Each waypoint has `sym=fuel`, `type=fuel`, and a `<desc>` showing its position along the route (e.g. `"142.3 km along route"`).

Load the file in CalTopo, Gaia GPS, OnX Offroad, or Garmin BaseCamp to compare routes on the map.

---

## Performance

Tested on an M-series laptop with a ~250 MB Idaho `.osm.pbf`:

| Operation | Target |
|---|---|
| `build` (cold) | < 90 s |
| `build` (cached load) | < 2 s |
| `POST /route` k=3 | < 800 ms |
| `POST /route` k=10 | < 3 s |
| Peak RSS (build) | < 4 GB |
| Peak RSS (route) | < 1 GB |

---

## Testing

```bash
cargo test          # unit + integration tests
cargo bench         # criterion benchmarks (dijkstra, build pipeline)
```
