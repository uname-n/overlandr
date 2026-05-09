# Overlandr Agent Guide

## Project overview
- Overlandr builds and serves overlanding route graphs from OpenStreetMap and USFS data.
- Primary flows:
  - `cargo test -q` for fast validation
  - `cargo run -- build ...` to build graph caches
  - `cargo run -- serve --graph ...` to serve `/route`
- Main docs:
  - `README.md`
  - `docs/reference.md`
  - `docs/algorithm.md`

## Code map
- `src/main.rs` — CLI entrypoint, build flow, route execution core
- `src/serve.rs` — HTTP server, request validation, GPX responses
- `src/graph/` — graph build, cache, contraction, spatial index
- `src/routing/` — Dijkstra, k-alternatives, fuel planning, scoring
- `src/osm/` — PBF loading and tag filtering
- `src/usfs/` — USFS shapefile import
- `src/gpx/` — GPX output
- `src/profile/` — profile loading and TOML mapping
- `profiles/overland.toml` — default routing + vehicle tuning
- `tests/` — integration coverage for build, fuel, route behavior

## Canonical commands
- Validate: `cargo test -q`
- Benchmarks: `cargo bench`
- Build app: `cargo build --release`
- Build a state graph: `make oregon` / `make idaho` / `make montana`
- Inspect a graph: `cargo run -- inspect --graph ./bin/oregon.bin`
- Serve a graph: `cargo run -- --graph ./bin/oregon.bin serve`
- Sample route request:
  - `curl -X POST http://localhost:3000/route -H 'Content-Type: application/json' -d '{"from":[44.919,-123.324],"to":[45.715,-123.476]}' -o routes.gpx`

## Repository rules
- Treat `osm/*.osm.pbf`, `bin/*.bin`, and `target/` as large/generated artifacts. Do not edit or scan them unless the user explicitly asks.
- Prefer `read` for source/docs and `bash` with targeted paths for discovery.
- Preserve request coordinate order as `[lat, lon]`.
- Keep CLI, HTTP API, and GPX output backward compatible unless the user requests a breaking change.
- When changing routing/profile behavior, run focused tests and note likely effects on route selection.
- When changing `profiles/overland.toml`, explain the routing tradeoff being adjusted.
- Avoid destructive release commands (`docker buildx build`, package pruning, deletes) unless explicitly requested.

## Suggested review focus
- Correctness of route snapping and graph traversal
- Alternative-route diversity behavior
- Fuel-planning edge cases
- Performance of graph build and routing hot paths
- API validation and operational safety in `src/serve.rs`
