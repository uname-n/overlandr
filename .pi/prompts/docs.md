---
description: Review Overlandr docs and update README.md, docs/reference.md, and docs/algorithm.md to match the current codebase
---
Review overlandr and ensure these docs are accurate and synchronized with the current implementation:
- `README.md`
- `docs/reference.md`
- `docs/algorithm.md`

Source of truth:
- `src/main.rs`
- `src/serve.rs`
- `src/graph/`
- `src/routing/`
- `src/osm/`
- `src/usfs/`
- `src/gpx/`
- `src/profile/`
- `profiles/overland.toml`
- `tests/`

Requirements:
1. Audit the code and compare it to the three docs above.
2. Correct outdated CLI, HTTP API, routing, graph-build, profile, performance, or output details.
3. Preserve backward-compatible wording unless the code clearly changed.
4. Preserve Overlandr conventions:
   - coordinates are `[lat, lon]`
   - avoid scanning or modifying generated artifacts such as `osm/*.osm.pbf`, `bin/*.bin`, and `target/`
5. Prefer minimal, high-signal doc edits rather than rewrites.
6. If a claim cannot be confirmed from code or tests, remove it or soften it.
7. After edits, give a short summary of:
   - what changed
   - any remaining uncertain or undocumented behavior
   - any code/docs mismatches that should be addressed separately

Be proactive: if the docs are already correct, say so explicitly after verifying them.