# overlandr

Discover diverse overlanding routes between two GPS coordinates using OpenStreetMap data.

Mainstream routers optimize for one fastest path. Overlandr does the opposite — it generates **multiple topologically distinct alternatives** ranked by offroad appeal: forest service roads, 4×4 tracks, gravel connectors, and scenic corridors influenced by nearby viewpoints, water, forests, and protected lands. Output is a multi-track GPX ready for CalTopo, Gaia, OnX, or Garmin BaseCamp.

---

## Docker Quickstart

If you are using the published GHCR images, the graph is baked in — no volume mount needed:

```bash
docker pull ghcr.io/uname-n/overlandr:oregon
docker run -p 3000:3000 ghcr.io/uname-n/overlandr:oregon
```

Available tags: `oregon`, `idaho`, `montana`, `arizona`. Each image serves on port 3000.

Plan routes via HTTP:

```bash
curl -X POST http://localhost:3000/route \
  -H 'Content-Type: application/json' \
  -d '{"from":[44.919,-123.324],"to":[45.715,-123.476]}' \
  -o routes.gpx
```

Open `routes.gpx` in CalTopo, Gaia GPS, OnX Offroad, or Garmin BaseCamp.

---

## Docs

- [Reference](docs/reference.md) — CLI commands, HTTP API, scenic routing options, vehicle profiles, custom profiles, output format, performance
- [Algorithm](docs/algorithm.md) — graph construction, scenic edge scoring, k-alternatives, ranking

## Examples

### California → Astoria, Oregon
```bash
curl -X POST http://localhost:3000/route \
  -H 'Content-Type: application/json' \
  -d '{"from":[41.99848, -123.72199],"to":[46.18523, -123.82365], "vehicle":"4x4"}' \
  -o routes/199_california-astoria_oregon.gpx
```

### Preston, Idaho → Montana 2
```bash
curl -X POST http://localhost:3000/route \
  -H 'Content-Type: application/json' \
  -d '{"from":[41.99985, -111.81281],"to":[48.63361, -116.04955], "vehicle":"4x4"}' \
  -o routes/91_preston_idaho-2_montana.gpx
```

### Montana 2 → Wyoming 59
```bash
curl -X POST http://localhost:3000/route \
  -H 'Content-Type: application/json' \
  -d '{"from":[48.63144, -116.04772],"to":[45.00086, -105.37186], "vehicle":"4x4"}' \
  -o routes/2_montana-59_wyoming.gpx
```