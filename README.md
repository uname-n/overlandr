# overlandr

Discover diverse overlanding routes between two GPS coordinates using OpenStreetMap data.

Mainstream routers optimize for one fastest path. Overlandr does the opposite — it generates **multiple topologically distinct alternatives** ranked by offroad appeal: forest service roads, 4×4 tracks, gravel connectors, abandoned alignments, and scenic corridors influenced by nearby viewpoints, water, forests, and protected lands. Output is a multi-track GPX ready for CalTopo, Gaia, OnX, or Garmin BaseCamp.

---

## Docker Quickstart

Pre-built images are published to GHCR with the graph baked in — no volume mount needed:

```bash
docker pull ghcr.io/uname-n/overlandr:oregon
docker run -p 3000:3000 ghcr.io/uname-n/overlandr:oregon
```

Available tags: `oregon`, `idaho`, `montana`. Each image serves on port 3000.

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
